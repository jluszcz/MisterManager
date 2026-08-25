//! The Recurring Goals screen: the recurring goals a new round is created
//! from, the month filter over them, and the form that adds or edits them.
//!
//! View state only -- no ratatui above the render functions at the bottom, and
//! no `Db` on the type. `App` runs the queries and hands the results in.

use super::Label;
use super::cursor::{Cursor, Viewport, impl_scroll};
use super::form::{
    Caret, Field, Focused, FormFields, Step, field_line_noted, next_in, parse_whole_amount,
    step_index, tax_note,
};
use super::month::MonthCycle;
use super::search::{Search, SearchBox};
use crate::db::RecurringGoalId;
use crate::db::recurring_goal::{Cadence, Entry, NewEntry};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Result, ensure};
use std::collections::HashMap;

/// One recurring goal entry as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub recurring_goal_id: RecurringGoalId,
    pub name: String,
    pub month: i64,
    pub base_cents: Cents,
    pub taxed: bool,
    pub cadence: Cadence,
    /// How many goals this entry currently has open. A hint only -- a second
    /// open goal against one entry is a legitimate thing to want, the same as
    /// the picker's "Open?" column.
    pub open_goals: i64,
}

pub struct RecurringGoals {
    all: Vec<Row>,
    /// Indices into `all` that survive the month filter and the needle.
    visible: Vec<usize>,
    /// The cycle is the calendar itself: the entries carry a month of the
    /// year and no date, so every month is always in it and the ends wrap.
    month: MonthCycle<i64>,
    search: SearchBox,
    cursor: Cursor,
}

impl RecurringGoals {
    /// Opens unfiltered. The screen is a reference list of a dozen-odd
    /// entries, so showing all of them is the useful default; `[` and `]` are
    /// what start narrowing it.
    pub fn new(today_month: i64) -> RecurringGoals {
        RecurringGoals {
            all: Vec::new(),
            visible: Vec::new(),
            month: MonthCycle::new((1..=12).collect(), today_month),
            search: SearchBox::new(),
            cursor: Cursor::new(),
        }
    }

    pub fn set_entries(&mut self, entries: Vec<Entry>, open_goals: HashMap<RecurringGoalId, i64>) {
        self.all = entries
            .into_iter()
            .map(|entry| Row {
                open_goals: open_goals.get(&entry.id).copied().unwrap_or(0),
                recurring_goal_id: entry.id,
                name: entry.name,
                month: entry.month,
                base_cents: entry.base_cents,
                taxed: entry.taxed,
                cadence: entry.cadence,
            })
            .collect();
        self.refilter();
    }

    /// `]`: forward a month, December wrapping to January.
    ///
    /// The entries carry a month of the year and no date, so there is no data
    /// range to clamp against the way [`super::ledger::Ledger`] has one.
    pub fn next_month(&mut self) {
        self.month.next();
        self.refilter();
    }

    /// `[`: back a month, January wrapping to December.
    pub fn previous_month(&mut self) {
        self.month.previous();
        self.refilter();
    }

    /// `Esc`: show every entry again. The needle is cleared before this is
    /// reached -- see [`super::search::escape_kept_filter`] -- so the key
    /// backs out of the innermost filter first, as it does on Savings.
    pub fn clear_month(&mut self) {
        self.month.clear();
        self.refilter();
    }

    pub fn selected_month(&self) -> Option<i64> {
        self.month.selected()
    }

    pub fn rows(&self) -> Vec<&Row> {
        self.visible.iter().map(|i| &self.all[*i]).collect()
    }

    pub fn selected(&self) -> Option<&Row> {
        self.visible.get(self.cursor.index()).map(|i| &self.all[*i])
    }

    pub fn title(&self) -> String {
        let month = match self.month.selected() {
            None => "All".to_string(),
            Some(m) => super::month_name(m),
        };
        let mut title = format!("Recurring Goals · {month} · {}", self.visible.len());
        if !self.search().is_empty() {
            title = format!("{title} · /{}", self.search());
        }
        title
    }
}

impl Search for RecurringGoals {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    /// The month filter and the needle in one pass, so the two cannot narrow
    /// to different lists. Also the hook `[` and `]` call when they move.
    ///
    /// An entry answers to its name and to the one figure it is *about*, the
    /// base. The month is `[`/`]`'s already, and the open-goal count is a
    /// tally rather than a figure the entry carries -- a needle reaching
    /// either would narrow through a column nobody was searching.
    fn refilter(&mut self) {
        let matcher = self.matcher();
        let month = self.month.selected();
        self.visible = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, row)| month.is_none_or(|m| row.month == m))
            .filter(|(_, row)| matcher.matches(&row.name, &[row.base_cents]))
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
    }
}

impl_scroll!(RecurringGoals, visible);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecurringGoalField {
    Name,
    Month,
    Amount,
    Taxed,
    Cadence,
}

impl RecurringGoalField {
    pub const ORDER: [RecurringGoalField; 5] = [
        RecurringGoalField::Name,
        RecurringGoalField::Month,
        RecurringGoalField::Amount,
        RecurringGoalField::Taxed,
        RecurringGoalField::Cadence,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RecurringGoalField::Name => "Name",
            RecurringGoalField::Month => "Month",
            RecurringGoalField::Amount => "Base",
            RecurringGoalField::Taxed => "Taxed",
            RecurringGoalField::Cadence => "Cadence",
        }
    }
}

/// A month of the year `delta` months on, December wrapping to January and
/// January back to December. The form's Month selector steps the same
/// calendar the screen's filter does, but as a field rather than a filter --
/// it has no All to fall out of, so it is not a [`MonthCycle`].
fn wrapped_month(month: i64, delta: isize) -> i64 {
    (month - 1 + delta as i64).rem_euclid(12) + 1
}

/// Adding or editing one recurring goal entry. Backs `a` and `e`.
///
/// Month, Taxed and Cadence are selectors rather than text fields, so none of
/// an out-of-range month, an unparseable boolean, or a cadence the schema's
/// `CHECK` would refuse is representable.
#[derive(Debug)]
pub struct RecurringGoalForm {
    pub editing: Option<RecurringGoalId>,
    pub focus: RecurringGoalField,
    name: Field,
    /// 1-12. A number rather than a `Field`, because the Month selector can
    /// only ever hold one.
    month: i64,
    amount: Field,
    taxed: bool,
    cadence: usize,
    /// The sales tax rate the `Taxed` note applies, as it stood when the form
    /// opened. `None` is a database no `Constants` sheet has been imported
    /// into: the note simply says nothing. Unlike the goal form, this one
    /// refuses nothing for it -- an entry writes no goal, and the picker that
    /// turns one into a goal is where the rate is refused instead.
    rate: Option<BasisPoints>,
}

impl RecurringGoalForm {
    pub fn add(rate: Option<BasisPoints>) -> RecurringGoalForm {
        RecurringGoalForm {
            editing: None,
            focus: RecurringGoalField::Name,
            name: Field::default(),
            month: 1,
            amount: Field::default(),
            taxed: false,
            cadence: 0,
            rate,
        }
    }

    pub fn edit(entry: &Entry, rate: Option<BasisPoints>) -> RecurringGoalForm {
        RecurringGoalForm {
            editing: Some(entry.id),
            focus: RecurringGoalField::Name,
            name: Field::given(entry.name.clone()),
            // A row outside 1-12 is corrupt; the selector has no way to show
            // one, so it opens on January rather than refusing to open.
            month: entry.month.clamp(1, 12),
            amount: Field::given(entry.base_cents.to_string()),
            taxed: entry.taxed,
            cadence: Cadence::ALL
                .iter()
                .position(|c| *c == entry.cadence)
                .unwrap_or(0),
            rate,
        }
    }

    /// The note beside the Base: what it comes to once the tax lambda has had
    /// it -- the same sentence, in the same place, that the goal form's Target
    /// carries, so the two forms answer the same question the same way. This
    /// form asks nothing about the rate: an entry writes no goal, and it is
    /// `App::commit_picker` -- the picker that turns a taxed entry into one --
    /// that refuses when no rate is on record.
    pub fn tax_note(&self) -> String {
        tax_note(self.taxed, self.amount.value(), self.rate)
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => "Edit recurring goal — Tab field · ←/→ selector · Enter save · Esc cancel",
            None => "Add recurring goal — Tab field · ←/→ selector · Enter save · Esc cancel",
        }
    }

    pub fn display(&self, field: RecurringGoalField) -> Label {
        Label::plain(match field {
            RecurringGoalField::Name => crate::demo::text(self.name.value()).into_owned(),
            RecurringGoalField::Month => super::month_full_name(self.month),
            RecurringGoalField::Amount => crate::demo::typed(self.amount.value()),
            RecurringGoalField::Taxed => (if self.taxed { "yes" } else { "no" }).to_string(),
            RecurringGoalField::Cadence => Cadence::ALL[self.cadence].as_str().to_string(),
        })
    }

    pub fn commit(&self) -> Result<NewEntry> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        Ok(NewEntry {
            name,
            month: self.month,
            base_cents: parse_whole_amount(self.amount.value())?,
            taxed: self.taxed,
            cadence: Cadence::ALL[self.cadence],
        })
    }
}

impl FormFields for RecurringGoalForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&RecurringGoalField::ORDER, self.focus, step);
    }

    fn cycle(&mut self, step: Step) {
        match self.focus {
            RecurringGoalField::Month => self.month = wrapped_month(self.month, step.direction()),
            RecurringGoalField::Taxed => self.taxed = !self.taxed,
            RecurringGoalField::Cadence => {
                self.cadence = step_index(self.cadence, Cadence::ALL.len(), step.direction())
            }
            _ => {}
        }
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            RecurringGoalField::Name => Focused::Text(&mut self.name),
            RecurringGoalField::Amount => Focused::Text(&mut self.amount),
            RecurringGoalField::Month | RecurringGoalField::Taxed | RecurringGoalField::Cadence => {
                Focused::Selector
            }
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            RecurringGoalField::Name => Caret::in_field(&self.name),
            RecurringGoalField::Amount => Caret::in_field(&self.amount),
            RecurringGoalField::Month | RecurringGoalField::Taxed | RecurringGoalField::Cadence => {
                Caret::End
            }
        }
    }
}

use super::form::render_fields;
use super::{Chrome, amount, month_name, render_table, right_header};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Cell, Row as TableRow};

pub fn render_form(frame: &mut Frame, form: &RecurringGoalForm) {
    let note = form.tax_note();
    let lines: Vec<TextLine> = RecurringGoalField::ORDER
        .iter()
        .map(|f| {
            // The Base field holds the pre-tax figure, so `Taxed` says what it
            // comes to beside the figure it applies to rather than beside
            // itself -- the goal form's Target does the same.
            let note = if *f == RecurringGoalField::Amount {
                note.as_str()
            } else {
                ""
            };
            field_line_noted(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
                note,
            )
        })
        .collect();
    render_fields(frame, form.title(), lines);
}

/// One row per recurring goal entry. Returns the [`Viewport`] it drew: the
/// height `PageUp`/`PageDown` move by, and the row the next draw starts from.
pub(super) fn render(frame: &mut Frame, area: Rect, recurring_goal: &RecurringGoals) -> Viewport {
    let rows: Vec<TableRow> = recurring_goal
        .rows()
        .iter()
        .map(|r| {
            TableRow::new(vec![
                Cell::from(crate::demo::text(&r.name).into_owned()),
                Cell::from(month_name(r.month)),
                amount(r.base_cents),
                Cell::from(if r.taxed { "yes" } else { "no" }),
                Cell::from(r.cadence.as_str()),
                Cell::from(TextLine::from(r.open_goals.to_string()).right_aligned()),
            ])
        })
        .collect();

    let header = TableRow::new(vec![
        Cell::from("Name"),
        Cell::from("Month"),
        right_header("Base"),
        Cell::from("Taxed"),
        Cell::from("Cadence"),
        right_header("Open"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Min(16),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Length(4),
    ];

    render_table(
        frame,
        area,
        recurring_goal,
        Chrome::titled(recurring_goal.title()).header(header),
        &widths,
        rows,
        recurring_goal.rows().len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::walk_until;
    use crate::tui::MIN_WIDTH;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::{backspace_key, char_key};

    fn entry(id: i64, name: &str, month: i64, cadence: Cadence) -> Entry {
        Entry {
            id: RecurringGoalId(id),
            name: name.to_string(),
            month,
            base_cents: Cents::from_dollars(128),
            taxed: false,
            cadence,
        }
    }

    /// Four entries in four different months, unfiltered, with the current
    /// month set to September — where `Dropbox` falls, and the only month of
    /// the four that `[` and `]` reach without stepping.
    fn screen() -> RecurringGoals {
        let mut recurring_goal = RecurringGoals::new(9);
        recurring_goal.set_entries(
            vec![
                entry(1, "Car Insurance", 3, Cadence::Annual),
                entry(2, "Dropbox", 9, Cadence::Annual),
                entry(3, "Backblaze", 11, Cadence::Biennial),
                entry(4, "Lego", 12, Cadence::Annual),
            ],
            HashMap::from([(RecurringGoalId(1), 2), (RecurringGoalId(4), 1)]),
        );
        recurring_goal
    }

    /// Two entries whose bases differ, for the tests about matching a figure
    /// -- `screen`'s four all cost the same, so a needle could not tell them
    /// apart. Dropbox carries three open goals, which is the tally a needle
    /// must *not* reach: no digit of either base is a `3`.
    fn priced() -> RecurringGoals {
        let mut recurring_goal = RecurringGoals::new(9);
        let mut entries = vec![
            entry(1, "Car Insurance", 3, Cadence::Annual),
            entry(2, "Dropbox", 9, Cadence::Annual),
        ];
        entries[0].base_cents = Cents::from_dollars(1_240);
        entries[1].base_cents = Cents::from_dollars(96);
        recurring_goal.set_entries(entries, HashMap::from([(RecurringGoalId(2), 3)]));
        recurring_goal
    }

    /// The same four entries, narrowed to September by one step out of All.
    fn september() -> RecurringGoals {
        let mut recurring_goal = screen();
        recurring_goal.next_month();
        recurring_goal
    }

    fn names(recurring_goal: &RecurringGoals) -> Vec<&str> {
        recurring_goal
            .rows()
            .iter()
            .map(|r| r.name.as_str())
            .collect()
    }

    #[test]
    fn each_row_carries_how_many_goals_are_open_against_it() {
        let recurring_goal = screen();
        let open: Vec<i64> = recurring_goal.rows().iter().map(|r| r.open_goals).collect();
        assert_eq!(open, vec![2, 0, 0, 1]);
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut recurring_goal = screen();
        assert_eq!(
            recurring_goal.selected().unwrap().recurring_goal_id,
            RecurringGoalId(1)
        );
        recurring_goal.select_previous();
        assert_eq!(
            recurring_goal.selected().unwrap().recurring_goal_id,
            RecurringGoalId(1)
        );

        recurring_goal.select_last();
        assert_eq!(
            recurring_goal.selected().unwrap().recurring_goal_id,
            RecurringGoalId(4)
        );
        recurring_goal.select_next();
        assert_eq!(
            recurring_goal.selected().unwrap().recurring_goal_id,
            RecurringGoalId(4)
        );

        recurring_goal.select_first();
        assert_eq!(recurring_goal.selected_index(), 0);
    }

    /// `d` refuses rather than deleting outright, so a cursor left past the
    /// end would make `e` and `d` act on nothing -- or on whatever later
    /// lands at that index.
    #[test]
    fn a_shrinking_list_moves_the_selection_into_bounds() {
        let mut recurring_goal = screen();
        recurring_goal.select_last();

        recurring_goal.set_entries(
            vec![entry(1, "Car Insurance", 3, Cadence::Annual)],
            HashMap::new(),
        );

        assert_eq!(recurring_goal.selected_index(), 0);
        assert_eq!(
            recurring_goal.selected().unwrap().recurring_goal_id,
            RecurringGoalId(1)
        );
    }

    #[test]
    fn an_empty_list_has_nothing_selected() {
        let mut recurring_goal = RecurringGoals::new(9);
        recurring_goal.set_entries(Vec::new(), HashMap::new());
        assert!(recurring_goal.rows().is_empty());
        assert!(recurring_goal.selected().is_none());
    }

    /// Nothing is hidden until a key is pressed: the screen is a reference
    /// list, and most months hold no entry at all.
    #[test]
    fn the_screen_opens_unfiltered() {
        let recurring_goal = screen();
        assert_eq!(recurring_goal.selected_month(), None);
        assert_eq!(
            names(&recurring_goal),
            ["Car Insurance", "Dropbox", "Backblaze", "Lego"]
        );
    }

    /// Either key enters the calendar at the current month, so the first
    /// press narrows rather than stepping somewhere arbitrary.
    #[test]
    fn the_first_step_filters_to_the_current_month() {
        let mut forward = screen();
        forward.next_month();
        assert_eq!(forward.selected_month(), Some(9));
        assert_eq!(names(&forward), ["Dropbox"]);

        let mut back = screen();
        back.previous_month();
        assert_eq!(back.selected_month(), Some(9));
    }

    /// The entries carry a month of the year and no date, so the cycle is the
    /// calendar itself rather than a window clamped to the range of real
    /// data, the way the ledgers' `[` and `]` are.
    #[test]
    fn stepping_forward_wraps_december_to_january() {
        let mut recurring_goal = september();
        for expected in [10, 11, 12, 1, 2] {
            recurring_goal.next_month();
            assert_eq!(recurring_goal.selected_month(), Some(expected));
        }
    }

    #[test]
    fn stepping_back_wraps_january_to_december() {
        let mut recurring_goal = september();
        for expected in [8, 7, 6, 5, 4, 3, 2, 1, 12, 11] {
            recurring_goal.previous_month();
            assert_eq!(recurring_goal.selected_month(), Some(expected));
        }
    }

    #[test]
    fn esc_shows_every_entry_again() {
        let mut recurring_goal = september();
        assert_eq!(names(&recurring_goal), ["Dropbox"]);

        recurring_goal.clear_month();

        assert_eq!(recurring_goal.selected_month(), None);
        assert_eq!(
            names(&recurring_goal),
            ["Car Insurance", "Dropbox", "Backblaze", "Lego"]
        );
    }

    /// No state is carried across the All filter, so both keys re-enter at
    /// the current month rather than at whichever month `Esc` was pressed
    /// from.
    #[test]
    fn stepping_out_of_all_re_enters_at_the_current_month() {
        let mut recurring_goal = september();
        recurring_goal.next_month();
        recurring_goal.next_month();
        assert_eq!(recurring_goal.selected_month(), Some(11));

        recurring_goal.clear_month();
        recurring_goal.next_month();
        assert_eq!(recurring_goal.selected_month(), Some(9));

        recurring_goal.clear_month();
        recurring_goal.previous_month();
        assert_eq!(recurring_goal.selected_month(), Some(9));
    }

    /// Most months hold no entry at all. An empty table is the honest answer
    /// there, and `e` and `d` must find nothing selected rather than a row
    /// the filter excluded.
    #[test]
    fn a_month_with_no_entries_leaves_an_empty_list() {
        let mut recurring_goal = september();
        recurring_goal.next_month();
        assert_eq!(recurring_goal.selected_month(), Some(10));
        assert!(recurring_goal.rows().is_empty());
        assert!(recurring_goal.selected().is_none());
    }

    /// `e` and `d` act on the selection, so a cursor left past the end of a
    /// narrowed list would edit whatever later landed at that index.
    #[test]
    fn the_cursor_moves_into_bounds_when_the_month_filter_narrows_the_list() {
        let mut recurring_goal = screen();
        recurring_goal.select_last();
        assert_eq!(recurring_goal.selected().unwrap().name, "Lego");

        recurring_goal.next_month();

        assert_eq!(recurring_goal.selected_index(), 0);
        assert_eq!(recurring_goal.selected().unwrap().name, "Dropbox");
    }

    #[test]
    fn the_title_names_the_filter_and_how_many_rows_it_left() {
        let mut recurring_goal = september();
        assert_eq!(recurring_goal.title(), "Recurring Goals · Sep · 1");

        recurring_goal.clear_month();
        assert_eq!(recurring_goal.title(), "Recurring Goals · All · 4");

        recurring_goal.next_month();
        assert_eq!(recurring_goal.title(), "Recurring Goals · Sep · 1");
    }

    /// Both filters, side by side, so the title says which of the two left
    /// the count it quotes.
    #[test]
    fn the_title_names_the_search_beside_the_month() {
        let mut recurring_goal = screen();
        recurring_goal.begin_search();
        for c in "dro".chars() {
            recurring_goal.push_search(c);
        }
        assert_eq!(recurring_goal.title(), "Recurring Goals · All · 1 · /dro");

        recurring_goal.next_month();
        assert_eq!(recurring_goal.title(), "Recurring Goals · Sep · 1 · /dro");
    }

    #[test]
    fn search_matches_names_case_insensitively_and_narrows_the_list() {
        let mut recurring_goal = screen();
        recurring_goal.begin_search();
        for c in "BAC".chars() {
            recurring_goal.push_search(c);
        }
        assert_eq!(names(&recurring_goal), ["Backblaze"]);

        recurring_goal.edit_search(backspace_key());
        recurring_goal.edit_search(backspace_key());
        recurring_goal.edit_search(backspace_key());
        assert_eq!(names(&recurring_goal).len(), 4);
    }

    /// The one figure an entry is *about*, typed without the separators the
    /// column draws.
    #[test]
    fn search_matches_an_entrys_base() {
        let mut recurring_goal = priced();
        recurring_goal.begin_search();
        for c in "1240".chars() {
            recurring_goal.push_search(c);
        }
        assert_eq!(names(&recurring_goal), ["Car Insurance"]);
    }

    /// `Open` is a tally of the goals made from an entry rather than a figure
    /// the entry carries, so a needle reaching it would narrow through a
    /// column nobody was searching. Nothing here answers to the three goals
    /// open against Dropbox.
    #[test]
    fn search_does_not_reach_the_open_goal_tally() {
        let mut recurring_goal = priced();
        recurring_goal.begin_search();
        recurring_goal.push_search('3');
        assert!(names(&recurring_goal).is_empty());
    }

    /// One pass over the entries, so the two filters cannot narrow to
    /// different lists: `Dropbox` answers the needle but falls outside March.
    #[test]
    fn the_month_narrows_within_the_search() {
        let mut recurring_goal = screen();
        recurring_goal.begin_search();
        for c in "o".chars() {
            recurring_goal.push_search(c);
        }
        assert_eq!(names(&recurring_goal), ["Dropbox", "Lego"]);

        recurring_goal.next_month();
        assert_eq!(names(&recurring_goal), ["Dropbox"]);
        recurring_goal.next_month();
        assert!(names(&recurring_goal).is_empty());
    }

    /// `Esc` on the screen clears the month; the needle has its own `Esc`
    /// inside the box, and `search::escape_kept_filter` is what reaches a
    /// kept one first.
    #[test]
    fn clearing_the_month_leaves_the_search_alone() {
        let mut recurring_goal = september();
        recurring_goal.begin_search();
        for c in "dropbox".chars() {
            recurring_goal.push_search(c);
        }

        recurring_goal.clear_month();
        assert_eq!(recurring_goal.search(), "dropbox");
        assert_eq!(names(&recurring_goal), ["Dropbox"]);
    }

    /// `e`, `d` and `s` act on the selection, so a cursor left past the end
    /// of a narrowed list would act on whatever later lands at that index.
    #[test]
    fn a_shrinking_search_moves_the_selection_into_bounds() {
        let mut recurring_goal = screen();
        recurring_goal.select_last();
        assert_eq!(recurring_goal.selected().unwrap().name, "Lego");

        recurring_goal.begin_search();
        for c in "drop".chars() {
            recurring_goal.push_search(c);
        }
        assert_eq!(recurring_goal.selected_index(), 0);
        assert_eq!(recurring_goal.selected().unwrap().name, "Dropbox");
    }

    #[test]
    fn an_empty_result_has_nothing_selected() {
        let mut recurring_goal = screen();
        recurring_goal.begin_search();
        for c in "zzz".chars() {
            recurring_goal.push_search(c);
        }
        assert!(recurring_goal.rows().is_empty());
        assert!(recurring_goal.selected().is_none());
    }

    /// The same question the goal form answers, in the same words: the field
    /// holds the base, and the note says what the goal made from it will
    /// actually be funded to.
    #[test]
    fn the_note_beside_the_base_says_what_it_comes_to_with_tax() {
        let mut form = RecurringGoalForm::add(Some(BasisPoints(625)));
        assert_eq!(form.tax_note(), "", "nothing to say while the flag is off");

        walk_until!(form.focus == RecurringGoalField::Amount, form.next_field());
        for c in "1000".chars() {
            form.edit(char_key(c));
        }
        walk_until!(form.focus == RecurringGoalField::Taxed, form.next_field());
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "(1,065 w/ tax)");
        assert_eq!(
            form.display(RecurringGoalField::Amount).plain_text(),
            "1000",
            "the field itself still holds the base"
        );
    }

    /// No rate on record is a database nobody has imported `Constants` into.
    /// The form still opens and simply says nothing, the way the goal form's
    /// note does -- this screen writes no goal, so there is nothing to refuse.
    #[test]
    fn the_note_is_empty_with_no_rate_on_record() {
        let mut form = RecurringGoalForm::add(None);
        walk_until!(form.focus == RecurringGoalField::Amount, form.next_field());
        for c in "1000".chars() {
            form.edit(char_key(c));
        }
        walk_until!(form.focus == RecurringGoalField::Taxed, form.next_field());
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
    }

    /// A half-typed base is not a figure yet, and a note that guessed at one
    /// would flicker through amounts the owner never asked about.
    #[test]
    fn the_note_stays_empty_until_the_base_is_a_whole_figure() {
        let mut form = RecurringGoalForm::add(Some(BasisPoints(625)));
        walk_until!(form.focus == RecurringGoalField::Amount, form.next_field());
        for c in "1000.5".chars() {
            form.edit(char_key(c));
        }
        walk_until!(form.focus == RecurringGoalField::Taxed, form.next_field());
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
    }

    #[test]
    fn a_catalog_form_commits_what_was_typed() {
        let mut form = RecurringGoalForm::add(None);
        assert_eq!(form.editing, None);

        for c in "Dropbox".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for _ in 1..9 {
            form.choice(Step::NEXT);
        }
        form.next_field();
        for c in "128".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        form.choice(Step::NEXT);
        form.next_field();
        form.choice(Step::NEXT);

        let new = form.commit().unwrap();
        assert_eq!(new.name, "Dropbox");
        assert_eq!(new.month, 9);
        assert_eq!(new.base_cents, Cents::from_dollars(128));
        assert!(new.taxed);
        assert_eq!(new.cadence, Cadence::Biennial);
    }

    /// The base amount is money; the month, the cadence and whether it is
    /// taxed are the rule rather than the figure.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_base_amount_and_keeps_the_rule() {
        crate::demo::install_with_salt(7);
        let form = RecurringGoalForm::edit(
            &Entry {
                id: RecurringGoalId(2),
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: true,
                cadence: Cadence::Biennial,
            },
            None,
        );
        let drawn = form.display(RecurringGoalField::Amount).plain_text();
        assert_ne!(drawn, "128.00");
        assert_eq!(drawn.len(), "128.00".len());
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "September"
        );
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "biennial"
        );
    }

    #[test]
    fn a_catalog_form_opened_on_an_entry_prefills_every_field() {
        let form = RecurringGoalForm::edit(
            &Entry {
                id: RecurringGoalId(2),
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: true,
                cadence: Cadence::Biennial,
            },
            None,
        );
        assert_eq!(form.editing, Some(RecurringGoalId(2)));
        assert_eq!(
            form.display(RecurringGoalField::Name).plain_text(),
            "Dropbox"
        );
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "September"
        );
        assert_eq!(
            form.display(RecurringGoalField::Amount).plain_text(),
            "128.00"
        );
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "yes");
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "biennial"
        );
    }

    /// A recurring goal's base is what each round of it is worth, and every
    /// round is a whole dollar.
    #[test]
    fn a_base_with_cents_in_it_is_refused() {
        let mut form = RecurringGoalForm::add(None);
        for c in "Dropbox".chars() {
            form.edit(char_key(c));
        }
        // Month is a selector and takes whatever it opens on; the base is the
        // field under test.
        form.next_field();
        form.next_field();
        for c in "128.99".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("128.99"), "{err}");
    }

    #[test]
    fn an_empty_name_is_refused() {
        let mut form = RecurringGoalForm::add(None);
        form.next_field();
        form.next_field();
        for c in "128".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    #[test]
    fn the_month_selector_names_the_month_and_wraps_at_both_ends() {
        let mut form = RecurringGoalForm::add(None);
        walk_until!(form.focus == RecurringGoalField::Month, form.next_field());
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "January"
        );

        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "February"
        );

        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "December"
        );

        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "January"
        );
    }

    /// Typing at the Month selector must not reach another field's text.
    #[test]
    fn typing_at_the_month_selector_changes_nothing() {
        let mut form = RecurringGoalForm::add(None);
        for c in "Dropbox".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "13".chars() {
            form.edit(char_key(c));
        }
        form.edit(backspace_key());

        assert_eq!(
            form.display(RecurringGoalField::Name).plain_text(),
            "Dropbox"
        );
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "January"
        );
    }

    #[test]
    fn the_taxed_selector_cycles_both_ways() {
        let mut form = RecurringGoalForm::add(None);
        walk_until!(form.focus == RecurringGoalField::Taxed, form.next_field());
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "no");
        form.choice(Step::NEXT);
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "yes");
        form.choice(Step::NEXT);
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "no");
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "yes");
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "no");
    }

    #[test]
    fn the_cadence_selector_cycles_both_ways() {
        let mut form = RecurringGoalForm::add(None);
        walk_until!(form.focus == RecurringGoalField::Cadence, form.next_field());
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "annual"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "biennial"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "annual"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "biennial"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "annual"
        );
    }

    /// `←`/`→` on a text field must not silently change Month, Taxed or
    /// Cadence.
    /// One press each way, never two: Taxed is a bool and Cadence has two
    /// options, so a second press would cycle a broken one straight back to
    /// where it started and the assertion would pass either way.
    #[test]
    fn cycling_does_nothing_unless_a_selector_is_focused() {
        let mut form = RecurringGoalForm::add(None);
        assert_eq!(form.focus, RecurringGoalField::Name);

        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "January"
        );
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "no");
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "annual"
        );

        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringGoalField::Month).plain_text(),
            "January"
        );
        assert_eq!(form.display(RecurringGoalField::Taxed).plain_text(), "no");
        assert_eq!(
            form.display(RecurringGoalField::Cadence).plain_text(),
            "annual"
        );
    }

    fn drawn(list: &RecurringGoals) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), list);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..8)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// `Base` and `Open` are the right-aligned columns, so they are the two
    /// headers that must end where their figures do.
    #[test]
    fn the_right_aligned_headers_end_where_their_own_columns_do() {
        let lines = drawn(&screen());
        let header = super::super::ends_in_order(
            &lines[1],
            &["Name", "Month", "Base", "Taxed", "Cadence", "Open"],
        );
        let row = super::super::ends_in_order(
            &lines[2],
            &["Car Insurance", "Mar", "128.00", "no", "annual", "2"],
        );
        assert_eq!(header[2], row[2], "Base over {:?}", lines[2]);
        assert_eq!(header[5], row[5], "Open over {:?}", lines[2]);
    }
}
