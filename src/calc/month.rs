//! A calendar month, named by its first day.
//!
//! The field is private and [`Month::of`] is the only way in, so a `Month`
//! holding the 15th cannot be built: what `balance_snapshot`'s `CHECK`
//! refuses, the type has already ruled out. The ledger's window steps by
//! these for the same reason -- `[` from January 31 lands on a month rather
//! than on a February 31 that has to be clamped back.

use chrono::{Datelike, Months, NaiveDate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month(NaiveDate);

impl Month {
    /// The month `date` falls in.
    pub fn of(date: NaiveDate) -> Month {
        Month(date.with_day(1).expect("every month has a first day"))
    }

    pub fn first_day(self) -> NaiveDate {
        self.0
    }

    /// The 31st, the 30th, or the 28th or 29th.
    pub fn last_day(self) -> NaiveDate {
        self.shifted(1)
            .0
            .pred_opt()
            .expect("the day before a first-of-month is representable")
    }

    /// `months` later, or earlier when negative.
    pub fn shifted(self, months: i32) -> Month {
        let shifted = if months >= 0 {
            self.0.checked_add_months(Months::new(months as u32))
        } else {
            self.0
                .checked_sub_months(Months::new(months.unsigned_abs()))
        };
        Month(shifted.expect("ledger dates are nowhere near the ends of the calendar"))
    }

    pub fn next(self) -> Month {
        self.shifted(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn a_month_is_named_by_its_first_day_whatever_day_built_it() {
        assert_eq!(Month::of(day(2026, 8, 31)).first_day(), day(2026, 8, 1));
    }

    #[test]
    fn each_month_ends_on_its_own_last_day() {
        assert_eq!(Month::of(day(2026, 10, 4)).last_day(), day(2026, 10, 31));
        assert_eq!(Month::of(day(2026, 9, 4)).last_day(), day(2026, 9, 30));
        assert_eq!(Month::of(day(2026, 2, 4)).last_day(), day(2026, 2, 28));
        assert_eq!(Month::of(day(2028, 2, 4)).last_day(), day(2028, 2, 29));
    }

    #[test]
    fn shifting_crosses_a_year_in_both_directions() {
        let december = Month::of(day(2026, 12, 9));
        assert_eq!(december.next().first_day(), day(2027, 1, 1));
        assert_eq!(december.shifted(-12).first_day(), day(2025, 12, 1));
    }
}
