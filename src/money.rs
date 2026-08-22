use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub};
use std::str::FromStr;

/// A monetary amount in integer cents. The only money type in the crate.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Cents(pub i64);

impl Cents {
    pub const ZERO: Cents = Cents(0);

    pub fn from_dollars(d: i64) -> Cents {
        Cents(d * 100)
    }

    /// Whole dollars, truncated toward negative infinity.
    pub fn dollars(self) -> i64 {
        self.0.div_euclid(100)
    }

    /// Round down to a whole dollar. `Planning` pins the excess with this.
    pub fn floor_to_dollar(self) -> Cents {
        Cents(self.dollars() * 100)
    }

    /// Grouped dollars with the cents dropped rather than rounded: `500.23`
    /// and `200.99` both print as their own dollar figure.
    ///
    /// Dropping the digits truncates toward zero, unlike [`Cents::dollars`],
    /// which floors -- this renders what is there rather than computing with
    /// it, so `-200.99` reads `-200`, the same figure as `200.99` with a sign.
    pub fn to_whole_dollars(self) -> String {
        let abs = self.0.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        format!("{sign}{}", grouped(abs / 100))
    }
}

/// A whole-dollar figure with thousands separators.
fn grouped(dollars: u64) -> String {
    let digits = dollars.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl Add for Cents {
    type Output = Cents;
    fn add(self, rhs: Cents) -> Cents {
        Cents(self.0 + rhs.0)
    }
}

impl Sub for Cents {
    type Output = Cents;
    fn sub(self, rhs: Cents) -> Cents {
        Cents(self.0 - rhs.0)
    }
}

impl Neg for Cents {
    type Output = Cents;
    fn neg(self) -> Cents {
        Cents(-self.0)
    }
}

impl AddAssign for Cents {
    fn add_assign(&mut self, rhs: Cents) {
        self.0 += rhs.0;
    }
}

impl Sum for Cents {
    fn sum<I: Iterator<Item = Cents>>(iter: I) -> Cents {
        Cents(iter.map(|c| c.0).sum())
    }
}

impl fmt::Display for Cents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.0.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        write!(f, "{sign}{}.{:02}", grouped(abs / 100), abs % 100)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("not a monetary amount: {0:?}")]
pub struct ParseMoneyError(String);

impl FromStr for Cents {
    type Err = ParseMoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseMoneyError(s.to_string());
        let cleaned: String = s
            .chars()
            .filter(|c| !matches!(c, '$' | ',' | '_' | ' '))
            .collect();
        let (negative, body) = match cleaned.strip_prefix('-') {
            Some(rest) => (true, rest.to_string()),
            None => (false, cleaned),
        };
        if body.is_empty() {
            return Err(err());
        }
        let (whole, frac) = match body.split_once('.') {
            Some((w, f)) => (w, f),
            None => (body.as_str(), ""),
        };
        if frac.len() > 2 || whole.contains('.') || frac.contains('.') {
            return Err(err());
        }
        if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
            return Err(err());
        }
        if whole.is_empty() && frac.is_empty() {
            return Err(err());
        }
        let whole: i64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| err())?
        };
        let frac: i64 = match frac.len() {
            0 => 0,
            1 => frac.parse::<i64>().map_err(|_| err())? * 10,
            _ => frac.parse().map_err(|_| err())?,
        };
        let value = whole
            .checked_mul(100)
            .and_then(|v| v.checked_add(frac))
            .ok_or_else(err)?;
        Ok(Cents(if negative { -value } else { value }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_thousands_separators() {
        assert_eq!(Cents(4200099).to_string(), "42,000.99");
        assert_eq!(Cents(-987654).to_string(), "-9,876.54");
        assert_eq!(Cents(0).to_string(), "0.00");
        assert_eq!(Cents(7).to_string(), "0.07");
        assert_eq!(Cents(123456789).to_string(), "1,234,567.89");
    }

    #[test]
    fn parses_the_shapes_a_human_types() {
        assert_eq!("42000.99".parse::<Cents>().unwrap(), Cents(4200099));
        assert_eq!("$42,000.99".parse::<Cents>().unwrap(), Cents(4200099));
        assert_eq!("-4500.85".parse::<Cents>().unwrap(), Cents(-450085));
        assert_eq!("140".parse::<Cents>().unwrap(), Cents(14000));
        assert_eq!("2.4".parse::<Cents>().unwrap(), Cents(240));
        assert_eq!(".5".parse::<Cents>().unwrap(), Cents(50));
    }

    /// The multiply is on a figure a human typed, so a long enough run of
    /// digits reaches it: unchecked, release builds wrap to a negative and
    /// hand it to whichever write the field feeds.
    #[test]
    fn rejects_a_figure_too_large_for_cents_rather_than_wrapping() {
        assert!("92233720368547759".parse::<Cents>().is_err());
        assert!("92233720368547758.99".parse::<Cents>().is_err());
        assert!("-92233720368547759".parse::<Cents>().is_err());
        assert_eq!(
            "92233720368547758.07".parse::<Cents>().unwrap(),
            Cents(i64::MAX)
        );
    }

    #[test]
    fn rejects_junk() {
        assert!("".parse::<Cents>().is_err());
        assert!("abc".parse::<Cents>().is_err());
        assert!("1.234".parse::<Cents>().is_err());
        assert!("1.2.3".parse::<Cents>().is_err());
    }

    #[test]
    fn whole_dollars_drop_the_cents_rather_than_rounding() {
        assert_eq!(Cents(500_000).to_whole_dollars(), "5,000");
        assert_eq!(Cents(50_023).to_whole_dollars(), "500");
        assert_eq!(Cents(20_099).to_whole_dollars(), "200");
        assert_eq!(Cents(7).to_whole_dollars(), "0");
        assert_eq!(Cents(123_456_789).to_whole_dollars(), "1,234,567");
    }

    /// Dropping the digits truncates toward zero, so a negative is the
    /// positive figure with a sign -- not `floor_to_dollar`'s next step down.
    #[test]
    fn whole_dollars_truncate_a_negative_toward_zero() {
        assert_eq!(Cents(-20_099).to_whole_dollars(), "-200");
        assert_eq!(Cents(-987_654).to_whole_dollars(), "-9,876");
    }

    #[test]
    fn floor_to_dollar_truncates_toward_negative_infinity() {
        assert_eq!(Cents(1750075).floor_to_dollar(), Cents(1750000));
        assert_eq!(Cents(1750000).floor_to_dollar(), Cents(1750000));
        assert_eq!(Cents(-150).floor_to_dollar(), Cents(-200));
    }
}
