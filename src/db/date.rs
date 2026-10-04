//! The stored date format, in one place.
//!
//! Every date column in `schema.sql` is `TEXT` holding `YYYY-MM-DD` -- the
//! shape a lexicographic `ORDER BY date` and a `BETWEEN` both need to mean
//! what they say. Writing one and reading it back is this module's whole
//! job, so the format literal is stated once rather than beside every query.

use crate::calc::Month;
use chrono::{Datelike, NaiveDate};
use rusqlite::ToSql;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Type, ValueRef};

/// The format every stored date is written in.
const FORMAT: &str = "%Y-%m-%d";

/// A date as a date column holds it.
pub fn iso(date: NaiveDate) -> String {
    date.format(FORMAT).to_string()
}

/// A stored date read back, for a `from_row` reading `column`.
///
/// Text that does not parse is a corrupt database rather than an ordinary
/// miss, so this is an error and never a fallback date. The message quotes
/// the offending text, because the column index says which column but not
/// which row.
pub fn parse(raw: &str, column: usize) -> rusqlite::Result<NaiveDate> {
    NaiveDate::parse_from_str(raw, FORMAT).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            format!("{raw:?} is not a YYYY-MM-DD date: {e}").into(),
        )
    })
}

/// [`parse`] for a nullable column, where `NULL` is a meaningful value.
pub fn parse_opt(raw: Option<String>, column: usize) -> rusqlite::Result<Option<NaiveDate>> {
    raw.as_deref().map(|raw| parse(raw, column)).transpose()
}

/// A month is stored as its first day, in the format every date column has.
impl ToSql for Month {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(iso(self.first_day())))
    }
}

/// Any other day is a corrupt row rather than a month to round down: the
/// schema's `CHECK` should have kept it out, and two spellings of one month
/// under a primary key would be two rows.
impl FromSql for Month {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_str()?;
        let date = NaiveDate::parse_from_str(raw, FORMAT).map_err(|e| {
            FromSqlError::Other(format!("{raw:?} is not a YYYY-MM-DD date: {e}").into())
        })?;
        if date.day() != 1 {
            return Err(FromSqlError::Other(
                format!("{raw:?} is not the first of a month").into(),
            ));
        }
        Ok(Month::of(date))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_month_survives_the_round_trip_through_storage() {
        let db = crate::db::open_in_memory().unwrap();
        let month = Month::of(NaiveDate::from_ymd_opt(2026, 8, 19).unwrap());
        let back: Month = db
            .conn
            .query_row("SELECT ?1", [month], |r| r.get(0))
            .unwrap();
        assert_eq!(back, month);
    }

    #[test]
    fn a_stored_month_that_is_not_a_first_is_refused_on_the_way_back() {
        let db = crate::db::open_in_memory().unwrap();
        let read: rusqlite::Result<Month> =
            db.conn.query_row("SELECT '2026-08-15'", [], |r| r.get(0));
        assert!(read.is_err());
    }

    #[test]
    fn a_date_survives_the_round_trip_through_storage() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 19).unwrap();
        assert_eq!(parse(&iso(date), 0).unwrap(), date);
    }

    #[test]
    fn a_stored_date_is_zero_padded_so_it_sorts_as_text() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        assert_eq!(iso(date), "2026-01-02");
    }

    #[test]
    fn text_that_is_not_a_date_names_itself_in_the_error() {
        let err = parse("19/08/2026", 3).unwrap_err().to_string();
        assert!(err.contains("19/08/2026"), "{err}");
    }

    #[test]
    fn a_null_date_column_parses_as_none() {
        assert_eq!(parse_opt(None, 0).unwrap(), None);
    }

    #[test]
    fn a_nullable_column_holding_a_date_parses_as_that_date() {
        let parsed = parse_opt(Some("2026-08-19".to_string()), 0).unwrap();
        assert_eq!(parsed, NaiveDate::from_ymd_opt(2026, 8, 19));
    }
}
