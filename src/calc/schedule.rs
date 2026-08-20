//! When a recurring thing happens.
//!
//! Pure date arithmetic, and deliberately no persisted vocabulary: `calc`
//! never learns that `biweekly` is a string in a column.
//! `db::recurring_txn::Cadence` maps onto [`Step`], and that match lives in
//! `src/recurring_txn.rs`, where policy belongs.

use anyhow::{Result, ensure};
use chrono::{Days, Months, NaiveDate};

/// The gap between two occurrences.
///
/// Two shapes rather than one count of days, because a monthly recurring
/// transaction is not 30 days: the mortgage is due on the 1st whatever
/// February does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Days(u32),
    Months(u32),
}

impl Step {
    fn is_positive(self) -> bool {
        match self {
            Step::Days(d) => d > 0,
            Step::Months(m) => m > 0,
        }
    }

    /// The `n`th occurrence after the anchor, counting the anchor as zero.
    ///
    /// **Always from the anchor, never from the previous result.**
    /// `checked_add_months` clamps, so an anchor on the 31st gives Feb 28 --
    /// and stepping from that clamped date would leave the recurring
    /// transaction permanently on the 28th. `anchor + n months` returns it to
    /// Mar 31.
    ///
    /// `None` where the arithmetic runs off the end of the calendar.
    fn advance(self, anchor: NaiveDate, n: u32) -> Option<NaiveDate> {
        match self {
            Step::Days(d) => anchor.checked_add_days(Days::new(u64::from(d) * u64::from(n))),
            Step::Months(m) => anchor.checked_add_months(Months::new(m.checked_mul(n)?)),
        }
    }
}

/// Every occurrence of the schedule that falls in `from ..= to`, in order.
///
/// A zero step is an error rather than an empty result: it would otherwise
/// mean "every date at once", and the loop below would never terminate.
pub fn occurrences(
    anchor: NaiveDate,
    step: Step,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<NaiveDate>> {
    ensure!(
        step.is_positive(),
        "a schedule step must be positive, got {step:?}"
    );
    let mut found = Vec::new();
    let mut n = 0u32;
    while let Some(date) = step.advance(anchor, n) {
        if date > to {
            break;
        }
        if date >= from {
            found.push(date);
        }
        n += 1;
    }
    Ok(found)
}

/// The first occurrence strictly after `after`.
///
/// Strictly: on payday itself the paycheck has already landed, and the runway
/// that matters is the one to the next one. `None` for a zero step, or where
/// the arithmetic runs off the end of the calendar.
pub fn next_after(anchor: NaiveDate, step: Step, after: NaiveDate) -> Option<NaiveDate> {
    if !step.is_positive() {
        return None;
    }
    let mut n = 0u32;
    loop {
        let date = step.advance(anchor, n)?;
        if date > after {
            return Some(date);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A biweekly paycheck: 14-day gaps, anchored on an occurrence that
    /// falls inside the window rather than at its start.
    #[test]
    fn a_biweekly_run_hits_every_fourteenth_day_of_the_window() {
        let got = occurrences(
            day(2026, 8, 28),
            Step::Days(14),
            day(2026, 8, 16),
            day(2026, 12, 31),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                day(2026, 8, 28),
                day(2026, 9, 11),
                day(2026, 9, 25),
                day(2026, 10, 9),
                day(2026, 10, 23),
                day(2026, 11, 6),
                day(2026, 11, 20),
                day(2026, 12, 4),
                day(2026, 12, 18),
            ]
        );
    }

    /// Mortgage, HOA, and Phone: the first of every month.
    #[test]
    fn a_monthly_run_hits_the_anchors_day_of_each_month() {
        let got = occurrences(
            day(2026, 9, 1),
            Step::Months(1),
            day(2026, 8, 16),
            day(2026, 12, 31),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                day(2026, 9, 1),
                day(2026, 10, 1),
                day(2026, 11, 1),
                day(2026, 12, 1),
            ]
        );
    }

    /// `checked_add_months` clamps, so an anchor on the 31st gives Feb 28.
    /// Stepping from *that* would leave the recurring transaction permanently
    /// on the 28th; stepping `anchor + n months` returns it to Mar 31.
    #[test]
    fn a_month_end_anchor_clamps_in_february_and_comes_back_afterwards() {
        let got = occurrences(
            day(2026, 1, 31),
            Step::Months(1),
            day(2026, 1, 1),
            day(2026, 4, 30),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                day(2026, 1, 31),
                day(2026, 2, 28),
                day(2026, 3, 31),
                day(2026, 4, 30),
            ]
        );
    }

    #[test]
    fn occurrences_before_the_window_are_skipped_and_the_anchor_is_not_special() {
        let got = occurrences(
            day(2026, 1, 1),
            Step::Days(14),
            day(2026, 2, 1),
            day(2026, 2, 28),
        )
        .unwrap();
        assert_eq!(got, vec![day(2026, 2, 12), day(2026, 2, 26)]);
    }

    #[test]
    fn a_window_that_ends_before_the_anchor_is_empty() {
        let got = occurrences(
            day(2026, 8, 28),
            Step::Days(14),
            day(2026, 1, 1),
            day(2026, 6, 30),
        )
        .unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn a_window_of_one_day_that_lands_on_an_occurrence_holds_exactly_it() {
        let got = occurrences(
            day(2026, 8, 28),
            Step::Days(14),
            day(2026, 9, 11),
            day(2026, 9, 11),
        )
        .unwrap();
        assert_eq!(got, vec![day(2026, 9, 11)]);
    }

    /// The next paycheck, from the day the app is asked. Anchored ahead of
    /// today, the anchor itself is the answer.
    #[test]
    fn next_after_returns_the_first_occurrence_beyond_the_date_given() {
        assert_eq!(
            next_after(day(2026, 8, 28), Step::Days(14), day(2026, 8, 16)),
            Some(day(2026, 8, 28))
        );
        assert_eq!(
            next_after(day(2026, 8, 28), Step::Days(14), day(2026, 9, 1)),
            Some(day(2026, 9, 11))
        );
    }

    /// On payday itself the paycheck has already landed, and the runway that
    /// matters is to the next one -- which is what `Overview!E2` held on
    /// 2026-08-14 against an 08-28 anchor.
    #[test]
    fn next_after_on_an_occurrence_itself_returns_the_one_after_it() {
        assert_eq!(
            next_after(day(2026, 8, 28), Step::Days(14), day(2026, 8, 28)),
            Some(day(2026, 9, 11))
        );
    }

    #[test]
    fn next_after_a_date_before_the_anchor_is_the_anchor() {
        assert_eq!(
            next_after(day(2026, 8, 28), Step::Months(1), day(2020, 1, 1)),
            Some(day(2026, 8, 28))
        );
    }

    /// `Cadence` cannot produce a zero step, but `Step` is public and a zero
    /// would loop forever rather than fail.
    #[test]
    fn a_zero_step_is_refused_rather_than_looping() {
        assert!(
            occurrences(
                day(2026, 1, 1),
                Step::Days(0),
                day(2026, 1, 1),
                day(2026, 2, 1)
            )
            .is_err()
        );
        assert!(
            occurrences(
                day(2026, 1, 1),
                Step::Months(0),
                day(2026, 1, 1),
                day(2026, 2, 1)
            )
            .is_err()
        );
        assert_eq!(
            next_after(day(2026, 1, 1), Step::Days(0), day(2026, 1, 1)),
            None
        );
    }
}
