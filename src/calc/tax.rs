use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result};

/// A price with sales tax on it, rounded up to the next whole dollar.
///
/// Up rather than to the nearest because a target sizes a purchase, and a
/// goal funded a few cents short of the register is short. The whole
/// computation stays in scaled integers so there is no float rounding: `num`
/// is the taxed value scaled by 10,000, which is the scaling `BasisPoints`
/// already carries.
pub fn tax(price: Cents, rate: BasisPoints) -> Result<Cents> {
    const SCALE: i64 = BasisPoints::ONE.0;
    const DOLLAR: i64 = 100;
    let num = price
        .0
        .checked_mul(SCALE + rate.0)
        .with_context(|| format!("price is too large to tax: {price}"))?;
    let units = super::div_ceil(num, SCALE * DOLLAR)?;
    Ok(Cents(units * DOLLAR))
}

/// The pre-tax price of an amount paid with tax in it: `paid / (1 + rate)`,
/// to the nearest cent, half up.
///
/// Not an inverse of [`tax`], which rounds up to a whole dollar: no
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

    #[test]
    fn rounds_up_to_the_next_whole_dollar_at_every_size() {
        let d = Cents::from_dollars;
        let rate = BasisPoints(625);
        assert_eq!(tax(d(5), rate).unwrap(), d(6)); // 5.3125
        assert_eq!(tax(d(48), rate).unwrap(), d(51)); // exactly 51.00
        assert_eq!(tax(d(100), rate).unwrap(), d(107)); // 106.25
        // One step at every size: $500 and up rounds to the dollar too.
        assert_eq!(tax(d(475), rate).unwrap(), d(505)); // 504.6875
        assert_eq!(tax(d(700), rate).unwrap(), d(744)); // 743.75
        assert_eq!(tax(d(1_000), rate).unwrap(), d(1_063)); // 1062.50
        assert_eq!(tax(d(1_200), rate).unwrap(), d(1_275)); // exactly 1275.00
        assert_eq!(tax(d(12_000), rate).unwrap(), d(12_750)); // exactly 12750.00
        assert_eq!(tax(d(12_001), rate).unwrap(), d(12_752)); // 12751.0625
    }

    /// A base with cents in it rounds up past them rather than to them.
    #[test]
    fn a_price_with_cents_still_lands_on_a_whole_dollar() {
        assert_eq!(tax(Cents(1), BasisPoints(625)).unwrap(), Cents(100));
        assert_eq!(
            tax(Cents(10_001), BasisPoints::ZERO).unwrap(),
            Cents(10_100)
        );
        assert_eq!(
            tax(Cents(10_000), BasisPoints::ZERO).unwrap(),
            Cents(10_000)
        );
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
