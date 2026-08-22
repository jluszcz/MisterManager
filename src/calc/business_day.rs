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

/// `date` moved back by `days` business days -- [`add`]'s mirror, counting
/// the same way: from the day *before* `date`, so zero is `date` itself even
/// on a weekend.
fn sub(date: NaiveDate, days: i64) -> Result<NaiveDate> {
    ensure!(
        days >= 0,
        "business days to subtract must not be negative, got {days}"
    );
    let mut at = date;
    let mut left = days;
    while left > 0 {
        at = at.checked_sub_days(Days::new(1)).with_context_date(at)?;
        if !matches!(at.weekday(), Weekday::Sat | Weekday::Sun) {
            left -= 1;
        }
    }
    Ok(at)
}

/// The `before` business days up to `date` and the `after` business days past
/// it, in order, with `date` itself between them.
///
/// `date` is included whatever its weekday, for [`add`]'s reason: the caller
/// is asking about a date it already holds, and rounding it onto a business
/// day here would answer a question nobody asked.
pub fn window(date: NaiveDate, before: i64, after: i64) -> Result<Vec<NaiveDate>> {
    let mut out = Vec::new();
    for n in (1..=before).rev() {
        out.push(sub(date, n)?);
    }
    out.push(date);
    for n in 1..=after {
        out.push(add(date, n)?);
    }
    Ok(out)
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

    /// The window a duplicate check scans: business days either side, with
    /// the date itself between them and the weekend skipped in both
    /// directions.
    #[test]
    fn a_window_spans_business_days_either_side_of_its_date() {
        // Wednesday, no weekend crossed.
        assert_eq!(
            window(day(2026, 8, 19), 2, 2).unwrap(),
            vec![
                day(2026, 8, 17),
                day(2026, 8, 18),
                day(2026, 8, 19),
                day(2026, 8, 20),
                day(2026, 8, 21),
            ]
        );
        // Monday: the two days behind it are Friday and Thursday.
        assert_eq!(
            window(day(2026, 8, 17), 2, 2).unwrap(),
            vec![
                day(2026, 8, 13),
                day(2026, 8, 14),
                day(2026, 8, 17),
                day(2026, 8, 18),
                day(2026, 8, 19),
            ]
        );
    }

    /// A window of no width is the date it was asked about, the same as
    /// `add`'s zero -- and a weekend date is returned as itself rather than
    /// rounded onto a business day.
    #[test]
    fn a_window_of_no_width_is_the_date_itself() {
        assert_eq!(
            window(day(2026, 8, 15), 0, 0).unwrap(),
            vec![day(2026, 8, 15)]
        );
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
