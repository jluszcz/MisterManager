pub mod business_day;
pub mod fund;
mod interest;
mod paycheck;
pub mod planning;
pub mod schedule;
mod tax;

use anyhow::{Result, ensure};

pub use interest::pro_rata;
pub use paycheck::{
    biweekly, fit, month_end_projection, per_paycheck, per_paycheck_over_years, period_days,
};
pub use tax::tax;

/// Ceiling division for positive divisors, correct for negative dividends.
///
/// `div_euclid` floors toward negative infinity and `rem_euclid` is never
/// negative, so adding one whenever there is a remainder gives the ceiling.
/// `(a + b - 1) / b` is wrong here: Rust's `/` truncates toward zero, so it
/// under-reports for negative `a`, and it can overflow near `i64::MAX`.
///
/// A non-positive divisor is an error rather than a `debug_assert!`. Every
/// divisor here traces back to a user-editable setting, and an assertion
/// that compiles out of release builds would leave the real deployment
/// dividing by zero -- the one build where it matters.
pub(crate) fn div_ceil(a: i64, b: i64) -> Result<i64> {
    ensure!(b > 0, "divisor must be positive, got {b}");
    let quotient = a.div_euclid(b);
    Ok(if a.rem_euclid(b) == 0 {
        quotient
    } else {
        quotient + 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div_ceil_exact_division() {
        assert_eq!(div_ceil(10, 5).unwrap(), 2);
    }

    #[test]
    fn div_ceil_remainder_rounds_up() {
        assert_eq!(div_ceil(11, 5).unwrap(), 3);
    }

    #[test]
    fn div_ceil_negative_dividends() {
        // Negative dividend that divides evenly
        assert_eq!(div_ceil(-42, 14).unwrap(), -3);
        // Negative dividend with remainder
        assert_eq!(div_ceil(-41, 14).unwrap(), -2);
    }

    /// A non-positive divisor must be refused in every build, release
    /// included. A debug-only assertion would compile out of the one build
    /// that matters and leave it dividing by zero.
    #[test]
    fn div_ceil_rejects_a_non_positive_divisor() {
        let err = div_ceil(10, 0).unwrap_err();
        assert!(err.to_string().contains("got 0"), "{err}");
        assert!(div_ceil(10, -5).is_err());
    }
}
