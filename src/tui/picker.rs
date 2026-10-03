//! The recurring goal as a multi-select list. Backs `s` on the Recurring Goals
//! screen.
//!
//! `Enter` creates every selected entry at once, which is what the annual
//! reseed of dozens of goals needs and still works for adding one.
//!
//! Each entry is listed beside the date its goal would take, and within the
//! ticked group and the rest alike the list runs in that date's order -- the
//! order the goals would fall due in, which is not the calendar's January to
//! December once a month has already passed this year.
//!
//! It opens with a caller-chosen set already ticked *and sorted to the top*,
//! which is what makes the reseed one keystroke; which set that is,
//! `App::open_recurring_goals` decides. A tick alone is easy to miss in a list
//! dozens long, so the two say the same thing twice: the entries about to be created
//! are the ones the list opens on. Neither is narrowing -- every entry is
//! listed either way -- so a preselection is a starting point the list can be
//! scrolled out of rather than a cage.

use super::cursor::{Cursor, Viewport, impl_scroll};
use super::{Account, Label};
use crate::db::recurring_goal::{Cadence, Entry};
use crate::db::{AccountId, RecurringGoalId};
use anyhow::{Context, Result};
use chrono::{Datelike, Months, NaiveDate};
use std::collections::{HashMap, HashSet};

/// The first of `month`, this year if `today` falls in or before that month
/// and next year otherwise. `None` only for a month outside 1-12, which the
/// schema's `CHECK` already refuses.
///
/// The month under way counts as this year's occurrence even once its first
/// is behind `today`: a reseed run on October 3rd is the one meant for this
/// October's round, and comparing against the first would push every entry
/// due this month a year further out than every entry due next month.
pub fn next_occurrence(month: u32, today: NaiveDate) -> Option<NaiveDate> {
    let year = if month >= today.month() {
        today.year()
    } else {
        today.year() + 1
    };
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// The goal date a new goal from `entry` takes.
///
/// Creating goals is a reseed for the year ahead, so both cadences start from
/// `next_occurrence` and land a year past it rather than on it. `Biennial`
/// steps two years instead when `has_goal_this_year`: every two years means
/// the year between is skipped rather than filled, and the entry has already
/// had this year's round. The workbook's "biannual" means every two years;
/// `Cadence::Biennial` already carries that translation.
///
/// Counting from the occurrence rather than from the calendar is what puts an
/// entry whose month has already passed two calendars out -- a March entry
/// reseeded in August 2026 is next-occurring in March 2027, so it lands in
/// 2028.
pub fn goal_date(entry: &Entry, has_goal_this_year: bool, today: NaiveDate) -> Result<NaiveDate> {
    let base = next_occurrence(entry.month as u32, today).with_context(|| {
        format!(
            "{:?} has an impossible month: {}",
            // The status line prints this verbatim, so the entry's name is
            // prose a viewer reads.
            crate::demo::text(&entry.name),
            entry.month
        )
    })?;
    let years = match (entry.cadence, has_goal_this_year) {
        (Cadence::Biennial, true) => 2,
        _ => 1,
    };
    base.checked_add_months(Months::new(12 * years))
        .with_context(|| {
            format!(
                "{:?}'s next goal date runs off the calendar",
                crate::demo::text(&entry.name)
            )
        })
}

pub struct Picker {
    entries: Vec<Entry>,
    /// Parallel to `entries`: the [`goal_date`] each would be created with,
    /// computed once by the caller so the date drawn is the date written.
    dates: Vec<NaiveDate>,
    open_counts: HashMap<RecurringGoalId, i64>,
    /// Parallel to `entries`, so the order the list shows is the order the
    /// goals are created in.
    selected: Vec<bool>,
    cursor: Cursor,
    container: Account,
}

impl Picker {
    pub fn new(
        mut entries: Vec<(Entry, NaiveDate)>,
        open_counts: HashMap<RecurringGoalId, i64>,
        preselected: &HashSet<RecurringGoalId>,
        container: Account,
    ) -> Picker {
        // Stable, so two entries due the same day keep the table's own order.
        entries.sort_by_key(|(_, date)| *date);
        // A stable partition, so the date order survives inside each group:
        // `selected` is built from the boundary rather than from a second
        // pass over `preselected`, which could not then disagree with the
        // order the entries ended up in.
        let (mut entries, rest): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|(e, _)| preselected.contains(&e.id));
        let boundary = entries.len();
        entries.extend(rest);
        let selected = (0..entries.len()).map(|i| i < boundary).collect();
        let (entries, dates) = entries.into_iter().unzip();
        Picker {
            entries,
            dates,
            open_counts,
            selected,
            cursor: Cursor::new(),
            container,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn container(&self) -> AccountId {
        self.container.id()
    }

    pub fn open_count(&self, id: RecurringGoalId) -> i64 {
        self.open_counts.get(&id).copied().unwrap_or(0)
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.get(index).copied().unwrap_or(false)
    }

    pub fn toggle(&mut self) {
        if let Some(selected) = self.selected.get_mut(self.cursor.index()) {
            *selected = !*selected;
        }
    }

    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|s| **s).count()
    }

    pub fn goal_date(&self, index: usize) -> Option<NaiveDate> {
        self.dates.get(index).copied()
    }

    /// Every ticked entry, beside the date its goal is created with.
    pub fn chosen(&self) -> Vec<(&Entry, NaiveDate)> {
        self.entries
            .iter()
            .zip(&self.dates)
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .map(|((entry, date), _)| (entry, *date))
            .collect()
    }

    pub fn title(&self) -> Label {
        Label::plain(format!(
            "Recurring goals — {} selected · Space toggles · Enter creates in ",
            self.selected_count()
        ))
        .account(self.container.clone())
        .text(" · Esc cancel")
    }
}

impl_scroll!(Picker, entries);

use super::widget::centered;
use super::{Chrome, amount, label_line, month_name, render_table};
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::widgets::{Block, Cell, Clear, Row};

/// One row per recurring goal entry, ticked where it is selected. Returns the
/// [`Viewport`] it drew: the height `PageUp`/`PageDown` move by, and the row
/// the next draw starts from.
pub(super) fn render(frame: &mut Frame, picker: &Picker) -> Viewport {
    // Wide enough for the whole title, which names the container the goals
    // land in -- the one thing on screen that is not otherwise visible.
    let area = centered(
        frame.area(),
        76,
        frame.area().height.saturating_sub(4).max(8),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::bordered().title(label_line(&picker.title())), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));

    let rows: Vec<Row> = picker
        .entries()
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let open = picker.open_count(entry.id);
            Row::new(vec![
                Cell::from(if picker.is_selected(i) { "✓" } else { " " }),
                Cell::from(crate::demo::text(&entry.name).into_owned()),
                // The goal's own date rather than the entry's bare month:
                // which year a round lands in is the one thing the catalog
                // cannot say and the reseed has to.
                Cell::from(match picker.goal_date(i) {
                    Some(date) => format!("{} {}", month_name(entry.month), date.year()),
                    None => month_name(entry.month),
                }),
                amount(entry.base_cents),
                Cell::from(entry.cadence.as_str()),
                Cell::from(if open > 0 {
                    format!("{open} open")
                } else {
                    String::new()
                }),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(2),
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(7),
    ];
    render_table(
        frame,
        inner,
        picker,
        Chrome::bare(),
        &widths,
        rows,
        picker.entries().len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self};
    use crate::money::Cents;
    use crate::test_support::{cash, day};
    use crate::tui::cursor::Scroll;

    fn accounts() -> Vec<account::Account> {
        vec![cash(1, "SAV"), cash(2, "NST")]
    }

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

    /// Each entry beside the date a reseed on 2026-08-16 would give it.
    fn dated(entries: Vec<Entry>) -> Vec<(Entry, NaiveDate)> {
        entries
            .into_iter()
            .map(|e| {
                let date = goal_date(&e, false, day(2026, 8, 16)).unwrap();
                (e, date)
            })
            .collect()
    }

    fn names(picker: &Picker) -> Vec<&str> {
        picker.entries().iter().map(|e| e.name.as_str()).collect()
    }

    /// The catalog's own order is insertion order, which says nothing about
    /// when anything is due. The list runs in the order the goals would fall
    /// due, so a month already past this year sorts after one still ahead.
    #[test]
    fn the_list_runs_in_the_order_the_goals_would_fall_due() {
        let picker = Picker::new(
            vec![
                (entry(1, "Lego", 12, Cadence::Annual), day(2027, 12, 1)),
                (
                    entry(2, "Car Insurance", 3, Cadence::Annual),
                    day(2028, 3, 1),
                ),
                (entry(3, "Dropbox", 9, Cadence::Annual), day(2027, 9, 1)),
            ],
            HashMap::new(),
            &HashSet::new(),
            Account::named(&accounts(), AccountId(1)),
        );
        assert_eq!(names(&picker), ["Dropbox", "Lego", "Car Insurance"]);
        assert_eq!(picker.goal_date(0), Some(day(2027, 9, 1)));
    }

    /// The ticked group still leads, and each group runs in date order.
    #[test]
    fn the_ticked_group_leads_and_each_group_runs_in_date_order() {
        let picker = Picker::new(
            vec![
                (entry(1, "Lego", 12, Cadence::Annual), day(2027, 12, 1)),
                (
                    entry(2, "Backblaze", 11, Cadence::Biennial),
                    day(2027, 11, 1),
                ),
                (
                    entry(3, "Car Insurance", 3, Cadence::Annual),
                    day(2028, 3, 1),
                ),
                (entry(4, "Dropbox", 9, Cadence::Annual), day(2027, 9, 1)),
            ],
            HashMap::new(),
            &HashSet::from([RecurringGoalId(1), RecurringGoalId(3)]),
            Account::named(&accounts(), AccountId(1)),
        );
        assert_eq!(
            names(&picker),
            ["Lego", "Car Insurance", "Dropbox", "Backblaze"]
        );
        let chosen: Vec<_> = picker
            .chosen()
            .into_iter()
            .map(|(e, date)| (e.name.as_str(), date))
            .collect();
        assert_eq!(
            chosen,
            [
                ("Lego", day(2027, 12, 1)),
                ("Car Insurance", day(2028, 3, 1))
            ],
            "each chosen entry carries the date it was drawn beside"
        );
    }

    /// The year is what the bare month cannot say: a month already past this
    /// year lands two calendars out, and the reseed has to show which.
    #[test]
    fn the_picker_draws_the_year_each_goal_lands_in() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let picker = Picker::new(
            dated(vec![
                entry(1, "Dropbox", 9, Cadence::Annual),
                entry(2, "Car Insurance", 3, Cadence::Annual),
            ]),
            HashMap::new(),
            &HashSet::new(),
            Account::named(&accounts(), AccountId(1)),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &picker);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Sep 2027"), "{text}");
        assert!(text.contains("Mar 2028"), "{text}");
    }

    #[test]
    fn the_next_occurrence_of_a_month_is_this_year_when_it_is_still_ahead() {
        assert_eq!(
            next_occurrence(12, day(2026, 8, 16)),
            Some(day(2026, 12, 1))
        );
        assert_eq!(
            next_occurrence(8, day(2026, 8, 1)),
            Some(day(2026, 8, 1)),
            "the first of this month counts on the first"
        );
        assert_eq!(
            next_occurrence(8, day(2026, 8, 16)),
            Some(day(2026, 8, 1)),
            "and still counts once the first is behind it: the month is not over"
        );
    }

    /// A month already past rolls into next year -- the case that makes March
    /// 2027 the next Car Insurance goal in August 2026.
    #[test]
    fn the_next_occurrence_of_a_month_already_past_crosses_the_year_boundary() {
        assert_eq!(next_occurrence(3, day(2026, 8, 16)), Some(day(2027, 3, 1)));
        assert_eq!(next_occurrence(7, day(2026, 8, 1)), Some(day(2027, 7, 1)));
    }

    /// A reseed is for the year ahead, so an annual entry lands a year past
    /// the occurrence that is next -- not on it.
    #[test]
    fn an_annual_entry_takes_the_year_after_its_next_occurrence() {
        let dropbox = entry(1, "Dropbox", 9, Cadence::Annual);
        assert_eq!(
            goal_date(&dropbox, false, day(2026, 8, 16)).unwrap(),
            day(2027, 9, 1)
        );
        // A goal already dated this year does not move an annual entry: every
        // year means every year.
        assert_eq!(
            goal_date(&dropbox, true, day(2026, 8, 16)).unwrap(),
            day(2027, 9, 1)
        );
    }

    /// The consequence of counting from the next occurrence rather than from
    /// the calendar: a month already past this year is next-occurring in 2027,
    /// so the year after it is 2028.
    #[test]
    fn an_annual_entry_whose_month_has_passed_lands_two_calendars_out() {
        let insurance = entry(3, "Car Insurance", 3, Cadence::Annual);
        assert_eq!(
            goal_date(&insurance, false, day(2026, 8, 16)).unwrap(),
            day(2028, 3, 1)
        );
    }

    /// The workbook's "biannual" means every two years, which is why
    /// `Cadence::Biennial` carries the translation. With no goal this year the
    /// entry is due, and lands where an annual one would.
    #[test]
    fn a_biennial_entry_with_no_goal_this_year_lands_a_year_out() {
        let backblaze = entry(2, "Backblaze", 11, Cadence::Biennial);
        assert_eq!(
            goal_date(&backblaze, false, day(2026, 8, 16)).unwrap(),
            day(2027, 11, 1)
        );
    }

    /// Every two years means the year between is skipped rather than filled,
    /// so an entry that has already had this year's round steps past next.
    #[test]
    fn a_biennial_entry_with_a_goal_this_year_skips_the_year_between() {
        let backblaze = entry(2, "Backblaze", 11, Cadence::Biennial);
        assert_eq!(
            goal_date(&backblaze, true, day(2026, 8, 16)).unwrap(),
            day(2028, 11, 1),
            "two years past the goal it already has"
        );
    }

    /// December is the month the year rolls over on, and the one the reseed is
    /// most likely to be run near.
    #[test]
    fn a_december_entry_lands_in_the_december_after_the_next_one() {
        let lego = entry(4, "Lego", 12, Cadence::Annual);
        assert_eq!(
            goal_date(&lego, false, day(2026, 8, 16)).unwrap(),
            day(2027, 12, 1)
        );
        assert_eq!(
            goal_date(&lego, false, day(2026, 12, 2)).unwrap(),
            day(2027, 12, 1),
            "December 2026 is still under way, so the reseed is for 2027"
        );
    }

    /// A recurring goal's name is the owner's own word for it, and the
    /// picker is the one place the catalog is drawn -- no other test renders
    /// it, so nothing else would notice the mask going away.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_recurring_goals_name_in_the_picker() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
        let picker = Picker::new(
            dated(vec![entry(1, "Dropbox", 9, Cadence::Annual)]),
            HashMap::new(),
            &HashSet::new(),
            Account::named(&accounts(), AccountId(2)),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &picker);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!text.contains("Dropbox"), "the entry name survived: {text}");
        assert!(
            text.contains(&crate::demo::text("Dropbox").to_string()),
            "no scrambled entry name found: {text}"
        );
        // The title is the only thing on screen naming the container the
        // goals land in, and it reaches the mask through `Account` rather
        // than through the cell above.
        assert!(!text.contains("Nest Egg"), "the container survived: {text}");
        assert!(
            text.contains(&crate::demo::text("Nest Egg").to_string()),
            "no scrambled container found: {text}"
        );
        // The month and the cadence are the app's own words, and stay.
        assert!(text.contains("Sep"), "the month must stay: {text}");
    }

    #[test]
    fn space_toggles_and_enter_creates_every_selected_entry() {
        let entries = vec![
            entry(1, "Dropbox", 9, Cadence::Annual),
            entry(2, "Backblaze", 11, Cadence::Biennial),
            entry(3, "Lego", 12, Cadence::Annual),
        ];
        let mut picker = Picker::new(
            dated(entries),
            HashMap::new(),
            &HashSet::new(),
            Account::named(&accounts(), AccountId(1)),
        );
        assert_eq!(picker.selected_count(), 0);

        picker.toggle();
        picker.select_next();
        picker.select_next();
        picker.toggle();

        assert_eq!(picker.selected_count(), 2);
        let chosen: Vec<&str> = picker
            .chosen()
            .iter()
            .map(|(e, _)| e.name.as_str())
            .collect();
        assert_eq!(chosen, ["Dropbox", "Lego"]);

        picker.toggle();
        assert_eq!(picker.selected_count(), 1, "Space toggles both ways");
    }

    #[test]
    fn the_open_column_counts_a_catalog_entrys_existing_goals() {
        let entries = vec![entry(1, "Dropbox", 9, Cadence::Annual)];
        let counts = HashMap::from([(RecurringGoalId(1), 2)]);
        let picker = Picker::new(
            dated(entries),
            counts,
            &HashSet::new(),
            Account::named(&accounts(), AccountId(1)),
        );
        assert_eq!(picker.open_count(RecurringGoalId(1)), 2);
        assert_eq!(picker.open_count(RecurringGoalId(9)), 0);
    }

    /// The picker's title is the only thing on screen naming the container
    /// its goals will be created in, which is why the title carries a
    /// container at all -- so it is the one word on the line worth a color.
    #[test]
    fn the_picker_title_names_the_container_it_creates_in() {
        let picker = Picker::new(
            dated(vec![entry(1, "Dropbox", 9, Cadence::Annual)]),
            HashMap::new(),
            &HashSet::new(),
            Account::named(&accounts(), AccountId(2)),
        );
        let title = picker.title();
        assert!(
            title.plain_text().contains("creates in Nest Egg"),
            "{}",
            title.plain_text()
        );
        assert_eq!(title.accounts().len(), 1);
        assert_eq!(title.accounts()[0].id(), AccountId(2));
    }
}
