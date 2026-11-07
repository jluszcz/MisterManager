use super::schedule::{self, Step};
use crate::money::Cents;
use anyhow::Result;
use chrono::{Datelike, Days, NaiveDate};

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

/// A year, in the whole weeks a pay cadence divides it into.
///
/// Weeks rather than the calendar's 365 days, because a pay cadence counts in
/// weeks. `52 * 7` is 364, which divides exactly by every whole-week cadence
/// there is -- weekly, biweekly, four-weekly -- so the floor in
/// [`period_days`] is an approximation only for the cadences that were never
/// whole weeks to begin with. The 365th day belongs to no pay period, and all
/// it can ever do is round one of them up.
const WEEKS_PER_YEAR: i64 = 52;

/// The other half of that year, so neither number is a bare `7` in a divide.
const DAYS_PER_WEEK: i64 = 7;

/// How many days apart two paydays fall, for a cadence of `periods_per_year`.
///
/// Derived rather than stored. The workbook carried it as a cell of its own
/// (`Constants!H2`) beside the count (`Constants!G2`), and two cells for one
/// fact can disagree: 26 periods and 15 days is a pay cadence that exists
/// nowhere, and a database holding it would count every deadline's runway in
/// one cadence while spreading every annual cost in the other. One setting,
/// so there is nothing to reconcile.
///
/// Exact for every whole-week cadence -- 52 -> 7, 26 -> 14, 13 -> 28 -- and
/// floored for the ones that are not: 24 -> 15, 12 -> 30. A semi-monthly or
/// monthly payday does not fall a fixed number of days apart at all, so there
/// is no exact answer to floor away.
///
/// Clamped at both ends, because `periods_per_year` is the owner's setting
/// and reaches a divide: a count at zero or below would divide by it, and one
/// above the days in a year of weeks would floor to a period no days long.
pub fn period_days(periods_per_year: i64) -> i64 {
    (WEEKS_PER_YEAR * DAYS_PER_WEEK / periods_per_year.max(1)).max(1)
}

/// The paydays a goal's runway is counted in.
///
/// Two answers to "how many paychecks are left before this date", because a
/// database may or may not say when its paychecks land. When it does, the
/// count is of those paychecks themselves: a runway measured in days and
/// divided by a period drifts against the real paydays, so a goal three weeks
/// out reads as two paychecks the day before one lands and as one the day
/// after, and its `$/Pay` doubles overnight with nothing having changed.
/// Counted off the schedule, the figure moves only when a paycheck is
/// allocated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Paydays {
    /// The paycheck recurring transaction's own schedule -- every occurrence
    /// counted from `anchor`, each on the business day it lands on -- and
    /// `from`, the first of them no payday batch has been applied to yet.
    /// Built by [`Paydays::scheduled`], which is where `from` is decided.
    Scheduled {
        anchor: NaiveDate,
        step: Step,
        from: NaiveDate,
    },
    /// No paycheck on record: the cadence setting alone, counted the way the
    /// workbook's `PerPaycheck()` lambda counts it.
    Cadence(i64),
}

impl Paydays {
    /// The paycheck schedule, starting from the first paycheck not yet
    /// allocated.
    ///
    /// A payday is allocated after the paycheck lands as often as before it,
    /// so "the paychecks from today on" is wrong both ways round: a paycheck
    /// that landed on Friday and is allocated on Monday is still this
    /// payday's money, and one allocated on its eve has been spent before it
    /// lands. What decides it is the payday batches themselves. Each one
    /// claims the paycheck its date is nearest -- the earlier on a tie --
    /// which holds whichever side of payday it was entered on and whether it
    /// was dated for today or for when the transfers land. The count then
    /// starts at the most recent paycheck to have landed, stepping past each
    /// one a batch has claimed.
    ///
    /// Never further back than that: a paycheck skipped outright -- no payday
    /// run for it at all -- stops being owed the day the next one lands. The
    /// money it would have funded is in the containers' unallocated remainder
    /// by then rather than in a paycheck still to come, so dividing a goal's
    /// remainder by it would ask too little of every paycheck that is.
    ///
    /// `allocated` is asked for the dates of the payday batches in a window
    /// of paychecks around today; the window is all the claiming can reach,
    /// since a batch outside it is nearer a paycheck outside it.
    pub fn scheduled(
        anchor: NaiveDate,
        step: Step,
        today: NaiveDate,
        allocated: impl FnOnce(NaiveDate, NaiveDate) -> Result<Vec<NaiveDate>>,
    ) -> Result<Paydays> {
        // The two most recent paychecks to have landed, and enough after them
        // that the last one claimable has a neighbour on each side.
        let landed = schedule::occurrences(anchor, step, anchor, today)?;
        let mut around: Vec<NaiveDate> = landed.iter().rev().take(2).rev().copied().collect();
        let start = around.len().saturating_sub(1);
        let mut after = today;
        while around.len() < start + 4 {
            let Some(next) = schedule::next_after(anchor, step, after) else {
                break;
            };
            around.push(next);
            after = next;
        }
        let (Some(&first), Some(&last)) = (around.first(), around.last()) else {
            return Ok(Paydays::Scheduled {
                anchor,
                step,
                from: today,
            });
        };
        // A batch entered on `first`'s eve is `first`'s, and when `first` is
        // the start -- the rule's first paycheck has yet to land, or only it
        // has -- nothing earlier in `around` is there to stand in for it. So
        // the window reaches back to the last day nearer `first` than the
        // paycheck a period before it, the tie going to that earlier one.
        let reach = around
            .get(1)
            .map_or(0, |&second| ((second - first).num_days() - 1) / 2);
        let since = first
            .checked_sub_days(Days::new(reach.max(0) as u64))
            .unwrap_or(first);
        let claimed: Vec<usize> = allocated(since, last)?
            .into_iter()
            .filter_map(|date| {
                // `min_by_key` keeps the first of equal keys: the earlier
                // paycheck on a tie.
                (0..around.len()).min_by_key(|&i| (date - around[i]).num_days().abs())
            })
            .collect();
        let mut i = start;
        while i < around.len() && claimed.contains(&i) {
            i += 1;
        }
        let from = match around.get(i) {
            Some(&from) => from,
            None => schedule::next_after(anchor, step, last).unwrap_or(last),
        };
        Ok(Paydays::Scheduled { anchor, step, from })
    }

    /// How many paychecks fund a goal due on `by`, never fewer than one.
    ///
    /// Scheduled, it is the paychecks landing in `from ..= by`. `by` is in it
    /// because money arriving on the deadline arrives in time.
    ///
    /// At least one, at both branches: a goal whose date has passed, or comes
    /// before the next paycheck, owes its whole remainder to this one -- the
    /// workbook's `MAX(1, ...)`.
    pub fn through(self, today: NaiveDate, by: NaiveDate) -> Result<i64> {
        let count = match self {
            Paydays::Scheduled { anchor, step, from } => {
                schedule::occurrences(anchor, step, from, by)?.len() as i64
            }
            // The cadence in days, which is where `period_days`'s clamp keeps
            // a nonsense setting off `div_ceil`'s divide -- the same call
            // `biweekly` makes above for the same reason.
            Paydays::Cadence(periods_per_year) => {
                super::div_ceil((by - today).num_days(), period_days(periods_per_year))?
            }
        };
        Ok(count.max(1))
    }
}

/// The workbook's `PerPaycheck()` lambda: what to set aside each payday to
/// reach `goal` by `by`, rounded up to a whole dollar.
///
/// ```text
/// IF(OR(ISBLANK(by), cur >= goal), "",
///    CEILING((goal - cur) / MAX(1, CEILING((by - today) / period, 1)), 1))
/// ```
///
/// The divisor is [`Paydays::through`], which is that lambda's only when no
/// paycheck is on record.
///
/// Returns `None` where the sheet shows blank: undated goals, and goals
/// already at or past their target.
pub fn per_paycheck(
    current: Cents,
    goal: Cents,
    by: Option<NaiveDate>,
    today: NaiveDate,
    paydays: Paydays,
) -> Result<Option<Cents>> {
    let Some(by) = by else { return Ok(None) };
    if current >= goal {
        return Ok(None);
    }
    let periods = paydays.through(today, by)?;
    let remaining = goal.0 - current.0;
    Ok(Some(Cents(
        super::div_ceil(remaining, periods * 100)? * 100,
    )))
}

/// What to set aside each payday for a cost that comes round every `years`
/// years, rounded up to a whole dollar.
///
/// The Recurring Goals screen's title: an entry carries a month and a cadence
/// rather than a date, so there is no runway for [`per_paycheck`] to divide.
/// The runway is the cadence itself -- one round's cost spread over every
/// paycheck before it comes round again.
///
/// `years` comes off a `Cadence` and so is never zero; `periods_per_year` is
/// a user-editable setting and is clamped the way [`biweekly`] clamps it.
pub fn per_paycheck_over_years(total: Cents, periods_per_year: i64, years: i64) -> Result<Cents> {
    let periods = periods_per_year.max(1) * years;
    Ok(Cents(super::div_ceil(total.0, periods * 100)? * 100))
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

/// `EOMONTH(date, 0) + 1` — the first day of the month after `date`.
pub fn month_end_projection(date: NaiveDate) -> NaiveDate {
    let (year, month) = (date.year(), date.month());
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
    use crate::test_support::day;

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
        let case = |cur, goal, by| {
            per_paycheck(d(cur), d(goal), Some(by), today, Paydays::Cadence(26)).unwrap()
        };

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
            per_paycheck(d(4_000), d(7_000), None, today, Paydays::Cadence(26)).unwrap(),
            None
        );
        // A goal sitting at or past its target asks for nothing.
        assert_eq!(
            per_paycheck(
                d(7_500),
                d(7_500),
                Some(day(2026, 12, 1)),
                today,
                Paydays::Cadence(26)
            )
            .unwrap(),
            None
        );
        assert_eq!(
            per_paycheck(
                d(8_000),
                d(7_500),
                Some(day(2026, 12, 1)),
                today,
                Paydays::Cadence(26)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn per_paycheck_clamps_past_due_goals_to_one_paycheck() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        // A goal whose date has passed still owes the full remainder now.
        assert_eq!(
            per_paycheck(
                d(100),
                d(250),
                Some(day(2026, 7, 1)),
                today,
                Paydays::Cadence(26)
            )
            .unwrap(),
            Some(d(150))
        );
    }

    /// A biweekly paycheck landing on Fridays -- 7/31, 8/14, 8/28 and 9/11
    /// of 2026 among them -- read on `today` against payday batches dated
    /// `batches`.
    fn fridays(today: NaiveDate, batches: &[NaiveDate]) -> Paydays {
        Paydays::scheduled(day(2026, 7, 31), Step::Days(14), today, |_, _| {
            Ok(batches.to_vec())
        })
        .unwrap()
    }

    /// 7/31's paycheck, allocated the Monday after it landed.
    fn july_allocated() -> NaiveDate {
        day(2026, 8, 3)
    }

    /// Fifteen days out is two periods to the cadence and one paycheck to
    /// the schedule: only the 14th lands before the 27th.
    #[test]
    fn a_scheduled_runway_counts_the_paychecks_that_land_before_the_deadline() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        let by = Some(day(2026, 8, 27));
        assert_eq!(
            per_paycheck(d(0), d(150), by, today, Paydays::Cadence(26)).unwrap(),
            Some(d(75))
        );
        let paydays = fridays(today, &[july_allocated()]);
        assert_eq!(
            per_paycheck(d(0), d(150), by, today, paydays).unwrap(),
            Some(d(150))
        );
    }

    /// Three paychecks left divide the remainder in three, rounded up to the
    /// dollar: 125 / 3 is 41.67.
    #[test]
    fn a_scheduled_ask_is_the_remainder_over_the_paychecks_left_rounded_up() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        let paydays = fridays(today, &[july_allocated()]);
        let ask = per_paycheck(d(1_375), d(1_500), Some(day(2026, 9, 11)), today, paydays);
        assert_eq!(ask.unwrap(), Some(d(42)));
    }

    /// The 14th has landed and nothing has been allocated from it, so it is
    /// still this payday's paycheck on the Monday after -- and stops being
    /// one once its batch is entered, dated for when the transfers land.
    #[test]
    fn a_paycheck_landed_but_not_yet_allocated_still_counts() {
        let today = day(2026, 8, 17);
        let by = day(2026, 9, 11);
        assert_eq!(
            fridays(today, &[july_allocated()])
                .through(today, by)
                .unwrap(),
            3
        );
        let late = [july_allocated(), day(2026, 8, 19)];
        assert_eq!(fridays(today, &late).through(today, by).unwrap(), 2);
    }

    /// Allocated on its eve, the 14th has been spent before it lands, whether
    /// the batch is dated for today or for when the transfers arrive.
    #[test]
    fn a_paycheck_allocated_on_its_eve_no_longer_counts() {
        let today = day(2026, 8, 13);
        let by = day(2026, 9, 11);
        for dated in [today, day(2026, 8, 17)] {
            let batches = [july_allocated(), dated];
            assert_eq!(
                fridays(today, &batches).through(today, by).unwrap(),
                2,
                "{dated}"
            );
        }
    }

    /// The rule's first paycheck, allocated on its eve: the batch is dated
    /// before every paycheck the schedule has, and still claims the first,
    /// whether it is read that eve or once the paycheck has landed.
    #[test]
    fn a_first_paycheck_allocated_on_its_eve_no_longer_counts() {
        let anchor = day(2026, 8, 14);
        let eve = day(2026, 8, 13);
        let by = day(2026, 9, 11);
        for today in [eve, day(2026, 8, 17)] {
            let paydays = Paydays::scheduled(anchor, Step::Days(14), today, |from, to| {
                Ok([eve]
                    .into_iter()
                    .filter(|d| (from..=to).contains(d))
                    .collect())
            })
            .unwrap();
            assert_eq!(paydays.through(today, by).unwrap(), 2, "{today}");
        }
    }

    /// A batch dated halfway between two paychecks is the earlier one's,
    /// being the later one's only if it was entered a week early.
    #[test]
    fn a_batch_midway_between_two_paychecks_claims_the_earlier() {
        let today = day(2026, 8, 21);
        let batches = [july_allocated(), day(2026, 8, 21)];
        assert_eq!(
            fridays(today, &batches)
                .through(today, day(2026, 9, 11))
                .unwrap(),
            2
        );
    }

    /// A paycheck landing today and not yet allocated is the one being
    /// allocated, and one landing on the deadline arrives in time, so both
    /// ends count.
    #[test]
    fn a_paycheck_on_today_or_on_the_deadline_counts_toward_the_runway() {
        let today = day(2026, 8, 14);
        let paydays = fridays(today, &[july_allocated()]);
        assert_eq!(paydays.through(today, day(2026, 8, 28)).unwrap(), 2);
    }

    /// The figure moves only when a paycheck is allocated, not as the days
    /// between two of them run down -- nor on the day the next one lands.
    #[test]
    fn a_scheduled_runway_holds_steady_until_the_next_paycheck_is_allocated() {
        let by = day(2026, 9, 11);
        let batches = [july_allocated(), day(2026, 8, 14)];
        for today in [day(2026, 8, 15), day(2026, 8, 20), day(2026, 8, 28)] {
            assert_eq!(
                fridays(today, &batches).through(today, by).unwrap(),
                2,
                "{today}"
            );
        }
        let today = day(2026, 8, 28);
        let batches = [july_allocated(), day(2026, 8, 14), today];
        assert_eq!(fridays(today, &batches).through(today, by).unwrap(), 1);
    }

    /// A paycheck skipped outright is waited on only until the next one
    /// lands: the 14th goes unallocated, and from the 28th the count starts
    /// there instead.
    #[test]
    fn a_skipped_paycheck_stops_counting_once_the_next_one_lands() {
        let by = day(2026, 9, 11);
        let batches = [july_allocated()];
        let before = day(2026, 8, 27);
        assert_eq!(fridays(before, &batches).through(before, by).unwrap(), 3);
        let after = day(2026, 8, 28);
        assert_eq!(fridays(after, &batches).through(after, by).unwrap(), 2);
    }

    /// A database with no payday batch at all counts the one paycheck that
    /// last landed, not every paycheck since the anchor.
    #[test]
    fn with_no_batch_on_record_only_the_last_paycheck_to_land_counts() {
        let today = day(2026, 8, 12);
        assert_eq!(
            fridays(today, &[])
                .through(today, day(2026, 8, 27))
                .unwrap(),
            2
        );
    }

    /// A deadline before the next paycheck, or already behind today, owes
    /// the whole remainder to this one rather than dividing by nothing.
    #[test]
    fn a_scheduled_runway_with_no_paycheck_left_is_one_paycheck() {
        let today = day(2026, 8, 15);
        let paydays = fridays(today, &[july_allocated(), day(2026, 8, 14)]);
        assert_eq!(paydays.through(today, day(2026, 8, 27)).unwrap(), 1);
        assert_eq!(paydays.through(today, day(2026, 7, 1)).unwrap(), 1);
    }

    /// `periods_per_year` comes straight from a workbook cell
    /// (`Constants!G2`), so a `0` there is reachable. `period_days`'s clamp
    /// absorbs it: this must compute a figure, not surface `div_ceil`'s
    /// non-positive-divisor error.
    #[test]
    fn per_paycheck_tolerates_a_zero_period() {
        let d = Cents::from_dollars;
        let today = day(2026, 8, 12);
        let result = per_paycheck(
            d(100),
            d(250),
            Some(day(2026, 9, 1)),
            today,
            Paydays::Cadence(0),
        )
        .unwrap();
        assert!(result.is_some());
    }

    /// `periods_per_year` comes straight from `Constants!G2`, so a `0` there
    /// is reachable too. Same reasoning as `per_paycheck`'s clamp.
    #[test]
    fn biweekly_tolerates_zero_periods_per_year() {
        assert!(biweekly(Cents::from_dollars(100), 0).is_ok());
    }

    /// The cadences the setting is ever plausibly set to. The workbook
    /// carried this in `Constants!H2` beside the count in `G2`, and the two
    /// agreeing is what makes deriving it safe rather than a second answer to
    /// the same question.
    ///
    /// A year of whole weeks divides exactly by every whole-week cadence, so
    /// only the two that are not whole weeks are floored at all.
    #[test]
    fn period_days_divides_a_year_of_weeks_by_the_pay_cadence() {
        assert_eq!(period_days(52), 7);
        assert_eq!(period_days(26), 14);
        assert_eq!(period_days(13), 28);
        // Neither of these falls a fixed number of days apart to begin with.
        assert_eq!(period_days(24), 15);
        assert_eq!(period_days(12), 30);
    }

    /// Clamped at both ends, for the reason every other divisor off this
    /// setting is: it is the owner's, it reaches a divide, and neither a
    /// count at zero nor one finer than a day should take a screen down.
    #[test]
    fn period_days_is_never_less_than_a_day_whatever_the_setting_says() {
        assert_eq!(period_days(0), 364);
        assert_eq!(period_days(-4), 364);
        assert_eq!(period_days(364), 1);
        assert_eq!(period_days(10_000), 1);
    }

    /// An annual cost is spread over one year's paychecks and a biennial one
    /// over two, both rounded up to a whole dollar.
    #[test]
    fn per_paycheck_over_years_spreads_a_round_over_the_paychecks_before_the_next_one() {
        let d = Cents::from_dollars;
        assert_eq!(per_paycheck_over_years(d(2_600), 26, 1).unwrap(), d(100));
        assert_eq!(per_paycheck_over_years(d(2_600), 26, 2).unwrap(), d(50));
    }

    /// Rounded up rather than to nearest, the way every other figure the
    /// paycheck lambdas produce is: a cent short each payday is a round the
    /// last paycheck cannot cover.
    #[test]
    fn per_paycheck_over_years_rounds_a_remainder_up_to_a_whole_dollar() {
        let d = Cents::from_dollars;
        assert_eq!(per_paycheck_over_years(d(26), 26, 1).unwrap(), d(1));
        assert_eq!(per_paycheck_over_years(Cents(2_601), 26, 1).unwrap(), d(2));
    }

    /// Same clamp, same reason as `biweekly`'s: `periods_per_year` is the
    /// owner's setting, and a `0` must not surface `div_ceil`'s error into a
    /// screen title.
    #[test]
    fn per_paycheck_over_years_tolerates_zero_periods_per_year() {
        assert!(per_paycheck_over_years(Cents::from_dollars(100), 0, 2).is_ok());
    }

    #[test]
    fn month_end_projection_is_the_first_of_next_month() {
        assert_eq!(month_end_projection(day(2026, 8, 12)), day(2026, 9, 1));
        assert_eq!(month_end_projection(day(2026, 8, 31)), day(2026, 9, 1));
        assert_eq!(month_end_projection(day(2026, 12, 4)), day(2027, 1, 1));
    }
}
