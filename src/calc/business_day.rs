//! Business-day arithmetic: weekends only, no holiday calendar.

use anyhow::{Result, ensure};
use chrono::{Datelike, Days, NaiveDate, Weekday};

/// `date` advanced by `days` business days — Saturday and Sunday skipped.
///
/// Deliberately not a holiday calendar. One would need a data source and
/// yearly upkeep to save the owner a manual date edit a handful of times a
/// year, and the transfer date is editable on the confirm modal anyway.
///
/// Counting starts the day *after* `date`, so `days` of zero returns `date`
/// itself even on a weekend: the caller asked for no movement, and rounding
/// forward here would hide their bug rather than fix it.
pub fn add(date: NaiveDate, days: i64) -> Result<NaiveDate> {
    ensure!(
        days >= 0,
        "business days to add must not be negative, got {days}"
    );
    let mut at = date;
    let mut left = days;
    while left > 0 {
        at = at.checked_add_days(Days::new(1)).with_context_date(at)?;
        if !matches!(at.weekday(), Weekday::Sat | Weekday::Sun) {
            left -= 1;
        }
    }
    Ok(at)
}

/// `checked_add_days` returns `None` only past `NaiveDate`'s ceiling, which
/// no transfer date reaches. Named rather than `unwrap`ed so the failure
/// says which date it was on.
trait ContextDate: Sized {
    fn with_context_date(self, at: NaiveDate) -> Result<NaiveDate>;
}

impl ContextDate for Option<NaiveDate> {
    fn with_context_date(self, at: NaiveDate) -> Result<NaiveDate> {
        self.ok_or_else(|| anyhow::anyhow!("date overflow stepping past {at}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    /// Mid-week, no weekend crossed.
    #[test]
    fn two_business_days_from_a_monday_is_wednesday() {
        assert_eq!(add(day(2026, 8, 17), 2).unwrap(), day(2026, 8, 19));
    }

    /// The case the default date hits most often: Thursday's payday lands the
    /// transfer on Monday, not on Saturday.
    #[test]
    fn two_business_days_from_a_thursday_skips_the_weekend() {
        assert_eq!(add(day(2026, 8, 20), 2).unwrap(), day(2026, 8, 24));
    }

    #[test]
    fn two_business_days_from_a_friday_is_tuesday() {
        assert_eq!(add(day(2026, 8, 21), 2).unwrap(), day(2026, 8, 25));
    }

    /// Counting starts from the day after `date`, so a weekend start is not
    /// itself counted and the first business day found is the answer.
    #[test]
    fn one_business_day_from_a_saturday_is_monday() {
        assert_eq!(add(day(2026, 8, 22), 1).unwrap(), day(2026, 8, 24));
    }

    /// Zero steps is the identity, weekend or not: the caller asked for no
    /// movement, and rounding a Saturday forward to Monday here would hide
    /// the caller's own bug.
    #[test]
    fn zero_business_days_returns_the_date_unchanged() {
        assert_eq!(add(day(2026, 8, 22), 0).unwrap(), day(2026, 8, 22));
    }

    /// A negative count has no meaning for a transfer date and no caller, so
    /// it is refused rather than silently treated as zero.
    #[test]
    fn a_negative_count_is_refused() {
        let err = add(day(2026, 8, 17), -1).unwrap_err();
        assert!(err.to_string().contains("-1"), "{err}");
    }
}
