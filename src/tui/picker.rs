//! The recurring goal as a multi-select list. Backs `s` on the Recurring Goals
//! screen.
//!
//! `Enter` creates every selected entry at once, which is what the annual
//! reseed of dozens of goals needs and still works for adding one.
//!
//! It opens with a caller-chosen set already ticked *and sorted to the top*,
//! which is what makes the reseed one keystroke; which set that is,
//! `App::open_recurring_goals` decides. A tick alone is easy to miss in a list
//! dozens long, so the two say the same thing twice: the entries about to be created
//! are the ones the list opens on. Neither is narrowing -- every entry is
//! listed either way -- so a preselection is a starting point the list can be
//! scrolled out of rather than a cage.

use super::cursor::{Cursor, Scroll, Viewport};
use super::{Account, Label};
use crate::db::recurring_goal::{Cadence, Entry};
use crate::db::{AccountId, RecurringGoalId};
use anyhow::{Context, Result};
use chrono::{Datelike, Months, NaiveDate};
use std::collections::{HashMap, HashSet};

/// The first of `month`, in the first year where that lands on or after
/// `today`. `None` only for a month outside 1-12, which the schema's `CHECK`
/// already refuses.
pub fn next_occurrence(month: u32, today: NaiveDate) -> Option<NaiveDate> {
    let this_year = NaiveDate::from_ymd_opt(today.year(), month, 1)?;
    if this_year >= today {
        Some(this_year)
    } else {
        NaiveDate::from_ymd_opt(today.year() + 1, month, 1)
    }
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
    let base = next_occurrence(entry.month as u32, today)
        .with_context(|| format!("{:?} has an impossible month: {}", entry.name, entry.month))?;
    let years = match (entry.cadence, has_goal_this_year) {
        (Cadence::Biennial, true) => 2,
        _ => 1,
    };
    base.checked_add_months(Months::new(12 * years))
        .with_context(|| format!("{:?}'s next goal date runs off the calendar", entry.name))
}

pub struct Picker {
    entries: Vec<Entry>,
    open_counts: HashMap<RecurringGoalId, i64>,
    /// Parallel to `entries`, so the order the list shows is the order the
    /// goals are created in.
    selected: Vec<bool>,
    cursor: Cursor,
    container: Account,
}

impl Picker {
    pub fn new(
        entries: Vec<Entry>,
        open_counts: HashMap<RecurringGoalId, i64>,
        preselected: &HashSet<RecurringGoalId>,
        container: Account,
    ) -> Picker {
        // A stable partition, so the table's own order survives inside each
        // group: `selected` is built from the boundary rather than from a
        // second pass over `preselected`, which could not then disagree with
        // the order the entries ended up in.
        let (mut entries, rest): (Vec<Entry>, Vec<Entry>) = entries
            .into_iter()
            .partition(|e| preselected.contains(&e.id));
        let boundary = entries.len();
        entries.extend(rest);
        let selected = (0..entries.len()).map(|i| i < boundary).collect();
        Picker {
            entries,
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

    pub fn chosen(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .map(|(entry, _)| entry)
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

impl Scroll for Picker {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    fn row_count(&self) -> usize {
        self.entries.len()
    }
}

use super::form::centered;
use super::{amount, label_line, month_name, table_state};
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Cell, Clear, Row, Table};

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
                Cell::from(entry.name.clone()),
                Cell::from(month_name(entry.month)),
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
        Constraint::Length(4),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(7),
    ];
    let height = usize::from(inner.height);
    let (mut state, viewport) = table_state(picker, picker.entries().len(), height);
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> "),
        inner,
        &mut state,
    );

    viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self};
    use crate::money::Cents;
    use crate::test_support::{cash, day};

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

    #[test]
    fn the_next_occurrence_of_a_month_is_this_year_when_it_is_still_ahead() {
        assert_eq!(
            next_occurrence(12, day(2026, 8, 16)),
            Some(day(2026, 12, 1))
        );
        assert_eq!(
            next_occurrence(8, day(2026, 8, 1)),
            Some(day(2026, 8, 1)),
            "at or after today, so the first of this month counts"
        );
    }

    /// A month already past rolls into next year -- the case that makes March
    /// 2027 the next Car Insurance goal in August 2026.
    #[test]
    fn the_next_occurrence_of_a_month_already_past_crosses_the_year_boundary() {
        assert_eq!(next_occurrence(3, day(2026, 8, 16)), Some(day(2027, 3, 1)));
        assert_eq!(next_occurrence(8, day(2026, 8, 16)), Some(day(2027, 8, 1)));
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
            day(2028, 12, 1),
            "the first has passed, so the next occurrence is already 2027"
        );
    }

    #[test]
    fn space_toggles_and_enter_creates_every_selected_entry() {
        let entries = vec![
            entry(1, "Dropbox", 9, Cadence::Annual),
            entry(2, "Backblaze", 11, Cadence::Biennial),
            entry(3, "Lego", 12, Cadence::Annual),
        ];
        let mut picker = Picker::new(
            entries,
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
        let chosen: Vec<&str> = picker.chosen().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(chosen, ["Dropbox", "Lego"]);

        picker.toggle();
        assert_eq!(picker.selected_count(), 1, "Space toggles both ways");
    }

    #[test]
    fn the_open_column_counts_a_catalog_entrys_existing_goals() {
        let entries = vec![entry(1, "Dropbox", 9, Cadence::Annual)];
        let counts = HashMap::from([(RecurringGoalId(1), 2)]);
        let picker = Picker::new(
            entries,
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
            vec![entry(1, "Dropbox", 9, Cadence::Annual)],
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
