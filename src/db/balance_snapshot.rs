//! The `balance_snapshot` and `holding_snapshot` tables: what each ledger
//! account and each fund stood at, one row per month.
//!
//! A month is a [`Month`], stored as its first day. What a ledger account's row *means* --
//! its balance on the month's last day, recorded or recovered -- is
//! `crate::balance_history`'s to say; this module only writes and reads rows.
//!
//! **The writers change nothing they would write the same.** Every `mm` run
//! takes a snapshot, and `Db::wrote_rows` is what decides whether the quit
//! path rewrites the report; a write that re-stored an unchanged figure would
//! count as a row changed and make every run look like one that edited
//! something. So the past is an insert that does nothing on a conflict, the
//! present is an upsert whose `WHERE` skips an equal figure, and a delete
//! names only rows that are there to go.

use super::{AccountId, Db};
use crate::calc::Month;
use crate::money::Cents;
use anyhow::Result;
use rusqlite::{Row, params};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceSnapshot {
    pub month: Month,
    pub account_id: AccountId,
    pub cents: Cents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldingSnapshot {
    pub month: Month,
    pub account_id: AccountId,
    pub ticker: String,
    pub cents: Cents,
}

// Column order is fixed by `select_balance_snapshot!` below -- keep the two in
// sync.
fn balance_from_row(row: &Row<'_>) -> rusqlite::Result<BalanceSnapshot> {
    Ok(BalanceSnapshot {
        month: row.get(0)?,
        account_id: row.get(1)?,
        cents: Cents(row.get(2)?),
    })
}

/// A `SELECT` of the columns [`balance_from_row`] reads, in the order it reads
/// them, with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_balance_snapshot {
    ($tail:literal) => {
        concat!(
            "SELECT month, account_id, cents FROM balance_snapshot ",
            $tail
        )
    };
}

// Column order is fixed by `select_holding_snapshot!` below -- keep the two
// in sync.
fn holding_from_row(row: &Row<'_>) -> rusqlite::Result<HoldingSnapshot> {
    Ok(HoldingSnapshot {
        month: row.get(0)?,
        account_id: row.get(1)?,
        ticker: row.get(2)?,
        cents: Cents(row.get(3)?),
    })
}

/// A `SELECT` of the columns [`holding_from_row`] reads, in the order it reads
/// them, with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_holding_snapshot {
    ($tail:literal) => {
        concat!(
            "SELECT month, account_id, ticker, cents FROM holding_snapshot ",
            $tail
        )
    };
}

/// Records `cents` for `account_id` in `month` unless that month already has
/// a figure for it: a month already recorded is what the month was.
pub fn fill_balance(db: &Db, month: Month, account_id: AccountId, cents: Cents) -> Result<()> {
    db.conn.execute(
        // `ON CONFLICT` rather than `OR IGNORE`, which would swallow the
        // month's `CHECK` along with the duplicate.
        "INSERT INTO balance_snapshot (month, account_id, cents) VALUES (?1, ?2, ?3)
         ON CONFLICT (month, account_id) DO NOTHING",
        params![month, account_id, cents.0],
    )?;
    Ok(())
}

/// Makes `month`'s ledger balances exactly `balances`: each one written over
/// whatever the month held for its account, and an account the month held
/// that `balances` does not name removed.
pub fn set_balances(db: &Db, month: Month, balances: &[(AccountId, Cents)]) -> Result<()> {
    for existing in balances_in(db, month)? {
        if !balances.iter().any(|(id, _)| *id == existing.account_id) {
            db.conn.execute(
                "DELETE FROM balance_snapshot WHERE month = ?1 AND account_id = ?2",
                params![month, existing.account_id],
            )?;
        }
    }
    for (account_id, cents) in balances {
        db.conn.execute(
            "INSERT INTO balance_snapshot (month, account_id, cents) VALUES (?1, ?2, ?3)
             ON CONFLICT (month, account_id) DO UPDATE SET cents = excluded.cents
              WHERE cents != excluded.cents",
            params![month, account_id, cents.0],
        )?;
    }
    Ok(())
}

/// Makes `month`'s fund balances exactly `held`, the way [`set_balances`]
/// does the ledger's: a fund sold since the last run leaves the month rather
/// than standing in it at its old figure.
pub fn set_holdings(db: &Db, month: Month, held: &[(AccountId, &str, Cents)]) -> Result<()> {
    for existing in holdings_in(db, month)? {
        let kept = held
            .iter()
            .any(|(id, ticker, _)| *id == existing.account_id && *ticker == existing.ticker);
        if !kept {
            db.conn.execute(
                "DELETE FROM holding_snapshot WHERE month = ?1 AND account_id = ?2 AND ticker = ?3",
                params![month, existing.account_id, existing.ticker],
            )?;
        }
    }
    for (account_id, ticker, cents) in held {
        db.conn.execute(
            "INSERT INTO holding_snapshot (month, account_id, ticker, cents)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (month, account_id, ticker) DO UPDATE SET cents = excluded.cents
              WHERE cents != excluded.cents",
            params![month, account_id, ticker, cents.0],
        )?;
    }
    Ok(())
}

/// Every ledger balance recorded, oldest month first.
pub fn balances(db: &Db) -> Result<Vec<BalanceSnapshot>> {
    let mut stmt = db
        .conn
        .prepare(select_balance_snapshot!("ORDER BY month, account_id"))?;
    let rows = stmt.query_map([], balance_from_row)?;
    super::collect_rows(rows)
}

/// Every fund balance recorded, oldest month first.
pub fn holdings(db: &Db) -> Result<Vec<HoldingSnapshot>> {
    let mut stmt = db.conn.prepare(select_holding_snapshot!(
        "ORDER BY month, account_id, ticker"
    ))?;
    let rows = stmt.query_map([], holding_from_row)?;
    super::collect_rows(rows)
}

fn balances_in(db: &Db, month: Month) -> Result<Vec<BalanceSnapshot>> {
    let mut stmt = db
        .conn
        .prepare(select_balance_snapshot!("WHERE month = ?1"))?;
    let rows = stmt.query_map(params![month], balance_from_row)?;
    super::collect_rows(rows)
}

fn holdings_in(db: &Db, month: Month) -> Result<Vec<HoldingSnapshot>> {
    let mut stmt = db
        .conn
        .prepare(select_holding_snapshot!("WHERE month = ?1"))?;
    let rows = stmt.query_map(params![month], holding_from_row)?;
    super::collect_rows(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, account};
    use crate::test_support::day;

    fn checking(db: &Db) -> AccountId {
        account::insert(db, "CHK", "Everyday", account::Kind::Cash, 0, None).unwrap()
    }

    #[test]
    fn a_filled_month_keeps_the_figure_it_was_first_given() {
        let db = db::open_in_memory().unwrap();
        let chk = checking(&db);
        fill_balance(&db, Month::of(day(2026, 8, 1)), chk, Cents(500)).unwrap();
        fill_balance(&db, Month::of(day(2026, 8, 1)), chk, Cents(900)).unwrap();
        assert_eq!(balances(&db).unwrap()[0].cents, Cents(500));
    }

    #[test]
    fn setting_a_month_overwrites_it_and_drops_an_account_it_no_longer_names() {
        let db = db::open_in_memory().unwrap();
        let chk = checking(&db);
        let sav = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 1, None).unwrap();
        set_balances(
            &db,
            Month::of(day(2026, 8, 1)),
            &[(chk, Cents(500)), (sav, Cents(100))],
        )
        .unwrap();
        set_balances(&db, Month::of(day(2026, 8, 1)), &[(chk, Cents(700))]).unwrap();
        assert_eq!(
            balances(&db).unwrap(),
            vec![BalanceSnapshot {
                month: Month::of(day(2026, 8, 1)),
                account_id: chk,
                cents: Cents(700)
            }]
        );
    }

    #[test]
    fn setting_a_month_leaves_every_other_month_alone() {
        let db = db::open_in_memory().unwrap();
        let chk = checking(&db);
        fill_balance(&db, Month::of(day(2026, 7, 1)), chk, Cents(300)).unwrap();
        set_balances(&db, Month::of(day(2026, 8, 1)), &[]).unwrap();
        assert_eq!(balances(&db).unwrap().len(), 1);
    }

    #[test]
    fn a_fund_sold_since_the_last_snapshot_leaves_the_month() {
        let db = db::open_in_memory().unwrap();
        let brk = account::insert(
            &db,
            "BRK",
            "Holdings",
            account::Kind::Investment,
            0,
            Some(account::TaxTreatment::Taxable),
        )
        .unwrap();
        let month = Month::of(day(2026, 8, 1));
        set_holdings(
            &db,
            month,
            &[(brk, "USM", Cents(1_000)), (brk, "USB", Cents(400))],
        )
        .unwrap();
        set_holdings(&db, month, &[(brk, "USM", Cents(1_200))]).unwrap();
        assert_eq!(
            holdings(&db).unwrap(),
            vec![HoldingSnapshot {
                month,
                account_id: brk,
                ticker: "USM".to_string(),
                cents: Cents(1_200)
            }]
        );
    }

    /// [`Month`] cannot hold any other day, so this is the backstop under it:
    /// a row written around the type.
    #[test]
    fn a_month_that_is_not_a_first_of_the_month_is_refused_by_the_schema() {
        let db = db::open_in_memory().unwrap();
        let chk = checking(&db);
        let written = db.conn.execute(
            "INSERT INTO balance_snapshot (month, account_id, cents) VALUES ('2026-08-15', ?1, 1)",
            [chk],
        );
        assert!(written.is_err());
    }
}
