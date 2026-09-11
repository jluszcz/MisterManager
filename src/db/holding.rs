//! The `holding` table: the balance the owner types for one fund in one
//! investment account.
//!
//! What each fund is *made of* is `fund_mix`'s, not this table's -- a
//! holding is where a balance sits, and a mix is what it is composed of, and
//! the two are looked up by ticker rather than carried together so that one
//! fetch of a fund's composition prices every account holding it.
//!
//! **A ticker arrives here already normalised to uppercase**, and this module
//! takes it as given rather than folding case itself. `UNIQUE (account_id,
//! ticker)`, [`update`]'s duplicate guard and `fund_mix`'s lookup by ticker
//! all compare the string exactly, so `usm` and `USM` would be two holdings,
//! two tickers and two compositions. `tui::fund::HoldingForm::commit` is where
//! that normalisation happens, being the only writer; a second writer owes the
//! same thing before it calls [`insert`] or [`update`].

use super::account::{self, Kind};
use super::{AccountId, Db, HoldingId};
use crate::money::Cents;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{OptionalExtension, Row, params};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holding {
    pub id: HoldingId,
    pub account_id: AccountId,
    pub ticker: String,
    pub balance: Cents,
    pub sort: i64,
}

// Column order is fixed by `select_holding!` below -- keep the two in sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<Holding> {
    Ok(Holding {
        id: row.get(0)?,
        account_id: row.get(1)?,
        ticker: row.get(2)?,
        balance: Cents(row.get(3)?),
        sort: row.get(4)?,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_holding {
    ($tail:literal) => {
        concat!(
            "SELECT id, account_id, ticker, balance_cents, sort FROM holding ",
            $tail
        )
    };
}

/// Records a balance in `ticker`, held in `account_id`.
///
/// Refuses an account that is not [`Kind::Investment`], naming it in the
/// message -- the guard the schema cannot express, since a `CHECK` sees only
/// the row being written and not the account it names.
/// `UNIQUE (account_id, ticker)` is the backstop for the other mistake this
/// could be: one ticker twice in one account is a typo, and the constraint is
/// what catches it once the kind is already right.
pub fn insert(db: &Db, account_id: AccountId, ticker: &str, balance: Cents) -> Result<HoldingId> {
    let owner = account::get(db, account_id)?;
    ensure!(
        owner.kind == Kind::Investment,
        "{} is not an investment account, so it cannot hold a fund",
        // The Funds screen puts this on its status line verbatim, so it
        // reaches the mask here rather than through `account_label::Account`.
        crate::demo::text(owner.name.as_str())
    );
    db.conn.execute(
        "INSERT INTO holding (account_id, ticker, balance_cents) VALUES (?1, ?2, ?3)",
        params![account_id, ticker, balance.0],
    )?;
    Ok(HoldingId(db.conn.last_insert_rowid()))
}

/// One holding by id. A missing holding is an error, not `None` -- the same
/// rule as [`super::account::get`]: an id read off another row is a foreign
/// key, and a dangling one is a corrupt database.
pub fn get(db: &Db, id: HoldingId) -> Result<Holding> {
    db.conn
        .query_row(select_holding!("WHERE id = ?1"), params![id], from_row)
        .optional()?
        .with_context(|| format!("no holding with id {id}"))
}

/// Every holding, across every account.
pub fn list(db: &Db) -> Result<Vec<Holding>> {
    let mut stmt = db
        .conn
        .prepare(select_holding!("ORDER BY account_id, sort, id"))?;
    let rows = stmt.query_map([], from_row)?;
    super::collect_rows(rows)
}

/// One account's holdings, in the order the Funds screen lists them.
pub fn list_for_account(db: &Db, account_id: AccountId) -> Result<Vec<Holding>> {
    let mut stmt = db
        .conn
        .prepare(select_holding!("WHERE account_id = ?1 ORDER BY sort, id"))?;
    let rows = stmt.query_map(params![account_id], from_row)?;
    super::collect_rows(rows)
}

/// Every ticker held anywhere, once each -- what the fetcher's refresh reads
/// to learn which funds to ask SEC about.
pub fn tickers(db: &Db) -> Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT DISTINCT ticker FROM holding ORDER BY ticker")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    super::collect_rows(rows)
}

/// Rewrites a holding's account, ticker and balance -- the three fields the
/// Funds screen's editor has a box for. Where it sorts among its account's
/// holdings is not the edit's to change.
///
/// Refuses the same two ways [`insert`] does, for the same reasons: an
/// account that is not [`Kind::Investment`], and a ticker the destination
/// account already holds under a different id. The second is `insert`'s
/// `UNIQUE (account_id, ticker)` reached from a new direction -- moving a
/// holding into an account can collide with one already there, and a raw
/// constraint violation on the status line is not a sentence a person can
/// act on, so it is checked and refused here rather than left to the
/// constraint.
pub fn update(
    db: &Db,
    id: HoldingId,
    account_id: AccountId,
    ticker: &str,
    balance: Cents,
) -> Result<()> {
    let owner = account::get(db, account_id)?;
    ensure!(
        owner.kind == Kind::Investment,
        "{} is not an investment account, so it cannot hold a fund",
        // The Funds screen puts this on its status line verbatim, so it
        // reaches the mask here rather than through `account_label::Account`.
        crate::demo::text(owner.name.as_str())
    );
    if list_for_account(db, account_id)?
        .iter()
        .any(|h| h.ticker == ticker && h.id != id)
    {
        bail!(
            "{} already holds {}",
            crate::demo::text(owner.name.as_str()),
            crate::demo::text(ticker)
        );
    }
    let changed = db.conn.execute(
        "UPDATE holding SET account_id = ?2, ticker = ?3, balance_cents = ?4 WHERE id = ?1",
        params![id, account_id, ticker, balance.0],
    )?;
    ensure!(changed == 1, "no holding with id {id}");
    Ok(())
}

pub fn delete(db: &Db, id: HoldingId) -> Result<()> {
    let removed = db
        .conn
        .execute("DELETE FROM holding WHERE id = ?1", params![id])?;
    ensure!(removed == 1, "no holding with id {id}");
    Ok(())
}

/// Moves a holding to `position` among its account's holdings, and renumbers
/// `sort` over all of them so the column stays `0..n-1`.
///
/// The same construction as [`super::account::reorder`], scoped to the
/// holding's account rather than to a kind: a position rather than a raw
/// `sort`, because `sort` is only ever read through an `ORDER BY` that breaks
/// ties by id, so "put it third" has a result that does not depend on rows
/// the caller never saw. A position past the end lands last, the same answer
/// a drag past the bottom of a list gives.
pub fn reorder(db: &Db, id: HoldingId, position: usize) -> Result<()> {
    db.transaction(|db| {
        let holding = get(db, id)?;
        let mut ordered = list_for_account(db, holding.account_id)?;
        let from = ordered
            .iter()
            .position(|h| h.id == id)
            .expect("the holding was just read by id, so its account lists it");
        let moved = ordered.remove(from);
        ordered.insert(position.min(ordered.len()), moved);
        for (sort, holding) in ordered.iter().enumerate() {
            db.conn.execute(
                "UPDATE holding SET sort = ?2 WHERE id = ?1",
                params![holding.id, sort as i64],
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Kind, TaxTreatment};

    fn account(db: &Db) -> crate::db::AccountId {
        account::insert(
            db,
            "RET",
            "Long Haul",
            Kind::Investment,
            0,
            Some(TaxTreatment::TaxFree),
        )
        .unwrap()
    }

    #[test]
    fn one_ticker_may_be_held_in_two_accounts() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();

        insert(&db, first, "USM", Cents(100_000)).unwrap();
        assert!(
            insert(&db, second, "USM", Cents(250_000)).is_ok(),
            "a ticker held in a second account was refused"
        );
    }

    #[test]
    fn one_ticker_twice_in_one_account_is_refused() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);

        insert(&db, id, "USM", Cents(100_000)).unwrap();
        assert!(
            insert(&db, id, "USM", Cents(250_000)).is_err(),
            "the same ticker was inserted twice into one account"
        );
    }

    #[test]
    fn a_holding_in_a_cash_account_is_refused() {
        let db = crate::db::open_in_memory().unwrap();
        let cash = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0, None).unwrap();

        assert!(
            insert(&db, cash, "USM", Cents(100_000)).is_err(),
            "a holding was written against a cash account"
        );
    }

    /// The guard names the account, since the Funds screen puts the message
    /// on its status line verbatim -- an owner told "not an investment
    /// account" with no name has been told which row failed but not why to
    /// look there.
    #[test]
    fn the_cash_account_refusal_names_the_account() {
        let db = crate::db::open_in_memory().unwrap();
        let cash = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0, None).unwrap();

        let err = insert(&db, cash, "USM", Cents(100_000)).unwrap_err();
        assert!(err.to_string().contains("Everyday"), "{err}");
    }

    /// Distinct values in every field, so a transposed `select_holding!`
    /// ordering cannot pass.
    #[test]
    fn insert_and_get_round_trip_every_field() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);

        let holding_id = insert(&db, id, "USM", Cents(100_000)).unwrap();

        let found = get(&db, holding_id).unwrap();
        assert_eq!(found.id, holding_id);
        assert_eq!(found.account_id, id);
        assert_eq!(found.ticker, "USM");
        assert_eq!(found.balance, Cents(100_000));
        assert_eq!(
            found.sort, 0,
            "a fresh holding takes the schema's default sort"
        );
    }

    #[test]
    fn list_covers_every_account_and_list_for_account_covers_only_one() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        insert(&db, first, "USM", Cents(100_000)).unwrap();
        insert(&db, first, "USB", Cents(50_000)).unwrap();
        insert(&db, second, "ISM", Cents(25_000)).unwrap();

        assert_eq!(list(&db).unwrap().len(), 3);
        let tickers: Vec<String> = list_for_account(&db, first)
            .unwrap()
            .into_iter()
            .map(|h| h.ticker)
            .collect();
        assert_eq!(tickers, vec!["USM", "USB"]);
    }

    /// The fetcher's refresh reads this to learn which funds to ask SEC
    /// about, so a ticker held in two accounts must be named once, not twice.
    #[test]
    fn tickers_lists_each_distinct_ticker_once() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        insert(&db, first, "USM", Cents(100_000)).unwrap();
        insert(&db, second, "USM", Cents(50_000)).unwrap();
        insert(&db, first, "USB", Cents(25_000)).unwrap();

        assert_eq!(tickers(&db).unwrap(), vec!["USB", "USM"]);
    }

    #[test]
    fn update_rewrites_the_ticker_and_balance() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        let holding_id = insert(&db, id, "USM", Cents(100_000)).unwrap();

        update(&db, holding_id, id, "USB", Cents(75_000)).unwrap();

        let found = get(&db, holding_id).unwrap();
        assert_eq!(found.ticker, "USB");
        assert_eq!(found.balance, Cents(75_000));
        assert_eq!(found.account_id, id, "the account was not asked to move");
    }

    /// The Funds screen's `e` edits all three fields the form asks for,
    /// account included -- a holding filed against the wrong account has no
    /// other way back.
    #[test]
    fn update_moves_a_holding_to_another_account() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        let holding_id = insert(&db, first, "USM", Cents(100_000)).unwrap();

        update(&db, holding_id, second, "USM", Cents(100_000)).unwrap();

        let found = get(&db, holding_id).unwrap();
        assert_eq!(found.account_id, second);
        assert!(
            list_for_account(&db, first).unwrap().is_empty(),
            "the holding is still listed under the account it left"
        );
        assert_eq!(
            list_for_account(&db, second).unwrap().len(),
            1,
            "the holding did not reach the account it moved to"
        );
    }

    /// `UNIQUE (account_id, ticker)` reached from `update`'s direction: an
    /// owner correcting which account a holding sits in must be told why in
    /// a sentence, not shown the constraint's own error.
    #[test]
    fn update_refuses_a_move_into_an_account_that_already_holds_the_ticker() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        let moving = insert(&db, first, "USM", Cents(100_000)).unwrap();
        insert(&db, second, "USM", Cents(50_000)).unwrap();

        let err = update(&db, moving, second, "USM", Cents(100_000)).unwrap_err();
        assert!(err.to_string().contains("USM"), "{err}");

        assert_eq!(
            get(&db, moving).unwrap().account_id,
            first,
            "the refused move must not have partly landed"
        );
    }

    /// The same guard `insert` applies: a holding cannot be moved onto a
    /// cash or credit account.
    #[test]
    fn update_refuses_a_non_investment_destination_account() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        let holding_id = insert(&db, id, "USM", Cents(100_000)).unwrap();
        let cash = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0, None).unwrap();

        let err = update(&db, holding_id, cash, "USM", Cents(100_000)).unwrap_err();
        assert!(err.to_string().contains("Everyday"), "{err}");
    }

    #[test]
    fn deleting_a_holding_removes_only_that_row() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        let keep = insert(&db, id, "USM", Cents(100_000)).unwrap();
        let gone = insert(&db, id, "USB", Cents(50_000)).unwrap();

        delete(&db, gone).unwrap();

        assert!(get(&db, gone).is_err());
        assert!(get(&db, keep).is_ok());
    }

    /// `reorder` takes a position and renumbers the whole account, so what
    /// the screen shows is what is stored -- no ties for the id to break.
    #[test]
    fn reorder_moves_a_holding_and_renumbers_its_account() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        let usm = insert(&db, id, "USM", Cents(100_000)).unwrap();
        let usb = insert(&db, id, "USB", Cents(50_000)).unwrap();
        let ism = insert(&db, id, "ISM", Cents(25_000)).unwrap();

        reorder(&db, ism, 0).unwrap();

        let ordered = list_for_account(&db, id).unwrap();
        assert_eq!(
            ordered.iter().map(|h| h.id).collect::<Vec<_>>(),
            vec![ism, usm, usb]
        );
        assert_eq!(
            ordered.iter().map(|h| h.sort).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    /// One account's order is not another's: renumbering the first account's
    /// holdings must leave the second account's exactly where they were.
    #[test]
    fn reorder_leaves_other_accounts_holdings_alone() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            1,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        insert(&db, first, "USM", Cents(100_000)).unwrap();
        insert(&db, first, "USB", Cents(50_000)).unwrap();
        let a = insert(&db, second, "ISM", Cents(25_000)).unwrap();
        let b = insert(&db, second, "ISB", Cents(10_000)).unwrap();

        let usm = list_for_account(&db, first).unwrap()[0].id;
        reorder(&db, usm, 1).unwrap();

        let second_ids: Vec<_> = list_for_account(&db, second)
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(second_ids, vec![a, b]);
    }

    #[test]
    fn a_position_past_the_end_lands_last() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        let usm = insert(&db, id, "USM", Cents(100_000)).unwrap();
        let usb = insert(&db, id, "USB", Cents(50_000)).unwrap();

        reorder(&db, usm, 99).unwrap();

        let ids: Vec<_> = list_for_account(&db, id)
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(ids, vec![usb, usm]);
    }

    #[test]
    fn getting_updating_deleting_or_reordering_a_missing_holding_is_an_error() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);
        assert!(get(&db, HoldingId(999)).is_err());
        assert!(update(&db, HoldingId(999), id, "USM", Cents::ZERO).is_err());
        assert!(delete(&db, HoldingId(999)).is_err());
        assert!(reorder(&db, HoldingId(999), 0).is_err());
    }

    #[test]
    fn list_and_tickers_are_empty_for_a_fresh_database() {
        let db = crate::db::open_in_memory().unwrap();
        assert!(list(&db).unwrap().is_empty());
        assert!(tickers(&db).unwrap().is_empty());
    }
}
