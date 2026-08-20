use crate::money::Cents;
use anyhow::Result;
use chrono::{Datelike, NaiveDate};

/// The workbook's `Biweekly()` lambda: a monthly amount spread over the
/// year's pay periods, rounded up to a whole dollar.
///
/// ```text
/// CEILING.MATH(monthly * 12 / periods, 1)
/// ```
pub fn biweekly(monthly: Cents, periods_per_year: i64) -> Result<Cents> {
    // A user-editable setting, so a zero here is reachable. Clamped rather
    // than rejected: a nonsense pay-period count should not take down the
    // whole Planning screen, the same call `mul_frac` makes for its
    // denominator. The clamp is what keeps `div_ceil`'s error unreachable
    // from here.
    let periods_per_year = periods_per_year.max(1);
    let num = monthly.0 * 12;
    let step = periods_per_year * 100;
    Ok(Cents(super::div_ceil(num, step)? * 100))
}

/// The workbook's `PerPaycheck()` lambda: what to set aside each payday to
/// reach `goal` by `by`, rounded up to a whole dollar.
///
/// ```text
/// IF(OR(ISBLANK(by), cur >= goal), "",
///    CEILING((goal - cur) / MAX(1, CEILING((by - today) / period, 1)), 1))
/// ```
///
/// Returns `None` where the sheet shows blank: undated goals, and goals
/// already at or past their target.
pub fn per_paycheck(
    current: Cents,
    goal: Cents,
    by: Option<NaiveDate>,
    today: NaiveDate,
    period_days: i64,
) -> Result<Option<Cents>> {
    let Some(by) = by else { return Ok(None) };
    if current >= goal {
        return Ok(None);
    }
    // `period_days` comes straight from `Constants!H2`; a zero there must not
    // reach `div_ceil`'s divide, matching `biweekly`'s clamp above.
    let period_days = period_days.max(1);
    let days = (by - today).num_days();
    let periods = super::div_ceil(days, period_days)?.max(1);
    let remaining = goal.0 - current.0;
    Ok(Some(Cents(
        super::div_ceil(remaining, periods * 100)? * 100,
    )))
}

/// Fit a set of per-paycheck asks into the money there is.
///
/// Under-subscribed, every ask is met in full and the difference is left over
/// -- deliberately, because that remainder is money the owner places by hand
/// rather than money the prefill should find a home for. Over-subscribed,
/// `pro_rata` divides the pot by the asks, which scales every goal to the
/// same fraction of what it wanted and leaves nothing over.
///
/// Weighting by the ask rather than by the raw shortfall is what makes a
/// deadline count: a goal due next paycheck asks for all of what it lacks,
/// while one due in three years asks for a thirtieth of a larger number.
///
/// A zero ask stays zero in both branches -- an undated goal has no runway to
/// divide and a met one needs nothing, and neither should be handed money
/// just because there is some.
pub fn fit(pot: Cents, asks: &[(i64, Cents)]) -> Result<Vec<(i64, Cents)>> {
    let total: i64 = asks.iter().map(|(_, c)| c.0).sum();
    if total <= pot.0 {
        return Ok(asks.to_vec());
    }
    super::pro_rata(pot, asks)
}

/// `EOMONTH(today, 0) + 1` — the first day of the following month.
pub fn month_end_projection(today: NaiveDate) -> NaiveDate {
    let (year, month) = (today.year(), today.month());
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).expect("first of month is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Cents;
    use chrono::NaiveDate;

    fn asks(v: &[(i64, i64)]) -> Vec<(i64, Cents)> {
        v.iter()
            .map(|(id, d)| (*id, Cents::from_dollars(*d)))
            .collect()
    }

    /// The behaviour the owner asked for: take what the goals ask, and leave
    /// the rest alone rather than finding a home for it.
    #[test]
    fn asks_that_fit_are_met_in_full_and_leave_the_rest() {
        let out = fit(Cents::from_dollars(1_000), &asks(&[(1, 300), (2, 200)])).unwrap();
        assert_eq!(out, asks(&[(1, 300), (2, 200)]));
        let allocated: i64 = out.iter().map(|(_, c)| c.0).sum();
        assert_eq!(Cents::from_dollars(1_000).0 - allocated, 50_000);
    }

    /// Exactly enough is still "fits": nothing is scaled and nothing is left.
    #[test]
    fn asks_that_exactly_exhaust_the_pot_are_met_in_full() {
        let out = fit(Cents::from_dollars(500), &asks(&[(1, 300), (2, 200)])).unwrap();
        assert_eq!(out, asks(&[(1, 300), (2, 200)]));
    }

    /// Over-subscribed, everyone takes the same haircut and the pot is spent
    /// to the dollar.
    #[test]
    fn asks_that_do_not_fit_are_scaled_to_the_pot() {
        let out = fit(Cents::from_dollars(500), &asks(&[(1, 750), (2, 250)])).unwrap();
        assert_eq!(out, asks(&[(1, 375), (2, 125)]));
        assert_eq!(out.iter().map(|(_, c)| c.0).sum::<i64>(), 50_000);
    }

    /// A goal asking for nothing is not handed money just because there is
    /// some: an undated goal has no runway and a met one needs nothing.
    #[test]
    fn a_zero_ask_stays_zero_whether_or_not_the_asks_fit() {
        let fits = fit(Cents::from_dollars(1_000), &asks(&[(1, 100), (2, 0)])).unwrap();
        assert_eq!(fits[1].1, Cents::ZERO);

        let over = fit(Cents::from_dollars(50), &asks(&[(1, 100), (2, 0)])).unwrap();
        assert_eq!(over[1].1, Cents::ZERO);
        assert_eq!(over[0].1, Cents::from_dollars(50));
    }

    /// Nothing asking for anything leaves the whole pot unallocated rather
    /// than piling it onto whichever goal happens to be first -- which is
    /// what `pro_rata` would do with a zero basis.
    #[test]
    fn asks_of_nothing_leave_the_pot_untouched() {
        let out = fit(Cents::from_dollars(1_000), &asks(&[(1, 0), (2, 0)])).unwrap();
        assert_eq!(out.iter().map(|(_, c)| c.0).sum::<i64>(), 0);
    }

    #[test]
    fn an_empty_set_of_asks_allocates_nothing() {
        assert!(fit(Cents::from_dollars(1_000), &[]).unwrap().is_empty());
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A monthly bill spread over 26 pay periods, rounded up: the exact
    /// share is `monthly * 12 / 26`, and every case here lands between two
    /// cents so the ceiling is what is being asserted.
    #[test]
    fn biweekly_rounds_each_monthly_bill_up_to_the_next_cent() {
        let d = Cents::from_dollars;
        let bw = |monthly| biweekly(monthly, 26).unwrap();
        assert_eq!(bw(d(1_200)), d(554)); // exactly 553.85
        assert_eq!(bw(d(300)), d(139)); //   exactly 138.46
        assert_eq!(bw(d(90)), d(42)); //     exactly  41.54
        assert_eq!(bw(d(60)), d(28)); //     exactly  27.69
        assert_eq!(bw(d(25)), d(12)); //     exactly  11.54
        assert_eq!(bw(d(1_000)), d(462)); // exactly 461.54
    }

    /// What a goal asks of one paycheck: the remainder divided across the
    /// paychecks its deadline leaves, rounded up. Today is 2026-08-12
    /// throughout, period 14 days.
    #[test]
    fn per_paycheck_divides_the_remainder_over_the_paychecks_left() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        let case = |cur, goal, by| per_paycheck(d(cur), d(goal), Some(by), today, 14).unwrap();

        // 20 days out -> 2 paychecks, 20/2
        assert_eq!(case(480, 500, day(2026, 9, 1)), Some(d(10)));
        // 20 days out -> 2 paychecks, 150/2
        assert_eq!(case(0, 150, day(2026, 9, 1)), Some(d(75)));
        // 142 days -> 11 paychecks, 4000/11 = 363.64
        assert_eq!(case(10_000, 14_000, day(2027, 1, 1)), Some(d(364)));
        // 385 days -> 28 paychecks, 3000/28 = 107.14
        assert_eq!(case(9_000, 12_000, day(2027, 9, 1)), Some(d(108)));
    }

    #[test]
    fn per_paycheck_is_none_when_undated_or_already_met() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        // An undated goal has no per-paycheck figure: nothing says by when.
        assert_eq!(
            per_paycheck(d(4_000), d(7_000), None, today, 14).unwrap(),
            None
        );
        // A goal sitting at or past its target asks for nothing.
        assert_eq!(
            per_paycheck(d(7_500), d(7_500), Some(day(2026, 12, 1)), today, 14).unwrap(),
            None
        );
        assert_eq!(
            per_paycheck(d(8_000), d(7_500), Some(day(2026, 12, 1)), today, 14).unwrap(),
            None
        );
    }

    #[test]
    fn per_paycheck_clamps_past_due_goals_to_one_paycheck() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        // A goal whose date has passed still owes the full remainder now.
        assert_eq!(
            per_paycheck(d(100), d(250), Some(day(2026, 7, 1)), today, 14).unwrap(),
            Some(d(150))
        );
    }

    /// `period_days` comes straight from a workbook cell (`Constants!H2`), so
    /// a `0` there is reachable. The clamp absorbs it: this must compute a
    /// figure, not surface `div_ceil`'s non-positive-divisor error.
    #[test]
    fn per_paycheck_tolerates_a_zero_period() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        let result = per_paycheck(d(100), d(250), Some(day(2026, 9, 1)), today, 0).unwrap();
        assert!(result.is_some());
    }

    /// `periods_per_year` comes straight from `Constants!G2`, so a `0` there
    /// is reachable too. Same reasoning as `per_paycheck`'s clamp.
    #[test]
    fn biweekly_tolerates_zero_periods_per_year() {
        assert!(biweekly(Cents::from_dollars(100), 0).is_ok());
    }

    #[test]
    fn month_end_projection_is_the_first_of_next_month() {
        assert_eq!(month_end_projection(day(2026, 8, 12)), day(2026, 9, 1));
        assert_eq!(month_end_projection(day(2026, 8, 31)), day(2026, 9, 1));
        assert_eq!(month_end_projection(day(2026, 12, 4)), day(2027, 1, 1));
    }
}
