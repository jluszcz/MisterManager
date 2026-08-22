use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result};

/// The workbook's `Tax()` lambda:
///
/// ```text
/// LET(v, price*(1+rate), inc, IFS(v<500, 1, TRUE, 5), CEILING.MATH(v, inc))
/// ```
///
/// The whole computation stays in scaled integers so there is no float
/// rounding: `num` is the taxed value scaled by 10,000, which is the scaling
/// `BasisPoints` already carries.
pub fn tax(price: Cents, rate: BasisPoints) -> Result<Cents> {
    const SCALE: i64 = BasisPoints::ONE.0;
    let num = price
        .0
        .checked_mul(SCALE + rate.0)
        .with_context(|| format!("price is too large to tax: {price}"))?;
    // Threshold is $500 of taxed value, expressed at the same scale.
    let increment = if num < 50_000 * SCALE { 100 } else { 500 };
    let step = SCALE * increment;
    // `step` is built from constants, so this `?` cannot fire today. It is
    // here so a future increment that is computed rather than chosen from a
    // literal cannot introduce a divide by zero unnoticed.
    let units = super::div_ceil(num, step)?;
    Ok(Cents(units * increment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Cents;

    /// Every case is a live cell from Money.xlsx, sheet `Savings`.
    #[test]
    fn matches_the_workbook() {
        let d = Cents::from_dollars;
        // Below $500 after tax: round up to the next dollar.
        assert_eq!(tax(d(5), BasisPoints(625)).unwrap(), d(6)); // 5.3125
        assert_eq!(tax(d(9), BasisPoints(625)).unwrap(), d(10)); // 9.5625
        assert_eq!(tax(d(10), BasisPoints(625)).unwrap(), d(11)); // 10.625
        assert_eq!(tax(d(48), BasisPoints(625)).unwrap(), d(51)); // exactly 51.00
        assert_eq!(tax(d(100), BasisPoints(625)).unwrap(), d(107)); // 106.25
        assert_eq!(tax(d(200), BasisPoints(625)).unwrap(), d(213)); // 212.50
        // At or above $500 after tax: round up to the next $5.
        assert_eq!(tax(d(475), BasisPoints(625)).unwrap(), d(505)); // 504.6875, crosses the threshold
        assert_eq!(tax(d(650), BasisPoints(625)).unwrap(), d(695)); // 690.625
        assert_eq!(tax(d(700), BasisPoints(625)).unwrap(), d(745)); // 743.75
        assert_eq!(tax(d(1200), BasisPoints(625)).unwrap(), d(1275)); // exactly 1275.00
        assert_eq!(tax(d(1428), BasisPoints(625)).unwrap(), d(1520)); // 1517.25
        assert_eq!(tax(d(2300), BasisPoints(625)).unwrap(), d(2445)); // 2443.75
        assert_eq!(tax(d(6725), BasisPoints(625)).unwrap(), d(7150)); // 7145.3125
        assert_eq!(tax(d(11800), BasisPoints(625)).unwrap(), d(12540)); // 12537.50
    }

    /// The scaled multiply is where a base large enough to be nonsense would
    /// wrap, and `goal::target` is on the strict path behind every Planning
    /// gate -- a wrapped negative there is a figure the waterfall would act
    /// on. The base reaches this from a workbook cell as well as from a form.
    #[test]
    fn refuses_a_price_too_large_to_scale_rather_than_wrapping() {
        let rate = BasisPoints(625);
        let widest = Cents(i64::MAX / (BasisPoints::ONE.0 + rate.0));
        assert!(tax(widest, rate).is_ok());
        let err = tax(Cents(widest.0 + 1), rate).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn zero_stays_zero() {
        assert_eq!(tax(Cents::ZERO, BasisPoints(625)).unwrap(), Cents::ZERO);
    }
}
