//! The portfolio-wide asset-allocation targets behind the Funds screen.
//!
//! Three shares, over the whole portfolio rather than a row at a time: bonds
//! track age at one point a year over thirty, and the equity remainder splits
//! by one configured international share.
//!
//! ```text
//! bonds       = (age - 30) / 100, clamped to 0..=1.0     bonds track age
//! intl_stock  = (1 - bonds) * intl_equity_share            the equity split
//! us_stock    = (1 - bonds) - intl_stock                   the remainder, not a second multiply
//! ```
//!
//! Taking `us_stock` as the remainder rather than `(1 - bonds) * (1 - intl_equity_share)` is what
//! keeps the three shares footing to exactly `BasisPoints::ONE` under truncation: two independent
//! multiplies can each truncate down and leave the total a basis point or two short.
//!
//! Pure, and with no database in it: `crate::fund` is the one place a stored
//! setting becomes an argument here.

use crate::rate::BasisPoints;
use chrono::{Datelike, NaiveDate};

/// The age at which a bond allocation starts, one percentage point a year.
///
/// Named here rather than written into the formula because it is the only
/// number in the rule; no birth year appears anywhere in the crate.
pub const BONDS_START_AGE: i64 = 30;

/// The three target shares, derived on every read.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Targets {
    /// `None` only when no birth date is on record — a question to ask, not a
    /// zero to assume. The two equity targets then divide the whole 100%
    /// rather than being told a bond target that is really a question.
    pub bonds: Option<BasisPoints>,
    pub us_stock: BasisPoints,
    pub intl_stock: BasisPoints,
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

/// The three target shares.
///
/// `age` is `None` when no birth date is on record. The bond share is
/// clamped at both ends: an age at or under [`BONDS_START_AGE`] targets no
/// bonds rather than a negative share, and an age far enough past it targets
/// all bonds rather than overflowing the equity remainder into the negative.
/// The equity remainder always splits by `intl_equity_share` of what the bond
/// share leaves, so the three always foot to `BasisPoints::ONE`.
///
/// **`intl_equity_share` is clamped to `0..=`[`BasisPoints::ONE`] here**, which
/// is the whole guard over it: it is stored as a ratio of two cells the
/// importer reads through `cell::as_rate_bp`, which reads whatever the sheet
/// carries, and a negative cell against a larger positive one stores a
/// negative share. Unclamped that hands back a negative `intl_stock` and a
/// `us_stock` over 100%, which is two wrong percentages on the Funds screen.
/// The clamp belongs with the derivation for the reason `planning::compute`'s
/// does, and is silent for the same reason: nothing in the app can write the
/// setting, so there is no screen with anything to report.
pub fn targets(age: Option<i64>, intl_equity_share: BasisPoints) -> Targets {
    let bonds =
        age.map(|age| BasisPoints(((age - BONDS_START_AGE) * 100).clamp(0, BasisPoints::ONE.0)));
    let equity_remainder = BasisPoints::ONE.0 - bonds.map_or(0, |b| b.0);

    let intl_share = intl_equity_share.0.clamp(0, BasisPoints::ONE.0);
    let intl_stock = BasisPoints(
        ((i128::from(equity_remainder) * i128::from(intl_share)) / i128::from(BasisPoints::ONE.0))
            as i64,
    );
    let us_stock = BasisPoints(equity_remainder - intl_stock.0);

    Targets {
        bonds,
        us_stock,
        intl_stock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn the_bond_target_is_one_point_a_year_over_thirty() {
        let t = targets(Some(48), BasisPoints(4_000));
        assert_eq!(t.bonds, Some(BasisPoints(1_800)));
    }

    #[test]
    fn the_equity_remainder_splits_by_the_configured_share() {
        let t = targets(Some(48), BasisPoints(4_000));
        // 82% equity, 40% of it international.
        assert_eq!(t.intl_stock, BasisPoints(3_280));
        assert_eq!(t.us_stock, BasisPoints(4_920));
    }

    #[test]
    fn the_three_targets_foot_to_one_hundred_percent() {
        let t = targets(Some(48), BasisPoints(4_000));
        let total = t.bonds.unwrap().0 + t.us_stock.0 + t.intl_stock.0;
        assert_eq!(total, 10_000);
    }

    #[test]
    fn with_no_birth_date_the_equity_targets_divide_the_whole_hundred_percent() {
        let t = targets(None, BasisPoints(4_000));
        assert_eq!(
            t.bonds, None,
            "a missing birth date became a zero bond target"
        );
        assert_eq!(t.intl_stock, BasisPoints(4_000));
        assert_eq!(t.us_stock, BasisPoints(6_000));
    }

    #[test]
    fn an_age_at_or_under_thirty_targets_no_bonds_rather_than_a_negative_share() {
        assert_eq!(
            targets(Some(30), BasisPoints(4_000)).bonds,
            Some(BasisPoints::ZERO)
        );
        assert_eq!(
            targets(Some(22), BasisPoints(4_000)).bonds,
            Some(BasisPoints::ZERO)
        );
    }

    #[test]
    fn an_age_past_a_hundred_and_thirty_targets_all_bonds_rather_than_overflowing() {
        assert_eq!(
            targets(Some(200), BasisPoints(4_000)).bonds,
            Some(BasisPoints(10_000))
        );
    }

    /// The setting behind `intl_equity_share` is a ratio of two cells read
    /// off the sheet unbounded, so a negative one against a larger positive
    /// one stores a negative share. Unclamped, that is a negative
    /// international target beside a US target over 100% -- two wrong
    /// percentages that still foot to a hundred, which is what makes it
    /// unnoticeable rather than obviously broken.
    #[test]
    fn an_equity_split_outside_the_range_is_clamped_rather_than_drawn() {
        let below = targets(Some(48), BasisPoints(-2_000));
        assert_eq!(below.intl_stock, BasisPoints::ZERO);
        assert_eq!(
            below.us_stock,
            BasisPoints(8_200),
            "the whole equity remainder"
        );

        let above = targets(Some(48), BasisPoints(12_000));
        assert_eq!(above.intl_stock, BasisPoints(8_200));
        assert_eq!(above.us_stock, BasisPoints::ZERO);

        for t in [below, above] {
            assert_eq!(
                t.bonds.unwrap().0 + t.us_stock.0 + t.intl_stock.0,
                10_000,
                "the clamp broke the three targets' footing"
            );
        }
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
