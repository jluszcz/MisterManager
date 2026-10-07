//! Proportions, at the two scalings the workbook writes them in.
//!
//! The two scalings differ by a factor of 100, so a value read at one and
//! spent at the other computes a plausible, badly wrong number rather than
//! failing -- which is why each is its own type rather than a bare `i64`.

use crate::money::Cents;
use std::fmt;
use std::ops::{Add, Sub};

/// A proportion in whole percent: `Percent(35)` is 35%.
///
/// A goal's funded percentage and an account's tax-free share, both routinely
/// stated in whole points, where a fraction of a point is noise. It is not a share of
/// money being divided up -- the Planning splits are [`BasisPoints`], so the
/// owner can split a payday `10.5 / 20.5 / 69` -- and nothing here bounds it:
/// an overfunded goal is `Percent(106)`, an overspent one is negative.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Percent(pub i64);

impl Percent {
    pub const ZERO: Percent = Percent(0);
    pub const ONE_HUNDRED: Percent = Percent(100);

    /// This share of `value`, truncated toward zero.
    ///
    /// Widened to `i128` for the multiply, for [`BasisPoints::of`]'s reason.
    pub fn of(self, value: Cents) -> Cents {
        Cents(((value.0 as i128 * self.0 as i128) / 100) as i64)
    }
}

/// A rate in basis points: `BasisPoints(625)` is 6.25%, the scaling
/// `Constants!E2` is read at, and the Planning splits' too
/// (`Planning!F19`, `F25:F27`).
///
/// The splits' `0..=100%` bound belongs to `planning::parse_percent` and
/// `plan::check_splits`, which enforce it on the paths where an out-of-range
/// share would reroute real money; it is not an invariant of the type, and a
/// gap between two of these is routinely negative.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BasisPoints(pub i64);

impl BasisPoints {
    pub const ZERO: BasisPoints = BasisPoints(0);
    /// One whole unit -- the multiplier `1.0` at this scaling.
    pub const ONE: BasisPoints = BasisPoints(10_000);

    /// This share of `value`, truncated toward zero.
    ///
    /// Widened to `i128` for the multiply: a share of a large balance
    /// overflows `i64` cents well before the division brings it back in range.
    pub fn of(self, value: Cents) -> Cents {
        Cents(((value.0 as i128 * self.0 as i128) / 10_000) as i64)
    }

    /// `self - rhs`, floored at zero.
    ///
    /// The Goals split is whatever the other shares leave behind, and those
    /// shares are user-editable: configured to more than 100% between them,
    /// an unsaturated subtraction would allocate a negative share. [`Sub`] is
    /// the other reading, for a gap that has to be able to go negative.
    pub fn saturating_sub(self, rhs: BasisPoints) -> BasisPoints {
        BasisPoints((self.0 - rhs.0).max(0))
    }

    /// The same share as a percentage with no trailing zeros: `BasisPoints(1_050)`
    /// is `10.5`, `BasisPoints(3_000)` is `30`, `BasisPoints(1_025)` is `10.25`.
    ///
    /// The fourth question: a share the owner typed, read back as they typed
    /// it. A Planning split is a choice rather than a measurement, so every
    /// digit it has is one somebody meant -- rounding `10.25` to a tenth would
    /// draw a split the waterfall is not dividing by, and padding `30` to
    /// `30.00` claims a precision nobody chose. It is also what `e` prefills,
    /// so opening the editor and pressing Enter writes back what was there.
    pub fn exact_percent(self) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let (whole, frac) = (abs / 100, abs % 100);
        match frac {
            0 => format!("{sign}{whole}"),
            f if f % 10 == 0 => format!("{sign}{whole}.{}", f / 10),
            f => format!("{sign}{whole}.{f:02}"),
        }
    }

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

    /// The same share to the nearest tenth of a percent, rounded half away
    /// from zero: `BasisPoints(1_176)` is `11.8`, `BasisPoints(1_000)` is
    /// `10.0`.
    ///
    /// The third question: a share read against a target band drawn from a
    /// rule of thumb, where a hundredth is precision the rule never had and a
    /// whole point hides a gap of most of one. The Retirement standings and
    /// milestones are what ask it.
    pub fn tenth_percent(self) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let tenths = (self.0.unsigned_abs() + 5) / 10;
        format!("{sign}{}.{}", tenths / 10, tenths % 10)
    }
}

impl Add for BasisPoints {
    type Output = BasisPoints;
    fn add(self, rhs: BasisPoints) -> BasisPoints {
        BasisPoints(self.0 + rhs.0)
    }
}

/// Plain subtraction, beside [`BasisPoints::saturating_sub`]: the difference
/// between two of these is a *gap*, and a portfolio over-weight in bonds has
/// to be able to say so. What the floor at zero protects -- a negative share
/// of money being divided up -- is the Goals split's question, not this one.
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

    #[test]
    fn a_whole_percent_of_truncates_toward_zero() {
        assert_eq!(Percent(25).of(Cents(1_001)), Cents(250));
        assert_eq!(Percent(25).of(Cents(-1_001)), Cents(-250));
    }

    /// The Planning splits against a `Planning!D22`-shaped remainder, one
    /// carrying cents so the truncation is what is being asserted.
    #[test]
    fn of_takes_the_splits_the_waterfall_asks_for() {
        let remainder = Cents(1_380_147);
        assert_eq!(BasisPoints(3_500).of(remainder), Cents(483_051)); // 4,830.51
        assert_eq!(BasisPoints(1_500).of(remainder), Cents(207_022)); // 2,070.22
    }

    /// The point of the scaling: a split with a fraction of a percent in it
    /// takes that fraction of the money rather than the whole point either
    /// side of it.
    #[test]
    fn of_takes_a_fractional_percentage() {
        let remainder = Cents::from_dollars(1_000);
        assert_eq!(BasisPoints(1_050).of(remainder), Cents::from_dollars(105));
        assert_eq!(BasisPoints(2_050).of(remainder), Cents::from_dollars(205));
        assert_eq!(BasisPoints(6_900).of(remainder), Cents::from_dollars(690));
    }

    #[test]
    fn of_truncates_toward_zero() {
        assert_eq!(BasisPoints(5_000).of(Cents(101)), Cents(50));
        assert_eq!(BasisPoints(5_000).of(Cents(-101)), Cents(-50));
    }

    #[test]
    fn of_zero_and_everything() {
        assert_eq!(BasisPoints::ZERO.of(Cents(1_380_147)), Cents::ZERO);
        assert_eq!(BasisPoints::ONE.of(Cents(1_380_147)), Cents(1_380_147));
    }

    /// A share of a large balance overflows `i64` cents partway through: a
    /// six-figure Net multiplied by 3,500 exceeds `i64::MAX` before the
    /// division brings it back, and only the widening this guards keeps it
    /// out of the wrap.
    #[test]
    fn of_does_not_overflow_on_a_large_balance() {
        let huge = Cents(i64::MAX / 50);
        assert_eq!(BasisPoints(5_000).of(huge), Cents(i64::MAX / 100));
    }

    #[test]
    fn saturating_sub_floors_at_zero() {
        assert_eq!(
            BasisPoints::ONE.saturating_sub(BasisPoints(3_500)),
            BasisPoints(6_500)
        );
        assert_eq!(
            BasisPoints::ONE.saturating_sub(BasisPoints(12_000)),
            BasisPoints::ZERO
        );
    }

    #[test]
    fn an_exact_percent_drops_only_the_zeros_nobody_typed() {
        assert_eq!(BasisPoints(3_000).exact_percent(), "30");
        assert_eq!(BasisPoints(1_050).exact_percent(), "10.5");
        assert_eq!(BasisPoints(1_025).exact_percent(), "10.25");
        assert_eq!(BasisPoints(1_005).exact_percent(), "10.05");
        assert_eq!(BasisPoints(5).exact_percent(), "0.05");
        assert_eq!(BasisPoints::ZERO.exact_percent(), "0");
        assert_eq!(BasisPoints::ONE.exact_percent(), "100");
        assert_eq!(BasisPoints(-1_050).exact_percent(), "-10.5");
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
    fn basis_points_add() {
        assert_eq!(BasisPoints(3_500) + BasisPoints(1_550), BasisPoints(5_050));
        assert_eq!(
            BasisPoints(1_050) + BasisPoints(2_050) + BasisPoints(6_900),
            BasisPoints::ONE
        );
    }

    #[test]
    fn a_tenth_percent_rounds_half_away_from_zero() {
        assert_eq!(BasisPoints(1_176).tenth_percent(), "11.8");
        assert_eq!(BasisPoints(1_000).tenth_percent(), "10.0");
        assert_eq!(BasisPoints(1_245).tenth_percent(), "12.5");
        assert_eq!(BasisPoints(1_244).tenth_percent(), "12.4");
        assert_eq!(BasisPoints(-1_245).tenth_percent(), "-12.5");
        assert_eq!(BasisPoints(0).tenth_percent(), "0.0");
    }
}
