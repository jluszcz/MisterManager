//! Proportions, at the two scalings the workbook writes them in.
//!
//! Both were `i64` before they were distinguished, and the two scalings differ
//! by a factor of 100 -- a sales tax rate handed to a Planning split computes a
//! plausible, badly wrong number rather than failing.

use crate::money::Cents;
use std::fmt;
use std::iter::Sum;
use std::ops::{Add, Sub};

/// A proportion in whole percent: `Percent(35)` is 35%, the way the Planning
/// splits are written in `Planning!F25:F27`.
///
/// Not only a share of something being divided up. A goal's funded percentage
/// is one of these too, and it is routinely outside `0..=100` -- an overfunded
/// goal is `Percent(106)`, an overspent one is negative. The `0..=100` bound
/// belongs to `planning::parse_percent`, which enforces it on the one path
/// where an out-of-range value would reroute real money; it is not an
/// invariant of the type, and nothing here may assume it.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Percent(pub i64);

impl Percent {
    pub const ZERO: Percent = Percent(0);
    pub const ONE_HUNDRED: Percent = Percent(100);

    /// This share of `value`, truncated toward zero.
    ///
    /// Widened to `i128` for the multiply: a percentage of a large balance
    /// overflows `i64` cents well before the division brings it back in range.
    pub fn of(self, value: Cents) -> Cents {
        Cents(((value.0 as i128 * self.0 as i128) / 100) as i64)
    }

    /// `self - rhs`, floored at zero.
    ///
    /// The Goals split is whatever the other shares leave behind, and those
    /// shares are user-editable: configured to more than 100 between them, an
    /// unsaturated subtraction would allocate a negative share.
    pub fn saturating_sub(self, rhs: Percent) -> Percent {
        Percent((self.0 - rhs.0).max(0))
    }
}

impl Add for Percent {
    type Output = Percent;
    fn add(self, rhs: Percent) -> Percent {
        Percent(self.0 + rhs.0)
    }
}

impl Sum for Percent {
    fn sum<I: Iterator<Item = Percent>>(iter: I) -> Percent {
        Percent(iter.map(|p| p.0).sum())
    }
}

/// A rate in basis points: `BasisPoints(625)` is 6.25%, the scaling
/// `Constants!E2` is read at.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BasisPoints(pub i64);

impl BasisPoints {
    pub const ZERO: BasisPoints = BasisPoints(0);
    /// One whole unit -- the multiplier `1.0` at this scaling.
    pub const ONE: BasisPoints = BasisPoints(10_000);

    /// The same share to the nearest whole percent, rounded half away from
    /// zero: `BasisPoints(9_056)` is `91`, `BasisPoints(-4_137)` is `-41`.
    ///
    /// Here beside [`Display`](fmt::Display) rather than in whichever screen
    /// wanted it first, for that impl's own reason: a share spelled two ways
    /// across the app reads as two allocations of the same money, and the
    /// spelling is a property of the type either way.
    ///
    /// What earns a second one is the *question* being asked. Two decimals
    /// are what a share being measured against another share needs -- the
    /// allocation summary's columns, where a point is a real gap. A fund's
    /// own stock share is read down a column, one line per holding, and the
    /// hundredths there are the filing's rounding rather than anything the
    /// owner acts on.
    pub fn whole_percent(self) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        format!("{sign}{}", (abs + 50) / 100)
    }
}

impl Add for BasisPoints {
    type Output = BasisPoints;
    fn add(self, rhs: BasisPoints) -> BasisPoints {
        BasisPoints(self.0 + rhs.0)
    }
}

/// Plain subtraction rather than `Percent`'s saturating one: the difference
/// between two of these is a *gap*, and a portfolio over-weight in bonds has
/// to be able to say so. What the floor at zero protects on `Percent` -- a
/// negative share of money being divided up -- has no counterpart here.
impl Sub for BasisPoints {
    type Output = BasisPoints;
    fn sub(self, rhs: BasisPoints) -> BasisPoints {
        BasisPoints(self.0 - rhs.0)
    }
}

/// A percentage with two decimals: `BasisPoints(3_600)` is `36.00`, and
/// `BasisPoints(-500)` is `-5.00`.
///
/// Signed because [`Sub`] above produces negative ones and both of the places
/// that spend them draw the result: the gap between a target and an actual,
/// and a composition whose slices claim more than the whole of themselves. A
/// `Display` dropping the sign would draw an over-weight class as an
/// under-weight one.
///
/// On the type rather than beside a screen, because the Funds screen and the
/// report both print these and a share the two rendered differently would
/// read as two different allocations of the same money.
impl fmt::Display for BasisPoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", abs / 100, abs % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Funds list's `Stock%` column, at the three places rounding can go
    /// wrong: up, down, and the exact half, which goes away from zero rather
    /// than to the nearer even -- a share is not a measurement being averaged
    /// over, and a column where `90.50` and `91.50` landed on different sides
    /// of their own halves would read as arbitrary.
    #[test]
    fn whole_percent_rounds_half_away_from_zero() {
        assert_eq!(BasisPoints(9_056).whole_percent(), "91");
        assert_eq!(BasisPoints(9_049).whole_percent(), "90");
        assert_eq!(BasisPoints(9_050).whole_percent(), "91");
        assert_eq!(BasisPoints(-4_137).whole_percent(), "-41");
        assert_eq!(BasisPoints(-4_150).whole_percent(), "-42");
    }

    /// Zero and the whole are the two ends of the `Stock%` column, and a
    /// fund reported to hold no stock says so with a figure -- the `—` beside
    /// it means nobody has asked.
    #[test]
    fn whole_percent_states_both_ends_of_the_scale() {
        assert_eq!(BasisPoints::ZERO.whole_percent(), "0");
        assert_eq!(BasisPoints::ONE.whole_percent(), "100");
    }

    /// The Planning splits against a `Planning!D22`-shaped remainder, one
    /// carrying cents so the truncation is what is being asserted.
    #[test]
    fn percent_of_takes_the_splits_the_waterfall_asks_for() {
        let remainder = Cents(1_380_147);
        assert_eq!(Percent(35).of(remainder), Cents(483_051)); // 4,830.51
        assert_eq!(Percent(15).of(remainder), Cents(207_022)); // 2,070.22
    }

    #[test]
    fn percent_of_truncates_toward_zero() {
        assert_eq!(Percent(50).of(Cents(101)), Cents(50));
        assert_eq!(Percent(50).of(Cents(-101)), Cents(-50));
    }

    #[test]
    fn percent_of_zero_and_everything() {
        assert_eq!(Percent::ZERO.of(Cents(1_380_147)), Cents::ZERO);
        assert_eq!(Percent::ONE_HUNDRED.of(Cents(1_380_147)), Cents(1_380_147));
    }

    /// A percentage of a large balance overflows `i64` cents partway
    /// through: a six-figure Net multiplied by 35 exceeds `i64::MAX` before
    /// the division brings it back, and only the widening this guards keeps
    /// it out of the wrap.
    #[test]
    fn percent_of_does_not_overflow_on_a_large_balance() {
        let huge = Cents(i64::MAX / 50);
        assert_eq!(Percent(50).of(huge), Cents(i64::MAX / 100));
    }

    #[test]
    fn saturating_sub_floors_at_zero() {
        assert_eq!(
            Percent::ONE_HUNDRED.saturating_sub(Percent(35)),
            Percent(65)
        );
        assert_eq!(
            Percent::ONE_HUNDRED.saturating_sub(Percent(120)),
            Percent::ZERO
        );
    }

    /// `Sub` produces these and both sinks draw them, so the sign is part of
    /// the figure rather than something a screen adds back.
    #[test]
    fn a_negative_basis_point_figure_prints_its_sign() {
        assert_eq!(BasisPoints(3_600).to_string(), "36.00");
        assert_eq!(
            (BasisPoints(1_800) - BasisPoints(5_937)).to_string(),
            "-41.37"
        );
        assert_eq!(BasisPoints(-5).to_string(), "-0.05");
    }

    #[test]
    fn percents_add_and_sum() {
        assert_eq!(Percent(35) + Percent(15), Percent(50));
        let total: Percent = [Percent(35), Percent(15), Percent(15)].into_iter().sum();
        assert_eq!(total, Percent(65));
    }
}
