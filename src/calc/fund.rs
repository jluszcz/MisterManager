//! The asset-allocation derivation behind the Funds screen —
//! `Planning!J2:L4` in the workbook.
//!
//! ```text
//! J2 = (DATEDIF(Birth Date, Today, "y") - 30) / 100     bonds track age
//! J3 = (1 - J2) * 0.4                                   the equity rows split
//! J4 = (1 - J2) * 0.6                                   what bonds leave
//! K  = M / SUM(M)                                       this row's share of the total
//! L  = MAX(0, J - K)                                    how far below target, in points
//! ```
//!
//! Pure, and with no database in it: [`Rule`] is this module's own enum, and
//! `crate::fund` is the one place `db::fund::Target` becomes one — the same
//! seam `crate::recurring_txn::step` puts between `db`'s `Cadence` and
//! `schedule::Step`.

use crate::money::Cents;
use crate::rate::BasisPoints;
use chrono::{Datelike, NaiveDate};

/// The age at which a bond allocation starts, one percentage point a year.
///
/// Named here rather than written into the formula because it is the only
/// number in the rule; no birth year appears anywhere in the crate.
pub const BONDS_START_AGE: i64 = 30;

/// What decides a row's target share.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Rule {
    AgeOver30,
    RemainderShare(BasisPoints),
}

/// One fund, as the derivation needs it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub rule: Rule,
    pub actual: Cents,
}

/// One fund's three derived columns.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ComputedRow {
    /// `None` only for an age row with no birth date on record — which is a
    /// question to ask, not a zero to assume.
    pub target: Option<BasisPoints>,
    /// This row's share of the total value.
    pub actual: BasisPoints,
    /// `max(0, target - actual)`, and `None` wherever the target is.
    pub delta: Option<BasisPoints>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Computed {
    pub rows: Vec<ComputedRow>,
    /// Every row's value, added up — the sheet's `M5`.
    pub total: Cents,
    /// Every *known* target, added up. Short of 100% whenever the shares do
    /// not divide the remainder completely, which the screen shows rather
    /// than refusing.
    pub target_total: BasisPoints,
    /// The row furthest below its target — the row the next contribution
    /// should go to. The first on a tie, and `None` when every row is at or
    /// above target.
    pub furthest_down: Option<usize>,
}

/// Whole years between two dates — `DATEDIF(..., "y")`.
///
/// A birthday still to come this year does not count, and comparing
/// `(month, day)` pairs is what gets a leap-day birthday right in a common
/// year: there is no February 29th to reach, so the year turns on March 1st.
pub fn whole_years(birth: NaiveDate, today: NaiveDate) -> i64 {
    let years = i64::from(today.year() - birth.year());
    match (today.month(), today.day()) < (birth.month(), birth.day()) {
        true => years - 1,
        false => years,
    }
}

/// The three derived columns, for every row at once.
///
/// `age` is `None` when no birth date is on record. Every multiply widens to
/// `i128` before dividing, and every division truncates toward zero: these
/// are display figures, neither a requirement (which would round up) nor a
/// transfer instruction (which would round down).
pub fn compute(rows: &[Row], age: Option<i64>) -> Computed {
    let age_target = age.map(|age| BasisPoints((age - BONDS_START_AGE).max(0) * 100));

    // An age row with no birth date claims nothing, so the share rows divide
    // the whole 100% -- the honest reading of "we do not know yet".
    let claimed: i64 = rows
        .iter()
        .filter(|row| matches!(row.rule, Rule::AgeOver30))
        .map(|_| age_target.map_or(0, |target| target.0))
        .sum();
    let remainder = (BasisPoints::ONE.0 - claimed).max(0);

    let total = Cents(rows.iter().map(|row| row.actual.0).sum());

    let computed: Vec<ComputedRow> = rows
        .iter()
        .map(|row| {
            let target = match row.rule {
                Rule::AgeOver30 => age_target,
                Rule::RemainderShare(share) => Some(BasisPoints(
                    ((i128::from(remainder) * i128::from(share.0)) / i128::from(BasisPoints::ONE.0))
                        as i64,
                )),
            };
            let actual = match total.0 {
                0 => BasisPoints::ZERO,
                _ => BasisPoints(
                    ((i128::from(row.actual.0) * i128::from(BasisPoints::ONE.0))
                        / i128::from(total.0)) as i64,
                ),
            };
            ComputedRow {
                target,
                actual,
                // From the stored basis points on both sides, so the column
                // the screen prints adds up against the columns beside it.
                delta: target.map(|target| BasisPoints((target.0 - actual.0).max(0))),
            }
        })
        .collect();

    let target_total = BasisPoints(
        computed
            .iter()
            .filter_map(|row| row.target)
            .map(|target| target.0)
            .sum(),
    );

    // Written out rather than `max_by_key`, which returns the *last* maximum
    // on a tie where the spec wants the first.
    let mut furthest_down: Option<usize> = None;
    for (i, row) in computed.iter().enumerate() {
        let delta = row.delta.map_or(0, |delta| delta.0);
        let better = furthest_down
            .is_none_or(|best| delta > computed[best].delta.map_or(0, |delta| delta.0));
        if delta > 0 && better {
            furthest_down = Some(i);
        }
    }

    Computed {
        rows: computed,
        total,
        target_total,
        furthest_down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    fn age_row(dollars: i64) -> Row {
        Row {
            rule: Rule::AgeOver30,
            actual: Cents::from_dollars(dollars),
        }
    }

    fn share_row(share_bp: i64, dollars: i64) -> Row {
        Row {
            rule: Rule::RemainderShare(BasisPoints(share_bp)),
            actual: Cents::from_dollars(dollars),
        }
    }

    /// A whole block, all three columns at once. The bond row is 10% at 40,
    /// and the two equity rows split the 90% it leaves 40/60 into 36% and
    /// 54%. The values divide into thirds and sixths of the 180,000 total,
    /// so the actual column truncates rather than landing exactly.
    #[test]
    fn a_block_derives_its_target_actual_and_delta_columns_together() {
        let computed = compute(
            &[
                age_row(30_000),
                share_row(4_000, 60_000),
                share_row(6_000, 90_000),
            ],
            Some(40),
        );

        let targets: Vec<Option<BasisPoints>> = computed.rows.iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            vec![
                Some(BasisPoints(1_000)),
                Some(BasisPoints(3_600)),
                Some(BasisPoints(5_400))
            ]
        );

        let actual: Vec<BasisPoints> = computed.rows.iter().map(|r| r.actual).collect();
        assert_eq!(
            actual,
            vec![BasisPoints(1_666), BasisPoints(3_333), BasisPoints(5_000)]
        );

        let delta: Vec<Option<BasisPoints>> = computed.rows.iter().map(|r| r.delta).collect();
        assert_eq!(
            delta,
            vec![
                Some(BasisPoints::ZERO),
                Some(BasisPoints(267)),
                Some(BasisPoints(400))
            ]
        );

        assert_eq!(computed.total, Cents::from_dollars(180_000));
        assert_eq!(computed.target_total, BasisPoints::ONE);
        assert_eq!(computed.furthest_down, Some(2));
    }

    /// The row the next contribution should go to.
    #[test]
    fn the_furthest_down_row_is_the_one_with_the_largest_gap() {
        let computed = compute(&[share_row(5_000, 10), share_row(5_000, 90)], None);
        assert_eq!(computed.furthest_down, Some(0));
    }

    /// A tie takes the first, so the answer does not depend on iteration
    /// order.
    #[test]
    fn a_tie_takes_the_first_row() {
        let computed = compute(&[share_row(5_000, 50), share_row(5_000, 50)], None);
        assert_eq!(computed.furthest_down, None, "both are exactly at target");

        let tied = compute(
            &[
                share_row(2_500, 0),
                share_row(2_500, 0),
                share_row(5_000, 1),
            ],
            None,
        );
        assert_eq!(tied.furthest_down, Some(0));
    }

    #[test]
    fn no_row_is_furthest_down_when_every_row_is_at_or_above_target() {
        let computed = compute(&[share_row(2_500, 50), share_row(2_500, 50)], None);
        assert!(
            computed
                .rows
                .iter()
                .all(|r| r.delta == Some(BasisPoints::ZERO))
        );
        assert_eq!(computed.furthest_down, None);
    }

    /// An empty portfolio must not divide by its total.
    #[test]
    fn a_zero_total_gives_every_row_a_zero_actual_share() {
        let computed = compute(&[age_row(0), share_row(4_000, 0)], Some(40));
        let actual: Vec<BasisPoints> = computed.rows.iter().map(|r| r.actual).collect();
        assert_eq!(actual, vec![BasisPoints::ZERO, BasisPoints::ZERO]);
        assert_eq!(computed.total, Cents::ZERO);
        // Nothing is funded, so the largest target is the largest gap -- the
        // equity row's 36% of the remainder, not the bond row's 10%.
        assert_eq!(computed.furthest_down, Some(1));
    }

    /// Nothing is left for the share rows to take a share of.
    #[test]
    fn a_zero_remainder_leaves_every_share_row_at_zero() {
        let computed = compute(&[age_row(1), share_row(6_000, 1)], Some(130));
        assert_eq!(computed.rows[0].target, Some(BasisPoints(10_000)));
        assert_eq!(computed.rows[1].target, Some(BasisPoints::ZERO));
    }

    /// Under thirty there is no bond allocation, and the clamp is what stops
    /// it going negative and inflating the remainder past 100%.
    #[test]
    fn an_age_under_thirty_puts_the_bond_row_at_zero() {
        let computed = compute(&[age_row(1), share_row(4_000, 1)], Some(22));
        assert_eq!(computed.rows[0].target, Some(BasisPoints::ZERO));
        assert_eq!(computed.rows[1].target, Some(BasisPoints(4_000)));
    }

    /// An unset birth date is neither an error nor a silent zero: the row has
    /// no target to show, and the rows beside it divide the whole 100% rather
    /// than being told the bond row claimed nothing when it does.
    #[test]
    fn an_unknown_age_leaves_the_age_row_without_a_target_or_a_delta() {
        let computed = compute(&[age_row(1), share_row(4_000, 1)], None);
        assert_eq!(computed.rows[0].target, None);
        assert_eq!(computed.rows[0].delta, None);
        assert_eq!(computed.rows[1].target, Some(BasisPoints(4_000)));
        assert_eq!(
            computed.target_total,
            BasisPoints(4_000),
            "an unknown target is left out of the sum rather than counted as zero"
        );
    }

    /// Shown, never refused: adding the first share row always leaves the
    /// shares short, so a refusal would block ordinary entry.
    #[test]
    fn shares_that_do_not_sum_to_one_are_computed_as_given() {
        let computed = compute(&[share_row(4_000, 1), share_row(800, 1)], None);
        assert_eq!(computed.target_total, BasisPoints(4_800));
    }

    /// A real portfolio times 10,000 overflows `i64` partway through, and
    /// only the `i128` widening keeps it out of the wrap.
    #[test]
    fn a_large_portfolio_does_not_overflow() {
        let huge = Cents(i64::MAX / 4);
        let computed = compute(
            &[
                Row {
                    rule: Rule::RemainderShare(BasisPoints(5_000)),
                    actual: huge,
                },
                Row {
                    rule: Rule::RemainderShare(BasisPoints(5_000)),
                    actual: huge,
                },
            ],
            None,
        );
        assert_eq!(computed.rows[0].actual, BasisPoints(5_000));
    }

    /// `DATEDIF(..., "y")` semantics: whole years only.
    ///
    /// The birth date is derived from an arbitrary reference date rather
    /// than written down directly -- a literal birth year picked to land on
    /// a real age (44, the workbook block's own age) would be a plausible
    /// real birth date committed to the repository, which the root
    /// `CLAUDE.md`'s rule against writing personal data into persistent
    /// artifacts exists to keep out.
    #[test]
    fn whole_years_does_not_count_a_birthday_still_to_come_this_year() {
        let reference = day(2026, 6, 15);
        let birth = reference.with_year(reference.year() - 44).unwrap();
        assert_eq!(whole_years(birth, reference.pred_opt().unwrap()), 43);
        assert_eq!(whole_years(birth, reference), 44);
        assert_eq!(
            whole_years(
                birth,
                NaiveDate::from_ymd_opt(reference.year(), 12, 31).unwrap()
            ),
            44
        );
        assert_eq!(
            whole_years(
                birth,
                NaiveDate::from_ymd_opt(reference.year() + 1, 1, 1).unwrap()
            ),
            44
        );
    }

    /// A leap-day birthday has no anniversary in a common year, and the
    /// comparison must not claim it does. `2000-02-29` is a synthetic
    /// leap-day fixture, not a real birth date -- at a 2027 "today" it
    /// stands for age 26, which the calendar edge under test needs; it is
    /// not derived from `today - N years` the way the test above is,
    /// because the leap day is the point of the fixture.
    #[test]
    fn a_leap_day_birthday_turns_on_the_first_of_march_in_a_common_year() {
        let birth = day(2000, 2, 29);
        assert_eq!(whole_years(birth, day(2027, 2, 28)), 26);
        assert_eq!(whole_years(birth, day(2027, 3, 1)), 27);
    }
}
