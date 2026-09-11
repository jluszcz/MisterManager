//! The Funds screen's inputs: the birth date and the international-equity
//! split, fed to `calc::fund`.
//!
//! The shape of `plan.rs` — reads settings out of `db`, hands plain values to
//! `calc`, hands the result back up.

use crate::calc::fund as calc_fund;
use crate::db::Db;
use crate::db::setting::{self, key};
use crate::rate::BasisPoints;
use anyhow::Result;
use chrono::NaiveDate;

/// The international share of the equity remainder, when
/// [`key::INTL_EQUITY_SHARE`] is unset.
///
/// A database nobody has imported into yet is a real state, not a
/// misconfiguration: the sheet's own split is what an import writes, and 40%
/// is what it carries.
pub const DEFAULT_INTL_EQUITY_SHARE: BasisPoints = BasisPoints(4_000);

/// Read the birth date and the equity split, and derive the three targets.
pub fn targets_from_db(db: &Db, today: NaiveDate) -> Result<calc_fund::Targets> {
    let age = setting::get(db, key::BIRTH_DATE)?.map(|birth| calc_fund::whole_years(birth, today));
    // Unset is a real state: a database nobody has imported into yet. The
    // sheet's own split is what an import writes, and 40% is what it carries.
    let intl = setting::get(db, key::INTL_EQUITY_SHARE)?.unwrap_or(DEFAULT_INTL_EQUITY_SHARE);
    Ok(calc_fund::targets(age, intl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::test_support::day;
    use chrono::Datelike;

    /// A birth date is personal data, so tests derive one from the day they
    /// are asking about rather than writing one down.
    fn born_years_before(today: NaiveDate, years: i32) -> NaiveDate {
        today.with_year(today.year() - years).unwrap()
    }

    #[test]
    fn the_targets_are_derived_from_the_stored_birth_date_and_split() {
        let today = day(2026, 8, 18);
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::BIRTH_DATE, born_years_before(today, 48)).unwrap();
        setting::set(&db, key::INTL_EQUITY_SHARE, BasisPoints(4_000)).unwrap();

        let targets = targets_from_db(&db, today).unwrap();

        assert_eq!(targets.bonds, Some(BasisPoints(1_800)));
        assert_eq!(targets.intl_stock, BasisPoints(3_280));
        assert_eq!(targets.us_stock, BasisPoints(4_920));
    }

    /// An import writes the equity split the sheet carries; a database
    /// nobody has imported into yet has not, and 40% is what stands in.
    #[test]
    fn an_unset_equity_split_defaults_to_the_sheets_own_forty_percent() {
        let today = day(2026, 8, 18);
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::BIRTH_DATE, born_years_before(today, 48)).unwrap();

        let targets = targets_from_db(&db, today).unwrap();

        assert_eq!(targets.intl_stock, BasisPoints(3_280));
        assert_eq!(targets.us_stock, BasisPoints(4_920));
    }

    /// Unset is a question for the screen to ask, not a zero to assume.
    #[test]
    fn an_unset_birth_date_leaves_the_bond_target_unknown_rather_than_zero() {
        let today = day(2026, 8, 18);
        let db = db::open_in_memory().unwrap();

        let targets = targets_from_db(&db, today).unwrap();

        assert_eq!(targets.bonds, None);
        assert_eq!(targets.intl_stock, DEFAULT_INTL_EQUITY_SHARE);
        assert_eq!(targets.us_stock, BasisPoints(6_000));
    }
}
