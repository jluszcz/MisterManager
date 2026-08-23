//! The Funds screen's inputs: the `fund` table and the birth date, fed to
//! `calc::fund`.
//!
//! The shape of `plan.rs` — reads settings and rows out of `db`, hands plain
//! values to `calc`, hands the result back up. This is also the one place a
//! stored `db::fund::Target` becomes a `calc::fund::Rule`, so `calc` never
//! learns that `age_over_30` is a string in a column and `db` never learns
//! what the string means.

use crate::calc::fund as calc_fund;
use crate::db::fund::{self, Target};
use crate::db::setting::{self, key};
use crate::db::{Db, FundId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::Result;
use chrono::NaiveDate;

/// One fund, with what the table holds and what the derivation makes of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FundRow {
    pub id: FundId,
    pub name: String,
    pub actual: Cents,
    /// `None` for an age row with no birth date on record.
    pub target: Option<BasisPoints>,
    pub actual_share: BasisPoints,
    pub delta: Option<BasisPoints>,
}

/// Every fund, derived.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Allocation {
    pub rows: Vec<FundRow>,
    pub total: Cents,
    pub target_total: BasisPoints,
    pub furthest_down: Option<usize>,
    /// `None` when `setting::key::BIRTH_DATE` is unset, which is what the
    /// screen's one-field prompt exists to fill.
    pub age: Option<i64>,
}

/// The one place a stored target becomes a derivation rule.
fn rule(target: Target) -> calc_fund::Rule {
    match target {
        Target::AgeOver30 => calc_fund::Rule::AgeOver30,
        Target::RemainderShare(share) => calc_fund::Rule::RemainderShare(share),
    }
}

/// Read the funds and the birth date, and derive every column the screen
/// shows.
///
/// `today` is the caller's, so a test can ask what the table looks like on a
/// birthday without touching the clock.
pub fn compute_from_db(db: &Db, today: NaiveDate) -> Result<Allocation> {
    let stored = fund::list(db)?;
    let age = setting::get(db, key::BIRTH_DATE)?.map(|birth| calc_fund::whole_years(birth, today));

    let rows: Vec<calc_fund::Row> = stored
        .iter()
        .map(|f| calc_fund::Row {
            rule: rule(f.target),
            actual: f.actual,
        })
        .collect();
    let computed = calc_fund::compute(&rows, age);

    Ok(Allocation {
        rows: stored
            .iter()
            .zip(computed.rows)
            .map(|(stored, derived)| FundRow {
                id: stored.id,
                name: stored.name.clone(),
                actual: stored.actual,
                target: derived.target,
                actual_share: derived.actual,
                delta: derived.delta,
            })
            .collect(),
        total: computed.total,
        target_total: computed.target_total,
        furthest_down: computed.furthest_down,
        age,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::fund::NewFund;
    use crate::test_support::day;
    use chrono::Datelike;

    /// A birth date is personal data, so tests derive one from the day they
    /// are asking about rather than writing one down.
    fn born_years_before(today: NaiveDate, years: i32) -> NaiveDate {
        today.with_year(today.year() - years).unwrap()
    }

    /// Three invented funds, under a birth date the test fixes rather than
    /// reads.
    fn seeded(today: NaiveDate) -> db::Db {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::BIRTH_DATE, born_years_before(today, 40)).unwrap();
        for (name, ord, target, dollars) in [
            ("Bonds", 0, Target::AgeOver30, 30_000),
            (
                "International",
                1,
                Target::RemainderShare(BasisPoints(4_000)),
                60_000,
            ),
            (
                "Domestic",
                2,
                Target::RemainderShare(BasisPoints(6_000)),
                90_000,
            ),
        ] {
            fund::insert(
                &db,
                &NewFund {
                    name: name.to_string(),
                    ord,
                    target,
                    actual: Cents::from_dollars(dollars),
                },
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn the_rows_come_back_in_table_order_with_their_derived_columns() {
        let today = day(2026, 8, 18);
        let allocation = compute_from_db(&seeded(today), today).unwrap();

        let names: Vec<&str> = allocation.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Bonds", "International", "Domestic"]);

        let targets: Vec<Option<BasisPoints>> = allocation.rows.iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            vec![
                Some(BasisPoints(1_000)),
                Some(BasisPoints(3_600)),
                Some(BasisPoints(5_400))
            ]
        );
        assert_eq!(allocation.rows[2].delta, Some(BasisPoints(400)));
        assert_eq!(allocation.total, Cents::from_dollars(180_000));
        assert_eq!(allocation.furthest_down, Some(2));
        assert_eq!(allocation.age, Some(40));
    }

    /// Each row keeps its own id: the screen edits and deletes by it.
    #[test]
    fn each_row_carries_the_id_of_the_fund_it_came_from() {
        let today = day(2026, 8, 18);
        let db = seeded(today);
        let ids: Vec<_> = fund::list(&db).unwrap().iter().map(|f| f.id).collect();

        let allocation = compute_from_db(&db, today).unwrap();

        assert_eq!(
            allocation.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            ids
        );
    }

    /// The age moves with the calendar and nothing is written when it does,
    /// which is why the target percentage is derived rather than stored.
    #[test]
    fn the_age_advances_on_the_birthday_with_no_write() {
        let today = day(2026, 8, 18);
        let db = seeded(today);
        let day_before = compute_from_db(&db, day(2027, 8, 17)).unwrap();
        let birthday = compute_from_db(&db, day(2027, 8, 18)).unwrap();

        assert_eq!(day_before.age, Some(40));
        assert_eq!(birthday.age, Some(41));
        assert_eq!(day_before.rows[0].target, Some(BasisPoints(1_000)));
        assert_eq!(birthday.rows[0].target, Some(BasisPoints(1_100)));
    }

    /// Unset is a question for the screen to ask, not a zero to assume.
    #[test]
    fn an_unset_birth_date_leaves_the_age_unknown_rather_than_zero() {
        let today = day(2026, 8, 18);
        let db = seeded(today);
        setting::clear(&db, key::BIRTH_DATE).unwrap();

        let allocation = compute_from_db(&db, today).unwrap();

        assert_eq!(allocation.age, None);
        assert_eq!(allocation.rows[0].target, None);
        assert_eq!(
            allocation.rows[1].target,
            Some(BasisPoints(4_000)),
            "the share rows divide the whole 100% while the age row claims nothing"
        );
    }

    #[test]
    fn a_database_with_no_funds_computes_an_empty_allocation() {
        let today = day(2026, 8, 18);
        let allocation = compute_from_db(&db::open_in_memory().unwrap(), today).unwrap();

        assert!(allocation.rows.is_empty());
        assert_eq!(allocation.total, Cents::ZERO);
        assert_eq!(allocation.target_total, BasisPoints::ZERO);
        assert_eq!(allocation.furthest_down, None);
    }
}
