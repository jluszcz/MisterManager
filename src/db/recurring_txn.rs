//! The `recurring_txn` table: rows whose amount and date are known in advance.
//!
//! A much smaller set than "things that happen every month" -- the many past
//! monthlies whose amounts and dates both move are autocomplete's job. Two
//! cadences, matching the data exactly; a weekly or annual arm would ship with
//! no recurring transaction in it and no test exercising it.
//!
//! CRUD, plus the queries regeneration needs. The policy that drives them --
//! horizons, adoption order, what a cadence *is* -- lives in
//! `src/recurring_txn.rs`.

use super::date::{self, iso};
use super::{AccountId, Db, RecurringTxnId};
use crate::money::Cents;
use anyhow::{Context, Result, bail, ensure};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, Row, params};
use std::collections::HashMap;
use std::str::FromStr;

/// How often a recurring transaction comes round.
///
/// The variants are exactly the schema's `CHECK (cadence IN (...))` list:
/// keep the two in step, or an insert that type-checks will fail against the
/// constraint. Adding one means an enum variant, a `CHECK` entry, and a
/// `calc::schedule::Step`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Cadence {
    Biweekly,
    Monthly,
}

impl Cadence {
    pub const ALL: [Cadence; 2] = [Cadence::Biweekly, Cadence::Monthly];

    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Biweekly => "biweekly",
            Cadence::Monthly => "monthly",
        }
    }
}

impl FromStr for Cadence {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "biweekly" => Ok(Cadence::Biweekly),
            "monthly" => Ok(Cadence::Monthly),
            other => bail!("unknown recurring transaction cadence {other:?}"),
        }
    }
}

/// A recurring transaction to be recorded, before it has an id.
///
/// `is_paycheck` is deliberately absent: at most one recurring transaction may
/// carry it, and [`set_paycheck`] is the one place it moves, in a transaction
/// that clears every other recurring transaction first. On the form it would
/// be an invariant with two homes.
#[derive(Clone, Debug)]
pub struct NewRecurringTxn {
    pub description: String,
    /// Signed exactly as the ledger is: the paycheck is positive on a cash
    /// account, the mortgage negative.
    pub cents: Cents,
    pub account_id: AccountId,
    pub cadence: Cadence,
    pub anchor_date: NaiveDate,
    /// The last date this recurring transaction ever generates, or `None` for
    /// a recurring transaction that does not end. Capped further by
    /// `key::RECURRING_TXN_HORIZON_MONTHS` at generation.
    pub horizon: Option<NaiveDate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurringTxn {
    pub id: RecurringTxnId,
    pub description: String,
    pub cents: Cents,
    pub account_id: AccountId,
    pub cadence: Cadence,
    pub anchor_date: NaiveDate,
    pub horizon: Option<NaiveDate>,
    /// How far ahead the owner has asked this one to be written out, when
    /// that is further than the rolling horizon reaches. A floor on
    /// generation, where `horizon` is the cap; `None` until the first `x`.
    pub generate_through: Option<NaiveDate>,
    /// The recurring transaction the ad-hoc projection date is derived from.
    /// At most one.
    pub is_paycheck: bool,
}

// Column order is fixed by `select_recurring_txn!` below -- keep the two in sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<RecurringTxn> {
    let cadence: String = row.get(4)?;
    let anchor: String = row.get(5)?;
    let horizon: Option<String> = row.get(6)?;
    let generate_through: Option<String> = row.get(8)?;
    Ok(RecurringTxn {
        id: row.get(0)?,
        description: row.get(1)?,
        cents: Cents(row.get(2)?),
        account_id: row.get(3)?,
        cadence: cadence
            .parse()
            .expect("schema CHECK guarantees a valid cadence"),
        anchor_date: date::parse(&anchor, 5)?,
        horizon: date::parse_opt(horizon, 6)?,
        generate_through: date::parse_opt(generate_through, 8)?,
        is_paycheck: row.get::<_, i64>(7)? != 0,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. One list per table -- see [`crate::db`] for the
/// idiom.
macro_rules! select_recurring_txn {
    ($tail:literal) => {
        concat!(
            "SELECT id, description, cents, account_id, cadence, anchor_date,
                    horizon, is_paycheck, generate_through
               FROM recurring_txn ",
            $tail
        )
    };
}

pub fn insert(db: &Db, recurring_txn: &NewRecurringTxn) -> Result<RecurringTxnId> {
    db.conn.execute(
        "INSERT INTO recurring_txn (description, cents, account_id, cadence, anchor_date, horizon)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            recurring_txn.description,
            recurring_txn.cents.0,
            recurring_txn.account_id,
            recurring_txn.cadence.as_str(),
            iso(recurring_txn.anchor_date),
            recurring_txn.horizon.map(iso)
        ],
    )?;
    Ok(RecurringTxnId(db.conn.last_insert_rowid()))
}

pub fn list(db: &Db) -> Result<Vec<RecurringTxn>> {
    let mut stmt = db.conn.prepare(select_recurring_txn!("ORDER BY id"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One recurring transaction by id. A missing one is an error, not `None` --
/// the same rule as [`super::account::get`]: an id read off another row is a
/// foreign key, and a dangling one is a corrupt database.
pub fn get(db: &Db, id: RecurringTxnId) -> Result<RecurringTxn> {
    db.conn
        .query_row(
            select_recurring_txn!("WHERE id = ?1"),
            params![id],
            from_row,
        )
        .optional()?
        .with_context(|| format!("no recurring transaction with id {id}"))
}

/// Overwrite a recurring transaction's editable columns.
///
/// **`is_paycheck` is not one of them**: the flag is `P`'s to move, and
/// clearing it here would silently un-derive the ad-hoc projection date the
/// moment the owner corrected a typo in the description.
pub fn update(db: &Db, id: RecurringTxnId, recurring_txn: &NewRecurringTxn) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE recurring_txn
            SET description = ?2, cents = ?3, account_id = ?4,
                cadence = ?5, anchor_date = ?6, horizon = ?7
          WHERE id = ?1",
        params![
            id,
            recurring_txn.description,
            recurring_txn.cents.0,
            recurring_txn.account_id,
            recurring_txn.cadence.as_str(),
            iso(recurring_txn.anchor_date),
            recurring_txn.horizon.map(iso)
        ],
    )?;
    ensure!(changed == 1, "no recurring transaction with id {id}");
    Ok(())
}

/// Delete a recurring transaction, releasing every row it owns.
///
/// `recurring_txn_id = NULL`, not a cascade: deleting a recurring transaction
/// must not silently move a balance, and the released rows become adoptable
/// again by whatever replaces it. `edited` is cleared with it -- the flag
/// means "this recurring transaction-owned row was hand-edited", which says
/// nothing once there is no recurring transaction.
///
/// Returns how many rows were released, for the confirmation's status line.
///
/// **Must be called at top level, not nested inside another transaction.**
/// `Db::transaction` is not reentrant.
pub fn delete(db: &Db, id: RecurringTxnId) -> Result<usize> {
    // Read for the check alone: the release below matches on
    // `recurring_txn_id`, so an unknown id would release nothing, delete
    // nothing, and report a successful deletion of zero rows.
    get(db, id)?;
    db.transaction(|db| {
        let released = db.conn.execute(
            "UPDATE txn SET recurring_txn_id = NULL, edited = 0 WHERE recurring_txn_id = ?1",
            params![id],
        )?;
        db.conn
            .execute("DELETE FROM recurring_txn WHERE id = ?1", params![id])?;
        Ok(released)
    })
}

/// Make this the paycheck recurring transaction, and no other.
///
/// The invariant is enforced by the write rather than checked by the reader:
/// every other recurring transaction's flag is cleared in the same
/// transaction, so two paycheck recurring transactions are unreachable through
/// this API.
///
/// **Must be called at top level, not nested inside another transaction.**
pub fn set_paycheck(db: &Db, id: RecurringTxnId) -> Result<()> {
    // Read for the check alone: without it an unknown id would clear the flag
    // from every row and set it on none, leaving no paycheck at all.
    get(db, id)?;
    db.transaction(|db| {
        db.conn.execute(
            "UPDATE recurring_txn SET is_paycheck = 0 WHERE id <> ?1",
            params![id],
        )?;
        db.conn.execute(
            "UPDATE recurring_txn SET is_paycheck = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    })
}

pub fn paycheck(db: &Db) -> Result<Option<RecurringTxn>> {
    let found = db
        .conn
        .query_row(
            select_recurring_txn!("WHERE is_paycheck = 1 ORDER BY id LIMIT 1"),
            [],
            from_row,
        )
        .optional()?;
    Ok(found)
}

/// How many `txn` rows each recurring transaction owns, at any date.
///
/// The Recurring Transactions screen's last column, and how the owner sees
/// that the first `g` adopted the imported rows instead of duplicating them.
pub fn owned_counts(db: &Db) -> Result<HashMap<RecurringTxnId, i64>> {
    let mut stmt = db
        .conn
        .prepare("SELECT recurring_txn_id, COUNT(*) FROM txn WHERE recurring_txn_id IS NOT NULL GROUP BY recurring_txn_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// Record how far ahead this recurring transaction is to be generated.
///
/// The one writer of `generate_through`: [`update`] deliberately leaves the
/// column alone, the same shape `is_paycheck` already has, because the form
/// that calls it has no field for either.
pub fn set_generate_through(db: &Db, id: RecurringTxnId, through: NaiveDate) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE recurring_txn SET generate_through = ?2 WHERE id = ?1",
        params![id, iso(through)],
    )?;
    ensure!(changed == 1, "no recurring transaction with id {id}");
    Ok(())
}

/// The furthest-dated `txn` row each recurring transaction owns.
///
/// The Recurring Transactions screen's date column. It reads the ledger
/// rather than the schedule, so it says how far the rows actually reach --
/// which is what `x` extends, and what stays `None` until the first `g`.
pub fn last_owned_dates(db: &Db) -> Result<HashMap<RecurringTxnId, NaiveDate>> {
    let mut stmt = db.conn.prepare(
        "SELECT recurring_txn_id, MAX(date) FROM txn
          WHERE recurring_txn_id IS NOT NULL GROUP BY recurring_txn_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let date: String = r.get(1)?;
        Ok((r.get(0)?, date::parse(&date, 1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// Step 1 of regeneration: give back every row this recurring transaction owns
/// from `from` onward that has not been hand-edited.
///
/// History is never rewritten and a corrected row is never clobbered, which is
/// exactly what the `date >= ?` and `edited = 0` halves buy.
pub fn release_generated(db: &Db, id: RecurringTxnId, from: NaiveDate) -> Result<usize> {
    let removed = db.conn.execute(
        "DELETE FROM txn WHERE recurring_txn_id = ?1 AND edited = 0 AND date >= ?2",
        params![id, iso(from)],
    )?;
    Ok(removed)
}

/// Step 1b of regeneration: give back every row this recurring transaction
/// owns on any of `dates`.
///
/// The rows [`release_generated`] left behind are hand-corrections, each
/// standing for one occurrence. A correction whose date no longer matches any
/// occurrence -- the recurring transaction's anchor or cadence moved, or the
/// row itself was moved through the ledger -- stands for nothing, and leaving
/// it owned means the occurrence loop inserts a second row for the schedule's
/// own date: four rows for three occurrences, and every projected balance from
/// there one payment out.
///
/// Released, not deleted, exactly as [`delete`] does it: the row is the
/// owner's hand-correction and clobbering it is never this code's call. What
/// is left is an ordinary ledger row, adoptable again by whatever generates
/// that date next.
pub fn release_dates(db: &Db, id: RecurringTxnId, dates: &[NaiveDate]) -> Result<usize> {
    let mut stmt = db
        .conn
        .prepare("UPDATE txn SET recurring_txn_id = NULL, edited = 0 WHERE recurring_txn_id = ?1 AND date = ?2")?;
    let mut released = 0;
    for date in dates {
        released += stmt.execute(params![id, iso(*date)])?;
    }
    Ok(released)
}

/// Step 2 of regeneration: claim one unclaimed row matching this occurrence.
///
/// Returns whether one was found. `edited` is set when the adopted amount
/// differs from the recurring transaction's -- a pre-entered paycheck of
/// 4,999.99 against the recurring transaction's 5,000.00 is exactly the
/// hand-correction `edited` exists to protect, and adopting it as edited
/// means step 1 never takes it back.
///
/// At most one row, through the subselect: SQLite has no `LIMIT` on `UPDATE`
/// in a default build, and a day carrying two identical rows must not have
/// both claimed by one occurrence.
pub fn adopt(
    db: &Db,
    id: RecurringTxnId,
    date: NaiveDate,
    account_id: AccountId,
    description: &str,
    cents: Cents,
) -> Result<bool> {
    let changed = db.conn.execute(
        "UPDATE txn
            SET recurring_txn_id = ?1, edited = CASE WHEN cents = ?5 THEN 0 ELSE 1 END
          WHERE id = (SELECT id FROM txn
                       WHERE recurring_txn_id IS NULL
                         AND date = ?2
                         AND account_id = ?3
                         AND description = ?4
                       ORDER BY id
                       LIMIT 1)",
        params![id, iso(date), account_id, description, cents.0],
    )?;
    Ok(changed == 1)
}

/// Step 0 of the occurrence loop: every date this recurring transaction
/// already owns, at `from` or later.
///
/// Called after [`release_generated`], so whatever the recurring transaction
/// still owns from that date onward is exactly the set of hand-corrected rows
/// `release_generated` was told not to touch. Each one already accounts for
/// its occurrence -- `regenerate_within` skips generating a second row for the
/// same date rather than asking [`adopt`] to reclaim a row this recurring
/// transaction already holds, which its `recurring_txn_id IS NULL` guard
/// refuses.
pub fn owned_dates(db: &Db, id: RecurringTxnId, from: NaiveDate) -> Result<Vec<NaiveDate>> {
    let mut stmt = db
        .conn
        .prepare("SELECT date FROM txn WHERE recurring_txn_id = ?1 AND date >= ?2 ORDER BY date")?;
    let rows = stmt.query_map(params![id, iso(from)], |row| {
        let raw: String = row.get(0)?;
        date::parse(&raw, 0)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Kind};
    use crate::db::txn::{self, NewTxn};
    use crate::db::{self, TxnId};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn checking(db: &Db) -> AccountId {
        account::insert(db, "CHK", "Everyday", Kind::Cash, 0).unwrap()
    }

    fn paycheck_recurring_txn(account_id: AccountId) -> NewRecurringTxn {
        NewRecurringTxn {
            description: "Salary".to_string(),
            cents: Cents(500_000),
            account_id,
            cadence: Cadence::Biweekly,
            anchor_date: day(2026, 8, 28),
            horizon: Some(day(2026, 12, 18)),
        }
    }

    fn write(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64, what: &str) -> TxnId {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: Cents(cents),
                account_id,
                description: what.to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn cadence_as_str_and_from_str_round_trip() {
        for cadence in Cadence::ALL {
            assert_eq!(cadence.as_str().parse::<Cadence>().unwrap(), cadence);
        }
        assert!("weekly".parse::<Cadence>().is_err());
    }

    /// The enum and the schema's `CHECK (cadence IN (...))` are two
    /// independent lists of the same two strings. A variant missing from the
    /// constraint type-checks and then fails at runtime.
    #[test]
    fn every_cadence_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        for cadence in Cadence::ALL {
            let mut new = paycheck_recurring_txn(account_id);
            new.cadence = cadence;
            insert(&db, &new)
                .unwrap_or_else(|e| panic!("{cadence:?} is not in the schema's CHECK list: {e}"));
        }
    }

    /// Distinct values in every field, so a transposed `select_recurring_txn!` ordering
    /// cannot pass.
    #[test]
    fn insert_and_get_round_trip_every_field() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.description, "Salary");
        assert_eq!(found.cents, Cents(500_000));
        assert_eq!(found.account_id, account_id);
        assert_eq!(found.cadence, Cadence::Biweekly);
        assert_eq!(found.anchor_date, day(2026, 8, 28));
        assert_eq!(found.horizon, Some(day(2026, 12, 18)));
        assert!(
            !found.is_paycheck,
            "a new recurring transaction is never the paycheck"
        );
    }

    /// A recurring transaction that does not end has no horizon, and null must
    /// survive the round trip -- read as a date, it would silently stop
    /// generating.
    #[test]
    fn a_rule_with_no_horizon_round_trips_as_none() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let mut new = paycheck_recurring_txn(account_id);
        new.horizon = None;
        let id = insert(&db, &new).unwrap();
        assert_eq!(get(&db, id).unwrap().horizon, None);
    }

    #[test]
    fn update_rewrites_every_editable_field_and_leaves_the_paycheck_flag_alone() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        set_paycheck(&db, id).unwrap();

        update(
            &db,
            id,
            &NewRecurringTxn {
                description: "Mortgage".to_string(),
                cents: Cents(-120_000),
                account_id: savings,
                cadence: Cadence::Monthly,
                anchor_date: day(2026, 9, 1),
                horizon: None,
            },
        )
        .unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.description, "Mortgage");
        assert_eq!(found.cents, Cents(-120_000));
        assert_eq!(found.account_id, savings);
        assert_eq!(found.cadence, Cadence::Monthly);
        assert_eq!(found.anchor_date, day(2026, 9, 1));
        assert_eq!(found.horizon, None);
        assert!(
            found.is_paycheck,
            "the flag is `P`'s to move, not an edit's"
        );
    }

    /// The invariant is enforced by the write rather than checked by the
    /// reader, so two paycheck recurring transactions must be unreachable
    /// through this API.
    #[test]
    fn setting_the_paycheck_flag_clears_it_everywhere_else() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let first = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let second = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();

        set_paycheck(&db, first).unwrap();
        assert_eq!(paycheck(&db).unwrap().unwrap().id, first);

        set_paycheck(&db, second).unwrap();
        assert_eq!(paycheck(&db).unwrap().unwrap().id, second);
        assert!(!get(&db, first).unwrap().is_paycheck);
    }

    #[test]
    fn there_is_no_paycheck_rule_until_one_is_named() {
        let db = db::open_in_memory().unwrap();
        assert!(paycheck(&db).unwrap().is_none());
        assert!(set_paycheck(&db, RecurringTxnId(999)).is_err());
    }

    /// Deleting a recurring transaction must not silently move a balance, so
    /// its rows are released rather than cascaded. `edited` goes with the
    /// recurring transaction: it means "this recurring transaction-owned row
    /// was hand-edited" and says nothing without one.
    #[test]
    fn deleting_a_rule_releases_its_rows_rather_than_deleting_them() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let owned = write(&db, account_id, day(2026, 8, 28), 500_000, "Salary");
        adopt(
            &db,
            id,
            day(2026, 8, 28),
            account_id,
            "Salary",
            Cents(499_999),
        )
        .unwrap();
        let untouched = write(&db, account_id, day(2026, 8, 10), -5_000, "Whole Foods");

        let released = delete(&db, id).unwrap();

        assert_eq!(released, 1);
        assert!(get(&db, id).is_err());
        assert_eq!(txn::count(&db).unwrap(), 2, "no row was deleted");
        let rows = txn::list(&db, &all_cash()).unwrap();
        let row = rows.iter().find(|t| t.id == owned).unwrap();
        assert_eq!(row.recurring_txn_id, None);
        assert!(!row.edited, "a released row is nobody's correction");
        assert!(rows.iter().any(|t| t.id == untouched));
    }

    fn all_cash() -> txn::Filter {
        txn::Filter {
            kind: Kind::Cash,
            account_id: None,
            from: NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
            to: NaiveDate::from_ymd_opt(2100, 1, 1).unwrap(),
            search: None,
        }
    }

    /// Step 1 of regeneration. History is never rewritten and a corrected row
    /// is never clobbered, so both the past and the edited are left alone.
    #[test]
    fn release_generated_takes_only_unedited_rows_dated_from_today() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();

        for date in [day(2026, 7, 3), day(2026, 8, 28), day(2026, 9, 11)] {
            write(&db, account_id, date, 500_000, "Salary");
            adopt(&db, id, date, account_id, "Salary", Cents(500_000)).unwrap();
        }
        // Hand-correct the 09-11 row.
        let rows = txn::list(&db, &all_cash()).unwrap();
        let corrected = rows.iter().find(|t| t.date == day(2026, 9, 11)).unwrap();
        txn::update(&db, corrected.id, &corrected.clone().into_new()).unwrap();
        // An unadopted future row this recurring transaction does not own --
        // deleting the `recurring_txn_id = ?1` guard would sweep it up too.
        write(&db, account_id, day(2026, 9, 4), -5_000, "Whole Foods");

        let removed = release_generated(&db, id, day(2026, 8, 16)).unwrap();

        assert_eq!(removed, 1, "only the unedited future row");
        let left: Vec<NaiveDate> = txn::list(&db, &all_cash())
            .unwrap()
            .iter()
            .map(|t| t.date)
            .collect();
        assert_eq!(
            left,
            vec![day(2026, 7, 3), day(2026, 9, 4), day(2026, 9, 11)]
        );
    }

    /// A hand-correction on a date the recurring transaction no longer
    /// generates is given back rather than deleted: the row is the owner's,
    /// and only its ownership is wrong.
    #[test]
    fn release_dates_gives_back_only_this_rules_rows_on_the_named_dates() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let other = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        for (recurring_txn_id, date) in [
            (id, day(2026, 8, 28)),
            (id, day(2026, 9, 11)),
            (other, day(2026, 9, 25)),
        ] {
            write(&db, account_id, date, 499_999, "Salary");
            adopt(
                &db,
                recurring_txn_id,
                date,
                account_id,
                "Salary",
                Cents(500_000),
            )
            .unwrap();
        }

        let released = release_dates(&db, id, &[day(2026, 8, 28), day(2026, 9, 25)]).unwrap();

        assert_eq!(
            released, 1,
            "09-25 belongs to the other recurring transaction"
        );
        let rows = txn::list(&db, &all_cash()).unwrap();
        assert_eq!(rows.len(), 3, "no row was deleted");
        let given_back = rows.iter().find(|t| t.date == day(2026, 8, 28)).unwrap();
        assert_eq!(given_back.recurring_txn_id, None);
        assert!(!given_back.edited, "a released row is nobody's correction");
        assert_eq!(
            given_back.cents,
            Cents(499_999),
            "the correction itself survives"
        );
        let kept = rows.iter().find(|t| t.date == day(2026, 9, 11)).unwrap();
        assert_eq!(
            kept.recurring_txn_id,
            Some(id),
            "an unnamed date is left owned"
        );
    }

    /// The workbook already holds every future occurrence as an ordinary row
    /// with a null `recurring_txn_id`. Adoption is what makes the first `g`
    /// idempotent instead of writing a second Mortgage.
    #[test]
    fn adopt_claims_one_unclaimed_row_and_flags_a_mismatched_amount() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        write(&db, account_id, day(2026, 8, 28), 500_000, "Salary");
        write(&db, account_id, day(2026, 9, 11), 499_999, "Salary");

        assert!(
            adopt(
                &db,
                id,
                day(2026, 8, 28),
                account_id,
                "Salary",
                Cents(500_000)
            )
            .unwrap()
        );
        assert!(
            adopt(
                &db,
                id,
                day(2026, 9, 11),
                account_id,
                "Salary",
                Cents(500_000)
            )
            .unwrap()
        );
        // Nothing left to adopt on a date with no matching row.
        assert!(
            !adopt(
                &db,
                id,
                day(2026, 9, 25),
                account_id,
                "Salary",
                Cents(500_000)
            )
            .unwrap()
        );

        let rows = txn::list(&db, &all_cash()).unwrap();
        let matched = rows.iter().find(|t| t.date == day(2026, 8, 28)).unwrap();
        let mismatched = rows.iter().find(|t| t.date == day(2026, 9, 11)).unwrap();
        assert_eq!(matched.recurring_txn_id, Some(id));
        assert!(!matched.edited, "an exact match is not a correction");
        assert_eq!(mismatched.recurring_txn_id, Some(id));
        assert!(
            mismatched.edited,
            "a ledger row of 4,999.99 against the recurring_txn's 5,000.00 is exactly the \
             hand-correction `edited` exists to protect"
        );
    }

    /// A day with two identical rows must not have both claimed by one
    /// occurrence: SQLite has no `LIMIT` on `UPDATE`, so the query has to say
    /// so itself.
    #[test]
    fn adopt_claims_at_most_one_row_per_occurrence() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        write(&db, account_id, day(2026, 8, 28), 500_000, "Salary");
        write(&db, account_id, day(2026, 8, 28), 500_000, "Salary");

        adopt(
            &db,
            id,
            day(2026, 8, 28),
            account_id,
            "Salary",
            Cents(500_000),
        )
        .unwrap();

        let claimed = txn::list(&db, &all_cash())
            .unwrap()
            .iter()
            .filter(|t| t.recurring_txn_id == Some(id))
            .count();
        assert_eq!(claimed, 1);
    }

    /// A row already owned by another recurring transaction, or on another
    /// account, or with another description, is not this occurrence's to take.
    #[test]
    fn adopt_ignores_rows_that_are_not_this_occurrences() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let other = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        write(&db, savings, day(2026, 8, 28), 500_000, "Salary");
        write(&db, account_id, day(2026, 8, 28), 500_000, "Salar");
        // Right account, date, and description, but already claimed by a
        // different recurring transaction -- deleting the `recurring_txn_id IS
        // NULL` guard would let a second `g` steal it.
        write(&db, account_id, day(2026, 8, 28), 500_000, "Salary");
        assert!(
            adopt(
                &db,
                other,
                day(2026, 8, 28),
                account_id,
                "Salary",
                Cents(500_000)
            )
            .unwrap()
        );

        assert!(
            !adopt(
                &db,
                id,
                day(2026, 8, 28),
                account_id,
                "Salary",
                Cents(500_000)
            )
            .unwrap()
        );
    }

    /// A recurring transaction has never been extended until `x` says so, and
    /// the extension is its own column: it must not be confused with the
    /// horizon, which is where the recurring transaction ends.
    #[test]
    fn set_generate_through_records_how_far_a_rule_has_been_extended() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        assert_eq!(get(&db, id).unwrap().generate_through, None);

        set_generate_through(&db, id, day(2027, 6, 1)).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.generate_through, Some(day(2027, 6, 1)));
        assert_eq!(
            found.horizon,
            Some(day(2026, 12, 18)),
            "extending must not move the date the recurring transaction ends"
        );
    }

    /// `update` is the form's write, and the form has no extension field.
    /// Clearing it there would silently pull every extended row back to the
    /// rolling horizon the next time a typo was corrected.
    #[test]
    fn update_leaves_the_extension_alone() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        set_generate_through(&db, id, day(2027, 6, 1)).unwrap();

        let mut edited = paycheck_recurring_txn(account_id);
        edited.description = "Salary Deposit".to_string();
        update(&db, id, &edited).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.description, "Salary Deposit");
        assert_eq!(found.generate_through, Some(day(2027, 6, 1)));
    }

    /// The Recurring Transactions screen's date column: how far out the
    /// ledger actually carries this recurring transaction, which is what the
    /// owner extends when it is not far enough.
    #[test]
    fn last_owned_dates_reports_the_furthest_row_each_rule_holds() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let other = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        for date in [day(2026, 8, 28), day(2026, 7, 3)] {
            write(&db, account_id, date, 500_000, "Salary");
            adopt(&db, id, date, account_id, "Salary", Cents(500_000)).unwrap();
        }

        let last = last_owned_dates(&db).unwrap();
        assert_eq!(last.get(&id), Some(&day(2026, 8, 28)));
        assert_eq!(
            last.get(&other),
            None,
            "a recurring transaction that owns nothing has no last date"
        );
    }

    #[test]
    fn owned_counts_reports_every_row_a_rule_holds_at_any_date() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let other = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        for date in [day(2026, 7, 3), day(2026, 8, 28)] {
            write(&db, account_id, date, 500_000, "Salary");
            adopt(&db, id, date, account_id, "Salary", Cents(500_000)).unwrap();
        }

        let counts = owned_counts(&db).unwrap();
        assert_eq!(counts.get(&id), Some(&2));
        assert_eq!(
            counts.get(&other),
            None,
            "a recurring transaction that owns nothing is absent"
        );
    }

    /// After `release_generated` runs, whatever a recurring transaction still
    /// owns is exactly the hand-corrected rows it was told not to touch --
    /// each one already accounts for its occurrence.
    #[test]
    fn owned_dates_reports_only_this_rules_rows_from_the_given_date_onward() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        let other = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();

        for date in [day(2026, 7, 3), day(2026, 8, 28), day(2026, 9, 11)] {
            write(&db, account_id, date, 500_000, "Salary");
            adopt(&db, id, date, account_id, "Salary", Cents(500_000)).unwrap();
        }
        // Owned by a different recurring transaction -- not this recurring
        // transaction's date to report.
        write(&db, account_id, day(2026, 9, 25), 500_000, "Salary");
        adopt(
            &db,
            other,
            day(2026, 9, 25),
            account_id,
            "Salary",
            Cents(500_000),
        )
        .unwrap();

        let dates = owned_dates(&db, id, day(2026, 8, 16)).unwrap();

        assert_eq!(
            dates,
            vec![day(2026, 8, 28), day(2026, 9, 11)],
            "07-03 is before `from`, 09-25 belongs to the other recurring transaction"
        );
    }

    #[test]
    fn owned_dates_is_empty_for_a_rule_that_owns_nothing() {
        let db = db::open_in_memory().unwrap();
        let account_id = checking(&db);
        let id = insert(&db, &paycheck_recurring_txn(account_id)).unwrap();
        assert!(owned_dates(&db, id, day(2000, 1, 1)).unwrap().is_empty());
    }

    #[test]
    fn list_is_empty_for_a_fresh_database_and_a_missing_rule_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(list(&db).unwrap().is_empty());
        assert!(get(&db, RecurringTxnId(999)).is_err());
        assert!(delete(&db, RecurringTxnId(999)).is_err());
    }
}
