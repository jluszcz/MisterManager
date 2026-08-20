//! Proportions, at the two scalings the workbook writes them in.
//!
//! Both were `i64` before they were distinguished, and the two scalings differ
//! by a factor of 100 -- a sales tax rate handed to a Planning split computes a
//! plausible, badly wrong number rather than failing.

use crate::money::Cents;
use std::iter::Sum;
use std::ops::Add;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn percents_add_and_sum() {
        assert_eq!(Percent(35) + Percent(15), Percent(50));
        let total: Percent = [Percent(35), Percent(15), Percent(15)].into_iter().sum();
        assert_eq!(total, Percent(65));
    }
}
