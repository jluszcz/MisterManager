//! Where retirement savings should stand at an age: a multiple of salary, and
//! the share of it held tax-free.
//!
//! Two anchors, the owner's: 3× and 10% by 35; 5–6× and 15–20% by 45. Every
//! other age is read off straight lines through them -- one for each end of
//! each band -- extended past both anchors and floored at zero. No database;
//! `crate::retirement` is what reads one.

use crate::money::Cents;
use crate::rate::BasisPoints;
use std::fmt;

/// The ages the milestone table states, in order.
pub const MILESTONES: [i64; 5] = [35, 40, 45, 50, 55];

const FIRST_AGE: i64 = 35;
const SECOND_AGE: i64 = 45;

/// What separates a band's two ends, in every band either medium prints.
/// Spaced, because unspaced it reads as part of the figures beside it.
pub const BAND_DASH: &str = " – ";

/// A dollar target at the precision it is a target to: `5MM`, `6.5MM`,
/// `340K`. Truncated toward zero to a tenth of a million or a whole thousand,
/// with no `$` -- each medium adds its own, as it does to every figure.
///
/// Here rather than beside a screen, for the band [`fmt::Display`]s' reason.
/// Not one of them because a demo keys its digits on the value, which a
/// `Display` over the band would hand it as one string.
pub fn compact(cents: Cents) -> String {
    let dollars = cents.0 / 100;
    let sign = if dollars < 0 { "-" } else { "" };
    let dollars = dollars.abs();
    if dollars >= 1_000_000 {
        let tenths = dollars / 100_000;
        match tenths % 10 {
            0 => format!("{sign}{}MM", tenths / 10),
            t => format!("{sign}{}.{t}MM", tenths / 10),
        }
    } else if dollars >= 1_000 {
        format!("{sign}{}K", dollars / 1_000)
    } else {
        format!("{sign}{dollars}")
    }
}

/// A multiple of salary in hundredths: `Multiple(340)` is 3.40×.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Multiple(pub i64);

impl Multiple {
    pub fn of(self, salary: Cents) -> Cents {
        Cents(salary.0 * self.0 / 100)
    }

    /// `None` for a salary at or under zero -- a multiple of nothing is not a
    /// figure, and a negative one would read as savings running backwards.
    pub fn between(saved: Cents, salary: Cents) -> Option<Multiple> {
        (salary.0 > 0).then(|| Multiple(saved.0 * 100 / salary.0))
    }
}

impl fmt::Display for Multiple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}×", self.0 / 100, self.0 % 100)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Band<T> {
    pub low: T,
    pub high: T,
}

impl<T> Band<T> {
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Band<U> {
        Band {
            low: f(self.low),
            high: f(self.high),
        }
    }
}

/// On the type rather than beside a screen, for `BasisPoints`' reason: the
/// screen and the report both print a band, and two spellings of one target
/// would read as two targets.
impl fmt::Display for Band<Multiple> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.low == self.high {
            true => write!(f, "{}", self.low),
            false => write!(
                f,
                "{}{BAND_DASH}{}",
                self.low.to_string().trim_end_matches('×'),
                self.high
            ),
        }
    }
}

impl fmt::Display for Band<BasisPoints> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.low == self.high {
            true => write!(f, "{}%", self.low.tenth_percent()),
            false => write!(
                f,
                "{}{BAND_DASH}{}%",
                self.low.tenth_percent(),
                self.high.tenth_percent()
            ),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Status {
    Short,
    OnTrack,
    Ahead,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Short => "Short",
            Status::OnTrack => "On track",
            Status::Ahead => "Ahead",
        }
    }
}

/// At or past the top is `Ahead`, which is what makes a band with no width --
/// 35's -- either met or short, with no "on track" inside a range of one.
pub fn status<T: Ord>(value: T, band: Band<T>) -> Status {
    if value < band.low {
        Status::Short
    } else if value >= band.high {
        Status::Ahead
    } else {
        Status::OnTrack
    }
}

/// The value at `age` on the line through `(35, first)` and `(45, second)`,
/// floored at zero. Every anchor difference here is a multiple of ten, so the
/// divide is exact at any whole age.
fn line(first: i64, second: i64, age: i64) -> i64 {
    (first + (second - first) * (age - FIRST_AGE) / (SECOND_AGE - FIRST_AGE)).max(0)
}

/// Ordered rather than read as `(low line, high line)`: the lines cross at
/// the first anchor, so below 35 the line that ends higher starts lower.
fn band(first: (i64, i64), second: (i64, i64), age: i64) -> Band<i64> {
    let a = line(first.0, second.0, age);
    let b = line(first.1, second.1, age);
    Band {
        low: a.min(b),
        high: a.max(b),
    }
}

pub fn saved_band(age: i64) -> Band<Multiple> {
    band((300, 300), (500, 600), age).map(Multiple)
}

pub fn tax_free_band(age: i64) -> Band<BasisPoints> {
    band((1_000, 1_000), (1_500, 2_000), age).map(BasisPoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_anchors_reproduce_exactly() {
        assert_eq!(
            saved_band(35),
            Band {
                low: Multiple(300),
                high: Multiple(300)
            }
        );
        assert_eq!(
            saved_band(45),
            Band {
                low: Multiple(500),
                high: Multiple(600)
            }
        );
        assert_eq!(
            tax_free_band(35),
            Band {
                low: BasisPoints(1_000),
                high: BasisPoints(1_000)
            }
        );
        assert_eq!(
            tax_free_band(45),
            Band {
                low: BasisPoints(1_500),
                high: BasisPoints(2_000)
            }
        );
    }

    #[test]
    fn forty_is_halfway_between_the_anchors_on_both_lines() {
        assert_eq!(
            saved_band(40),
            Band {
                low: Multiple(400),
                high: Multiple(450)
            }
        );
        assert_eq!(
            tax_free_band(40),
            Band {
                low: BasisPoints(1_250),
                high: BasisPoints(1_500)
            }
        );
    }

    #[test]
    fn past_the_second_anchor_the_lines_carry_on() {
        assert_eq!(
            saved_band(55),
            Band {
                low: Multiple(700),
                high: Multiple(900)
            }
        );
        assert_eq!(
            tax_free_band(55),
            Band {
                low: BasisPoints(2_000),
                high: BasisPoints(3_000)
            }
        );
    }

    /// Below 35 the steeper line is the lower one, so a band read straight
    /// off "low" and "high" would come out inverted.
    #[test]
    fn under_the_first_anchor_the_band_is_still_low_then_high() {
        assert_eq!(
            saved_band(30),
            Band {
                low: Multiple(150),
                high: Multiple(200)
            }
        );
        assert_eq!(
            tax_free_band(30),
            Band {
                low: BasisPoints(500),
                high: BasisPoints(750)
            }
        );
    }

    #[test]
    fn a_line_extended_far_enough_back_floors_at_zero() {
        assert_eq!(
            saved_band(20),
            Band {
                low: Multiple(0),
                high: Multiple(0)
            }
        );
        assert_eq!(
            tax_free_band(20),
            Band {
                low: BasisPoints(0),
                high: BasisPoints(250)
            }
        );
    }

    #[test]
    fn a_figure_below_the_band_is_short_inside_it_on_track_and_at_the_top_ahead() {
        let band = Band { low: 10, high: 20 };
        assert_eq!(status(9, band), Status::Short);
        assert_eq!(status(10, band), Status::OnTrack);
        assert_eq!(status(19, band), Status::OnTrack);
        assert_eq!(status(20, band), Status::Ahead);
    }

    /// 35's band has no width, so there is nothing to be on track *within*.
    #[test]
    fn meeting_a_band_with_no_width_is_ahead_never_on_track() {
        let band = saved_band(35);
        assert_eq!(status(Multiple(300), band), Status::Ahead);
        assert_eq!(status(Multiple(299), band), Status::Short);
    }

    #[test]
    fn a_multiple_is_floored_to_a_hundredth_and_needs_a_positive_salary() {
        let salary = Cents::from_dollars(100_000);
        assert_eq!(
            Multiple::between(Cents::from_dollars(339_999), salary),
            Some(Multiple(339))
        );
        assert_eq!(Multiple::between(Cents::from_dollars(1), Cents::ZERO), None);
        assert_eq!(
            Multiple::between(Cents::from_dollars(1), Cents::from_dollars(-5)),
            None
        );
        assert_eq!(Multiple(340).of(salary), Cents::from_dollars(340_000));
    }

    #[test]
    fn a_band_prints_one_figure_when_it_has_no_width_and_a_range_otherwise() {
        assert_eq!(saved_band(35).to_string(), "3.00×");
        assert_eq!(saved_band(40).to_string(), "4.00 – 4.50×");
        assert_eq!(tax_free_band(35).to_string(), "10.0%");
        assert_eq!(tax_free_band(40).to_string(), "12.5 – 15.0%");
    }

    #[test]
    fn a_compact_figure_truncates_to_a_tenth_of_a_million_or_a_whole_thousand() {
        let compact_of = |d| compact(Cents::from_dollars(d));
        assert_eq!(compact_of(5_075_000), "5MM");
        assert_eq!(compact_of(6_525_000), "6.5MM");
        assert_eq!(compact_of(1_000_000), "1MM");
        assert_eq!(compact_of(999_999), "999K");
        assert_eq!(compact_of(340_000), "340K");
        assert_eq!(compact_of(950), "950");
        assert_eq!(compact_of(-2_500_000), "-2.5MM");
    }
}
