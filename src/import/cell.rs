use super::SheetRange;
use crate::money::Cents;
use crate::rate::{BasisPoints, Percent};
use calamine::Data;
use chrono::NaiveDate;

/// One cell, with everything past the sheet's used range reading as blank.
///
/// `calamine` answers `None` off the end of the range, and every reader below
/// already turns `Data::Empty` into `None`, so the two absences are the same
/// absence to a caller. Collapsing them here is what lets a block scan run to
/// `range.height()` without checking whether each column reaches that far.
pub fn at(range: &SheetRange, row: usize, col: usize) -> Data {
    range.get((row, col)).cloned().unwrap_or(Data::Empty)
}

/// A monetary cell. Excel stores these as f64; every real amount in the
/// workbook is exact to the cent, so rounding at 2dp is lossless here.
pub fn as_cents(value: &Data) -> Option<Cents> {
    match value {
        Data::Float(f) => Some(Cents((f * 100.0).round() as i64)),
        Data::Int(i) => Some(Cents(i * 100)),
        _ => None,
    }
}

/// A percentage cell (`Constants!E2` = 0.0625) as basis points.
pub fn as_rate_bp(value: &Data) -> Option<BasisPoints> {
    match value {
        Data::Float(f) => Some(BasisPoints((f * 10_000.0).round() as i64)),
        Data::Int(i) => Some(BasisPoints(i * 10_000)),
        _ => None,
    }
}

pub fn as_date(value: &Data) -> Option<NaiveDate> {
    match value {
        // `ExcelDateTime::as_datetime` requires calamine's `chrono` feature,
        // which this crate does not enable. `to_ymd_hms_milli` has no such
        // gate, so it works with calamine's default features.
        Data::DateTime(dt) => {
            let (year, month, day, ..) = dt.to_ymd_hms_milli();
            NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
        }
        Data::DateTimeIso(s) => {
            let prefix = s.get(..10)?;
            NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok()
        }
        _ => None,
    }
}

pub fn as_text(value: &Data) -> Option<String> {
    match value {
        Data::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

/// A fraction cell expressed as whole percent: `0.35` becomes `35`.
///
/// Accepts `Int` as well as `Float`, because a cell edited to a whole number
/// round-trips as an integer and would otherwise be read as absent — which,
/// combined with a default, fails silently.
pub fn as_percent(value: &Data) -> Option<Percent> {
    match value {
        Data::Float(f) => Some(Percent((f * 100.0).round() as i64)),
        Data::Int(i) => Some(Percent(i * 100)),
        _ => None,
    }
}

pub fn as_i64(value: &Data) -> Option<i64> {
    match value {
        Data::Int(i) => Some(*i),
        Data::Float(f) if f.fract() == 0.0 => Some(*f as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Cents;
    use calamine::{Cell, Data, ExcelDateTime, ExcelDateTimeType, Range};
    use chrono::NaiveDate;

    #[test]
    fn a_cell_beyond_the_used_range_reads_as_blank() {
        // A range anchored at A1 holding one value at B2.
        let range: SheetRange = Range::from_sparse(vec![
            Cell::new((0, 0), Data::Empty),
            Cell::new((1, 1), Data::Float(2.5)),
        ]);

        assert_eq!(at(&range, 1, 1), Data::Float(2.5));
        assert_eq!(at(&range, 0, 0), Data::Empty);
        assert_eq!(at(&range, 99, 99), Data::Empty);
    }

    #[test]
    fn floats_become_exact_cents() {
        // Individual workbook amounts are clean to the cent even though sums
        // carry float dust.
        assert_eq!(as_cents(&Data::Float(-4500.85)), Some(Cents(-450_085)));
        assert_eq!(as_cents(&Data::Float(5000.35)), Some(Cents(500_035)));
        assert_eq!(as_cents(&Data::Float(2.4)), Some(Cents(240)));
        assert_eq!(as_cents(&Data::Int(-140)), Some(Cents(-14_000)));
        assert_eq!(as_cents(&Data::Empty), None);
        assert_eq!(as_cents(&Data::String("n/a".into())), None);
    }

    #[test]
    fn percentages_survive_as_basis_points() {
        // Constants!E2 holds 0.0625 as a raw float.
        assert_eq!(as_rate_bp(&Data::Float(0.0625)), Some(BasisPoints(625)));
        assert_eq!(as_rate_bp(&Data::Float(0.0)), Some(BasisPoints::ZERO));
    }

    #[test]
    fn dates_come_back_as_naive_dates() {
        let dt = ExcelDateTime::new(46246.0, ExcelDateTimeType::DateTime, false);
        assert_eq!(
            as_date(&Data::DateTime(dt)),
            NaiveDate::from_ymd_opt(2026, 8, 12)
        );
        assert_eq!(as_date(&Data::Empty), None);
    }

    #[test]
    fn iso_datetime_strings_parse_their_date_prefix() {
        assert_eq!(
            as_date(&Data::DateTimeIso("2026-08-12T00:00:00".to_string())),
            NaiveDate::from_ymd_opt(2026, 8, 12)
        );
    }

    #[test]
    fn iso_datetime_strings_that_do_not_parse_return_none_not_a_panic() {
        // Garbage that is at least 10 bytes long: the slice succeeds but the
        // date parse fails. Must return None, not propagate a parse error.
        assert_eq!(as_date(&Data::DateTimeIso("not-a-date".to_string())), None);
    }

    #[test]
    fn iso_datetime_strings_shorter_than_ten_bytes_do_not_panic() {
        // A naive `&s[..10]` slice would panic here: the string is only 5
        // bytes long, so byte index 10 is out of bounds.
        assert_eq!(as_date(&Data::DateTimeIso("2026".to_string())), None);
    }

    #[test]
    fn iso_datetime_strings_do_not_panic_on_a_multi_byte_boundary() {
        // Nine ASCII digits then a two-byte 'é': byte index 10 lands on the
        // second byte of 'é', which is not a char boundary. A naive
        // `&s[..10]` slice panics here; `String::get` must return None
        // instead.
        let s = "123456789é-08-2026";
        assert!(!s.is_char_boundary(10), "test fixture must hit a boundary");
        assert_eq!(as_date(&Data::DateTimeIso(s.to_string())), None);
    }

    #[test]
    fn text_trims_and_rejects_blanks() {
        assert_eq!(
            as_text(&Data::String("  Transfer  ".into())),
            Some("Transfer".to_string())
        );
        assert_eq!(as_text(&Data::String("   ".into())), None);
        assert_eq!(as_text(&Data::Empty), None);
    }

    #[test]
    fn fractions_read_as_whole_percent_from_either_representation() {
        assert_eq!(as_percent(&Data::Float(0.35)), Some(Percent(35)));
        assert_eq!(as_percent(&Data::Float(0.5)), Some(Percent(50)));
        // A cell edited to a whole number arrives as an integer.
        assert_eq!(as_percent(&Data::Int(0)), Some(Percent::ZERO));
        assert_eq!(as_percent(&Data::Empty), None);
    }

    #[test]
    fn integers_read_from_either_representation() {
        assert_eq!(as_i64(&Data::Int(26)), Some(26));
        assert_eq!(as_i64(&Data::Float(26.0)), Some(26));
        assert_eq!(as_i64(&Data::Float(26.4)), None);
    }
}
