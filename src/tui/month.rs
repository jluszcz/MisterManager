//! The `[` / `]` month filter every scrubbing screen shares.
//!
//! Two screens narrow a list to one month and offer an All that shows every
//! row: Savings, over a goal's `goal_date`, and Recurring Goals, over an
//! entry's month of the year. They differ only in what a month *is* -- a real
//! year-month on one, a bare 1-12 on the other -- so the state machine is
//! shared and the domain is the type parameter.
//!
//! The ledgers deliberately do not use this. Their window is a one-or-two
//! month span clamped to the data's range and pushed down into the SQL, with
//! no All to return to; see [`super::ledger::Window`].

use chrono::{Datelike, NaiveDate};

/// One calendar month, as Savings filters by: a `goal_date` carries a year,
/// so December 2026 and December 2027 are different filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct YearMonth {
    year: i32,
    month: u32,
}

impl YearMonth {
    /// The month a date falls in.
    pub fn of(date: NaiveDate) -> YearMonth {
        YearMonth {
            year: date.year(),
            month: date.month(),
        }
    }

    /// Whether a date falls in this month.
    pub fn contains(&self, date: NaiveDate) -> bool {
        YearMonth::of(date) == *self
    }

    /// How the month reads in a title bar: `Sep 2026`.
    pub fn label(&self) -> String {
        NaiveDate::from_ymd_opt(self.year, self.month, 1)
            .expect("a YearMonth only ever holds a month of the year")
            .format("%b %Y")
            .to_string()
    }

    /// Every month from `first` to `last` inclusive, in order.
    ///
    /// Empty months are included, the same as Recurring Goals stepping a
    /// calendar where most months hold nothing: the cycle is the span, not
    /// the set of months that happen to have rows.
    pub fn range(first: YearMonth, last: YearMonth) -> Vec<YearMonth> {
        let mut months = Vec::new();
        let mut current = first;
        while current <= last {
            months.push(current);
            current = current.succ();
        }
        months
    }

    fn succ(&self) -> YearMonth {
        match self.month {
            12 => YearMonth {
                year: self.year + 1,
                month: 1,
            },
            m => YearMonth {
                year: self.year,
                month: m + 1,
            },
        }
    }
}

/// The months a screen's `[` and `]` cycle through, and which of them -- if
/// any -- is selected.
///
/// `entry` is where a step leaves All from, which is today's month. It must be
/// one of `months`; a caller whose cycle does not span today clamps before
/// constructing. An empty cycle cannot be stepped out of All at all.
#[derive(Debug)]
pub struct MonthCycle<M> {
    months: Vec<M>,
    entry: M,
    /// `None` is the All filter.
    selected: Option<M>,
}

impl<M: Copy + PartialEq> MonthCycle<M> {
    /// Opens on All. Both screens are lists worth seeing whole, and `[` and
    /// `]` are what start narrowing them.
    pub fn new(months: Vec<M>, entry: M) -> MonthCycle<M> {
        MonthCycle {
            months,
            entry,
            selected: None,
        }
    }

    /// Take a rebuilt cycle, for a screen whose months come from its data.
    ///
    /// Keeps the selected month when the new cycle still holds it, and falls
    /// back to All when it does not: the selection is stored as the month
    /// itself rather than as a position precisely so a cycle that changed
    /// length underneath it cannot silently mean a different month.
    pub fn set_months(&mut self, months: Vec<M>, entry: M) {
        self.selected = self.selected.filter(|m| months.contains(m));
        self.months = months;
        self.entry = entry;
    }

    /// `]`: forward one month, the last wrapping to the first.
    pub fn next(&mut self) {
        self.step(1);
    }

    /// `[`: back one month, the first wrapping to the last.
    pub fn previous(&mut self) {
        self.step(-1);
    }

    /// Stepping out of All lands on `entry` whichever direction it is stepped,
    /// so `Esc` then `[` reaches the same place as `Esc` then `]` and no state
    /// crosses the All filter.
    fn step(&mut self, delta: isize) {
        if self.months.is_empty() {
            return;
        }
        let position = self
            .selected
            .and_then(|m| self.months.iter().position(|x| *x == m));
        self.selected = Some(match position {
            None => self.entry,
            Some(i) => {
                let len = self.months.len() as isize;
                self.months[(i as isize + delta).rem_euclid(len) as usize]
            }
        });
    }

    /// `Esc`: show every row again.
    pub fn clear(&mut self) {
        self.selected = None;
    }

    pub fn selected(&self) -> Option<M> {
        self.selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ym(year: i32, month: u32) -> YearMonth {
        YearMonth { year, month }
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn a_year_month_contains_every_day_of_its_own_month() {
        assert!(ym(2026, 8).contains(date(2026, 8, 1)));
        assert!(ym(2026, 8).contains(date(2026, 8, 31)));
    }

    #[test]
    fn a_year_month_excludes_the_same_month_of_another_year() {
        assert!(!ym(2026, 8).contains(date(2027, 8, 12)));
    }

    #[test]
    fn a_range_runs_month_by_month_across_a_year_boundary() {
        assert_eq!(
            YearMonth::range(ym(2026, 11), ym(2027, 2)),
            vec![ym(2026, 11), ym(2026, 12), ym(2027, 1), ym(2027, 2)],
        );
    }

    #[test]
    fn a_range_of_one_month_is_that_month() {
        assert_eq!(
            YearMonth::range(ym(2026, 8), ym(2026, 8)),
            vec![ym(2026, 8)]
        );
    }

    #[test]
    fn a_range_whose_ends_are_backwards_is_empty() {
        assert!(YearMonth::range(ym(2027, 2), ym(2026, 11)).is_empty());
    }

    #[test]
    fn a_year_month_labels_itself_with_its_year() {
        assert_eq!(ym(2026, 9).label(), "Sep 2026");
    }

    fn cycle() -> MonthCycle<i64> {
        MonthCycle::new((1..=12).collect(), 8)
    }

    #[test]
    fn a_cycle_opens_on_all() {
        assert_eq!(cycle().selected(), None);
    }

    #[test]
    fn stepping_forward_out_of_all_lands_on_the_entry_month() {
        let mut c = cycle();
        c.next();
        assert_eq!(c.selected(), Some(8));
    }

    #[test]
    fn stepping_back_out_of_all_lands_on_the_same_entry_month() {
        let mut c = cycle();
        c.previous();
        assert_eq!(c.selected(), Some(8));
    }

    #[test]
    fn stepping_past_the_last_month_wraps_to_the_first() {
        let mut c = MonthCycle::new((1..=12).collect(), 12);
        c.next();
        c.next();
        assert_eq!(c.selected(), Some(1));
    }

    #[test]
    fn stepping_before_the_first_month_wraps_to_the_last() {
        let mut c = MonthCycle::new((1..=12).collect(), 1);
        c.next();
        c.previous();
        assert_eq!(c.selected(), Some(12));
    }

    #[test]
    fn clearing_returns_to_all() {
        let mut c = cycle();
        c.next();
        c.clear();
        assert_eq!(c.selected(), None);
    }

    #[test]
    fn a_step_after_clearing_re_enters_at_the_entry_not_the_month_left_behind() {
        let mut c = cycle();
        c.next();
        c.next();
        c.clear();
        c.next();
        assert_eq!(c.selected(), Some(8));
    }

    #[test]
    fn a_rebuilt_cycle_keeps_a_month_it_still_holds() {
        let mut c = MonthCycle::new(vec![ym(2026, 8), ym(2026, 9)], ym(2026, 8));
        c.next();
        c.next();
        c.set_months(vec![ym(2026, 8), ym(2026, 9), ym(2026, 10)], ym(2026, 8));
        assert_eq!(c.selected(), Some(ym(2026, 9)));
    }

    #[test]
    fn a_rebuilt_cycle_that_lost_the_month_falls_back_to_all() {
        let mut c = MonthCycle::new(vec![ym(2026, 8), ym(2026, 9)], ym(2026, 8));
        c.next();
        c.next();
        c.set_months(vec![ym(2026, 8)], ym(2026, 8));
        assert_eq!(c.selected(), None);
    }

    #[test]
    fn an_empty_cycle_cannot_be_stepped_out_of_all() {
        let mut c: MonthCycle<YearMonth> = MonthCycle::new(Vec::new(), ym(2026, 8));
        c.next();
        c.previous();
        assert_eq!(c.selected(), None);
    }
}
