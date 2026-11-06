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

/// The pre-tax price of an amount paid with tax in it: `paid / (1 + rate)`,
/// to the nearest cent, half up.
///
/// Not an inverse of [`tax`], which rounds up to a whole dollar or five: no
/// base taxes to exactly an arbitrary figure, so this is the part of what was
/// paid that was price rather than tax, and what a taxed goal's base falls
/// by when that much is spent towards it. Refuses a negative amount, which
/// is no payment, rather than rounding it the other way.
pub fn pretax(paid: Cents, rate: BasisPoints) -> Result<Cents> {
    const SCALE: i64 = BasisPoints::ONE.0;
    anyhow::ensure!(
        paid >= Cents::ZERO,
        "cannot untax a negative amount: {paid}"
    );
    let denom = SCALE + rate.0;
    anyhow::ensure!(denom > 0, "a tax rate of {rate} leaves no price to recover");
    let num = paid
        .0
        .checked_mul(2 * SCALE)
        .with_context(|| format!("amount is too large to untax: {paid}"))?;
    Ok(Cents((num + denom) / (2 * denom)))
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
    fn pretax_takes_the_tax_back_out_to_the_nearest_cent() {
        let rate = BasisPoints(625);
        // 106.25 is exactly 100 with 6.25% on it.
        assert_eq!(pretax(Cents(10_625), rate).unwrap(), Cents(10_000));
        // 80 / 1.0625 = 75.294...
        assert_eq!(pretax(Cents::from_dollars(80), rate).unwrap(), Cents(7_529));
        // 0.17 / 1.0625 = 0.16 exactly; 0.18 / 1.0625 = 0.1694... rounds up.
        assert_eq!(pretax(Cents(17), rate).unwrap(), Cents(16));
        assert_eq!(pretax(Cents(18), rate).unwrap(), Cents(17));
        assert_eq!(pretax(Cents::ZERO, rate).unwrap(), Cents::ZERO);
        assert_eq!(
            pretax(Cents(4_200), BasisPoints::ZERO).unwrap(),
            Cents(4_200)
        );
    }

    #[test]
    fn pretax_refuses_a_negative_amount() {
        assert!(pretax(Cents(-100), BasisPoints(625)).is_err());
    }

    #[test]
    fn zero_stays_zero() {
        assert_eq!(tax(Cents::ZERO, BasisPoints(625)).unwrap(), Cents::ZERO);
    }
}
