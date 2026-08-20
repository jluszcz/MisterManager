use super::date::{self, iso};
use super::{AccountId, Db, RecurringTxnId, TxnId};
use crate::db::account::Kind;
use crate::money::Cents;
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use rusqlite::{Row, params};

#[derive(Clone, Debug)]
pub struct NewTxn {
    pub date: NaiveDate,
    pub cents: Cents,
    pub account_id: AccountId,
    pub description: String,
    pub recurring_txn_id: Option<RecurringTxnId>,
}

/// Escape a user-typed string for a `LIKE` pattern used with `ESCAPE '\'`.
///
/// A `%` or `_` the user typed is a literal. Unescaped, a single `%` in the
/// search box matches the whole ledger, which reads as a broken search rather
/// than as a wildcard doing its job.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A stored transaction, read back rather than summed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Txn {
    pub id: TxnId,
    pub date: NaiveDate,
    pub cents: Cents,
    pub account_id: AccountId,
    pub description: String,
    /// The recurring transaction that generated this row, if any. Null for
    /// hand-entered rows.
    pub recurring_txn_id: Option<RecurringTxnId>,
    /// Set once a recurring transaction-generated row has been hand-edited.
    /// See `update`.
    pub edited: bool,
}

// Column order is fixed by `select_txn!` below -- keep the two in sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<Txn> {
    let date: String = row.get(1)?;
    Ok(Txn {
        id: row.get(0)?,
        date: date::parse(&date, 1)?,
        cents: Cents(row.get(2)?),
        account_id: row.get(3)?,
        description: row.get(4)?,
        recurring_txn_id: row.get(5)?,
        edited: row.get::<_, i64>(6)? != 0,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. One list per table -- see [`crate::db`] for the
/// idiom.
macro_rules! select_txn {
    ($tail:literal) => {
        concat!(
            "SELECT t.id, t.date, t.cents, t.account_id, t.description,
                    t.recurring_txn_id, t.edited
               FROM txn t ",
            $tail
        )
    };
}

/// One ledger screen's slice of `txn`.
#[derive(Clone, Debug)]
pub struct Filter {
    /// Which ledger: Cash or Credit.
    pub kind: Kind,
    /// One account, or every account of `kind`.
    pub account_id: Option<AccountId>,
    /// Oldest date shown, inclusive.
    pub from: NaiveDate,
    /// Newest date shown, inclusive. A window is built from the first and
    /// last day of a month, so both bounds have to be inclusive.
    pub to: NaiveDate,
    /// Description substring. `%` and `_` in it are literals.
    pub search: Option<String>,
}

/// The rows one ledger screen shows, oldest first.
pub fn list(db: &Db, filter: &Filter) -> Result<Vec<Txn>> {
    let mut stmt = db.conn.prepare(select_txn!(
        "JOIN account a ON a.id = t.account_id
          WHERE a.kind = ?1
            AND t.date BETWEEN ?2 AND ?3
            AND (?4 IS NULL OR t.account_id = ?4)
            AND (?5 IS NULL OR t.description LIKE ?5 ESCAPE '\\')
          ORDER BY t.date, t.id"
    ))?;
    let search = filter
        .search
        .as_deref()
        .map(|s| format!("%{}%", like_escape(s)));
    let rows = stmt.query_map(
        params![
            filter.kind.as_str(),
            iso(filter.from),
            iso(filter.to),
            filter.account_id,
            search
        ],
        from_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The oldest and newest dates any row carries, across both kinds.
///
/// `None` for an empty ledger, which has no range to clamp `[` and `]`
/// against.
pub fn date_range(db: &Db) -> Result<Option<(NaiveDate, NaiveDate)>> {
    let (min, max): (Option<String>, Option<String>) =
        db.conn
            .query_row("SELECT MIN(date), MAX(date) FROM txn", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
    let (Some(min), Some(max)) = (min, max) else {
        return Ok(None);
    };
    Ok(Some((date::parse(&min, 0)?, date::parse(&max, 1)?)))
}

impl Txn {
    /// The editable half of a stored row, for handing back to `update`.
    ///
    /// Drops `id` and `edited`, which `update` owns, and carries
    /// `recurring_txn_id` only so the struct is complete — `update` ignores
    /// it.
    pub fn into_new(self) -> NewTxn {
        NewTxn {
            date: self.date,
            cents: self.cents,
            account_id: self.account_id,
            description: self.description,
            recurring_txn_id: self.recurring_txn_id,
        }
    }
}

/// Overwrite a transaction's editable columns, flagging it `edited` when it
/// came from a recurring transaction.
///
/// **`txn.recurring_txn_id` is ignored**: the row keeps whatever recurring
/// transaction generated it. Nothing in the UI carries a recurring transaction
/// id, so honoring the field would detach every edited row from its recurring
/// transaction — precisely what `edited` exists to prevent.
///
/// `edited` is what lets stage 5's regeneration tell "a row I generated" from
/// "a row I generated and you then fixed": it deletes and rewrites `WHERE
/// recurring_txn_id = ? AND edited = 0 AND date >= ?`. Without the flag,
/// regeneration either clobbers corrections or refuses to touch anything and
/// stops being useful. Rows with a null `recurring_txn_id` are unaffected —
/// nothing regenerates them.
pub fn update(db: &Db, id: TxnId, txn: &NewTxn) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE txn
            SET date = ?2, cents = ?3, account_id = ?4, description = ?5,
                edited = CASE WHEN recurring_txn_id IS NULL THEN edited ELSE 1 END
          WHERE id = ?1",
        params![
            id,
            iso(txn.date),
            txn.cents.0,
            txn.account_id,
            txn.description
        ],
    )?;
    ensure!(changed == 1, "no transaction with id {id}");
    Ok(())
}

pub fn delete(db: &Db, id: TxnId) -> Result<()> {
    let removed = db
        .conn
        .execute("DELETE FROM txn WHERE id = ?1", params![id])?;
    ensure!(removed == 1, "no transaction with id {id}");
    Ok(())
}

/// Every account's balance on or before `date`, in `account::list` order.
///
/// Joins outward from `account` rather than grouping over `txn`: a `GROUP BY`
/// silently omits an account with no rows in range, and an account missing
/// from Overview reads as an account that does not exist rather than one at
/// zero.
///
/// Three of these — one per column — is what Overview costs, instead of one
/// query per account per column.
pub fn balances_at(db: &Db, date: NaiveDate) -> Result<Vec<(AccountId, Cents)>> {
    let mut stmt = db.conn.prepare(
        "SELECT a.id,
                COALESCE((SELECT SUM(t.cents) FROM txn t
                           WHERE t.account_id = a.id AND t.date <= ?1), 0)
           FROM account a
          ORDER BY a.kind, a.sort, a.code",
    )?;
    let rows = stmt.query_map(params![iso(date)], |r| Ok((r.get(0)?, Cents(r.get(1)?))))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn insert(db: &Db, txn: &NewTxn) -> Result<TxnId> {
    db.conn.execute(
        "INSERT INTO txn (date, cents, account_id, description, recurring_txn_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            iso(txn.date),
            txn.cents.0,
            txn.account_id,
            txn.description,
            txn.recurring_txn_id
        ],
    )?;
    Ok(TxnId(db.conn.last_insert_rowid()))
}

/// How many rows the ledger holds, at any date.
pub fn count(db: &Db) -> Result<i64> {
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))?;
    Ok(n)
}

/// The ledger sum on or before `date` -- the app's `SUMIFS`.
///
/// This is the raw stored sum. Credit accounts store debt as positive, so
/// callers that present a balance negate it, exactly as `Overview` does.
pub fn balance_at(db: &Db, account_id: AccountId, date: NaiveDate) -> Result<Cents> {
    let sum: i64 = db.conn.query_row(
        "SELECT COALESCE(SUM(cents), 0) FROM txn WHERE account_id = ?1 AND date <= ?2",
        params![account_id, iso(date)],
        |r| r.get(0),
    )?;
    Ok(Cents(sum))
}

/// One account's rows on one date.
pub fn on_date(db: &Db, account_id: AccountId, date: NaiveDate) -> Result<Vec<Txn>> {
    let mut stmt = db.conn.prepare(select_txn!(
        "WHERE t.account_id = ?1 AND t.date = ?2 ORDER BY t.id"
    ))?;
    let rows = stmt.query_map(params![account_id, iso(date)], from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn balance_at_by_kind(db: &Db, kind: Kind, date: NaiveDate) -> Result<Cents> {
    let sum: i64 = db.conn.query_row(
        "SELECT COALESCE(SUM(t.cents), 0)
           FROM txn t JOIN account a ON a.id = t.account_id
          WHERE a.kind = ?1 AND t.date <= ?2",
        params![kind.as_str(), iso(date)],
        |r| r.get(0),
    )?;
    Ok(Cents(sum))
}

/// The all-time ledger sum, ignoring dates. `Savings!B1` uses an undated
/// `SUMIF`, so container reconciliation must too.
pub fn balance_all_time(db: &Db, account_id: AccountId) -> Result<Cents> {
    let sum: i64 = db.conn.query_row(
        "SELECT COALESCE(SUM(cents), 0) FROM txn WHERE account_id = ?1",
        params![account_id],
        |r| r.get(0),
    )?;
    Ok(Cents(sum))
}

/// Move `cents` (a positive magnitude) from one account to the other, writing
/// both legs inside a single SQL transaction.
///
/// The transaction is the whole point: a failure partway through would commit
/// one leg and silently misstate that account's balance forever. The two rows
/// are deliberately *not* linked afterwards — no balance query would read such
/// a link, so it would buy only joint delete.
pub fn insert_transfer(
    db: &Db,
    from_account_id: AccountId,
    to_account_id: AccountId,
    date: NaiveDate,
    cents: Cents,
    from_description: &str,
    to_description: &str,
) -> Result<()> {
    db.transaction(|db| {
        write_transfer(
            db,
            from_account_id,
            to_account_id,
            date,
            cents,
            from_description,
            to_description,
        )
    })
}

/// Both legs of a transfer, without a transaction around them.
///
/// **Must be called inside a `Db::transaction`, and never opens one.** That
/// is what lets a payday write several transfers and several withdrawals
/// atomically: `Db::transaction` is not reentrant, so a batch cannot be built
/// out of calls that each open their own. `db::clear_imported_data` carries
/// the same contract for the same reason.
///
/// Each leg is signed from **its own account's kind**, not from the
/// destination's alone. Cash is signed naturally and credit is signed as
/// debt, so "value left here" and "value arrived here" are opposite signs on
/// the two ledgers:
///
/// | from → to | from leg | to leg | |
/// |---|---|---|---|
/// | cash → cash | `-` | `+` | money moves |
/// | cash → credit | `-` | `-` | a card payment: cash out, debt shed |
/// | credit → cash | `+` | `+` | a cash advance: debt taken on, cash in |
/// | credit → credit | `+` | `-` | a balance transfer: debt moves |
///
/// Every row of that table nets to zero change in net worth, which is what
/// makes it a transfer. Signing the source leg unconditionally negative --
/// correct only for a cash source -- silently inflates net worth by twice the
/// amount whenever the source is a card.
pub fn write_transfer(
    db: &Db,
    from_account_id: AccountId,
    to_account_id: AccountId,
    date: NaiveDate,
    cents: Cents,
    from_description: &str,
    to_description: &str,
) -> Result<()> {
    // Checked, not asserted: the sign convention is applied below per each
    // account's kind, so a negative magnitude here writes both legs the
    // wrong way round, and a `debug_assert!` would let exactly that through
    // in a release build.
    ensure!(
        cents >= Cents::ZERO,
        "write_transfer expects a positive magnitude, got {cents}"
    );

    let is_credit = |id: AccountId| -> rusqlite::Result<bool> {
        let kind: String =
            db.conn
                .query_row("SELECT kind FROM account WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })?;
        Ok(kind == "credit")
    };
    let credit_source = is_credit(from_account_id)?;
    let credit_destination = is_credit(to_account_id)?;

    insert(
        db,
        &NewTxn {
            date,
            // Cash source pays out; credit source funds the move by taking
            // on debt.
            cents: if credit_source { cents } else { -cents },
            account_id: from_account_id,
            description: from_description.to_string(),
            recurring_txn_id: None,
        },
    )?;
    insert(
        db,
        &NewTxn {
            date,
            // Cash destination receives; credit destination sheds debt.
            cents: if credit_destination { -cents } else { cents },
            account_id: to_account_id,
            description: to_description.to_string(),
            recurring_txn_id: None,
        },
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub description: String,
    pub account_id: AccountId,
    pub cents: Cents,
    pub uses: i64,
}

/// Descriptions starting with `prefix`, most-used first, ties broken by most
/// recent. The account and amount come from the most recent use, so the entry
/// form can prefill them.
pub fn autocomplete(db: &Db, prefix: &str, limit: i64) -> Result<Vec<Suggestion>> {
    // SQLite's bare-column rule: with MAX(date) in the select list, the bare
    // columns come from the row that produced that maximum.
    let mut stmt = db.conn.prepare(
        "SELECT description, account_id, cents, COUNT(*) AS uses, MAX(date) AS last_used
           FROM txn
          WHERE description LIKE ?1 ESCAPE '\\'
          GROUP BY description
          ORDER BY uses DESC, last_used DESC
          LIMIT ?2",
    )?;
    let pattern = format!("{}%", like_escape(prefix));
    let rows = stmt.query_map(params![pattern, limit], |r| {
        Ok(Suggestion {
            description: r.get(0)?,
            account_id: r.get(1)?,
            cents: Cents(r.get(2)?),
            uses: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Kind};
    use crate::money::Cents;
    use chrono::NaiveDate;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn fixture() -> (Db, AccountId, AccountId, AccountId) {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        (db, checking, savings, card_one)
    }

    fn add(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64, desc: &str) {
        insert(
            db,
            &NewTxn {
                date,
                cents: Cents(cents),
                account_id,
                description: desc.to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn balance_at_sums_rows_on_or_before_the_date() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2025, 12, 31), 920_784, "End of Year");
        add(&db, checking, day(2026, 1, 1), -4_500, "Phone");
        // A future row must not count toward the earlier date.
        add(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");

        assert_eq!(
            balance_at(&db, checking, day(2026, 1, 1)).unwrap(),
            Cents(916_284)
        );
        assert_eq!(
            balance_at(&db, checking, day(2025, 12, 30)).unwrap(),
            Cents::ZERO
        );
        assert_eq!(
            balance_at(&db, checking, day(2026, 9, 1)).unwrap(),
            Cents(796_284)
        );
    }

    #[test]
    fn balance_all_time_ignores_dates_but_not_accounts() {
        let (db, checking, savings, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100_000, "past");
        // A far-future row must still count -- balance_all_time is undated,
        // unlike balance_at.
        add(&db, checking, day(2099, 1, 1), 50_000, "future");
        // A different account must not bleed in.
        add(&db, savings, day(2026, 1, 1), 999_999, "other account");

        assert_eq!(balance_all_time(&db, checking).unwrap(), Cents(150_000));
    }

    #[test]
    fn balance_at_by_kind_sums_across_accounts() {
        let (db, checking, savings, card_one) = fixture();
        add(&db, checking, day(2026, 1, 1), 100_000, "a");
        add(&db, savings, day(2026, 1, 1), 50_000, "b");
        add(&db, card_one, day(2026, 1, 1), 7_000, "charge");

        assert_eq!(
            balance_at_by_kind(&db, Kind::Cash, day(2026, 1, 1)).unwrap(),
            Cents(150_000)
        );
        // Credit is stored as debt: positive is a charge. Callers negate.
        assert_eq!(
            balance_at_by_kind(&db, Kind::Credit, day(2026, 1, 1)).unwrap(),
            Cents(7_000)
        );
    }

    #[test]
    fn a_transfer_writes_both_legs() {
        let (db, checking, savings, _) = fixture();
        insert_transfer(
            &db,
            checking,
            savings,
            day(2026, 8, 31),
            Cents(250_000),
            "Transfer",
            "Transfer",
        )
        .unwrap();

        assert_eq!(
            balance_at(&db, checking, day(2026, 8, 31)).unwrap(),
            Cents(-250_000)
        );
        assert_eq!(
            balance_at(&db, savings, day(2026, 8, 31)).unwrap(),
            Cents(250_000)
        );

        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    /// A cash advance: the card funds the move, so its debt *grows*. Signing
    /// the source leg negative because it is the source -- correct only for
    /// cash -- sheds debt and hands over cash at the same time, inventing
    /// twice the amount out of nothing.
    #[test]
    fn a_transfer_out_of_a_card_adds_debt_rather_than_shedding_it() {
        let (db, checking, _, card_one) = fixture();
        insert_transfer(
            &db,
            card_one,
            checking,
            day(2026, 8, 31),
            Cents(50_000),
            "Cash advance",
            "Cash advance",
        )
        .unwrap();

        // Credit is stored as debt, so positive is more owed.
        assert_eq!(
            balance_at(&db, card_one, day(2026, 8, 31)).unwrap(),
            Cents(50_000)
        );
        assert_eq!(
            balance_at(&db, checking, day(2026, 8, 31)).unwrap(),
            Cents(50_000)
        );
    }

    /// A balance transfer moves debt between cards: one owes more, the other
    /// less. Both legs negative would shed debt twice.
    #[test]
    fn a_transfer_between_two_cards_moves_debt_instead_of_erasing_it() {
        let (db, _, _, card_one) = fixture();
        let card_two = account::insert(&db, "CC2", "Card Two", Kind::Credit, 1).unwrap();
        insert_transfer(
            &db,
            card_one,
            card_two,
            day(2026, 8, 31),
            Cents(120_000),
            "Balance transfer",
            "Balance transfer",
        )
        .unwrap();

        assert_eq!(
            balance_at(&db, card_one, day(2026, 8, 31)).unwrap(),
            Cents(120_000),
            "the card the debt moved off should owe more, not less"
        );
        assert_eq!(
            balance_at(&db, card_two, day(2026, 8, 31)).unwrap(),
            Cents(-120_000)
        );
    }

    /// The invariant behind every one of the four cases: a transfer moves
    /// value, it does not create it. Net worth is cash minus debt, and it must
    /// be identical either side of any transfer between any two accounts.
    ///
    /// This is the test that fails loudly if a leg's sign is ever keyed off
    /// the wrong account -- a bug that leaves both rows looking individually
    /// plausible and only shows up in the total.
    #[test]
    fn no_transfer_between_any_two_accounts_changes_net_worth() {
        let net = |db: &Db| {
            balance_at_by_kind(db, Kind::Cash, day(2026, 12, 31)).unwrap()
                - balance_at_by_kind(db, Kind::Credit, day(2026, 12, 31)).unwrap()
        };

        for (from_kind, to_kind) in [
            (Kind::Cash, Kind::Cash),
            (Kind::Cash, Kind::Credit),
            (Kind::Credit, Kind::Cash),
            (Kind::Credit, Kind::Credit),
        ] {
            let db = db::open_in_memory().unwrap();
            let from = account::insert(&db, "FROM", "Source", from_kind, 0).unwrap();
            let to = account::insert(&db, "TO", "Destination", to_kind, 1).unwrap();
            add(&db, from, day(2026, 1, 1), 500_000, "opening");
            add(&db, to, day(2026, 1, 1), 200_000, "opening");
            let before = net(&db);

            insert_transfer(
                &db,
                from,
                to,
                day(2026, 8, 31),
                Cents(75_000),
                "Transfer",
                "Transfer",
            )
            .unwrap();

            assert_eq!(
                net(&db),
                before,
                "{from_kind:?} -> {to_kind:?} changed net worth"
            );
        }
    }

    /// `write_transfer` is the whole of a transfer except the transaction
    /// around it, so several of them compose into one atomic payday. Called
    /// inside `Db::transaction`, a failure anywhere must leave no leg
    /// behind -- which is what `insert_transfer` gets for free and a batch
    /// would otherwise not.
    #[test]
    fn several_transfers_in_one_transaction_all_roll_back_together() {
        let (db, checking, savings, _) = fixture();

        let result = db.transaction(|db| {
            write_transfer(
                db,
                checking,
                savings,
                day(2026, 8, 20),
                Cents::from_dollars(100),
                "Rainy Day",
                "Rainy Day",
            )?;
            write_transfer(
                db,
                checking,
                AccountId(9_999),
                day(2026, 8, 20),
                Cents::from_dollars(200),
                "Nowhere",
                "Nowhere",
            )
        });

        assert!(result.is_err(), "a transfer to a missing account succeeded");
        assert_eq!(count(&db).unwrap(), 0, "a leg survived a failed batch");
    }

    /// A bad source account fails before either leg is written. This is a
    /// reasonable smoke test, but it does *not* prove the transaction rolls
    /// anything back: the destination-kind lookup is a read that succeeds,
    /// and the very next statement -- the first leg's own insert, with the
    /// bad `account_id` -- is what fails. SQLite gives per-statement
    /// atomicity for free, so this passes identically whether or not
    /// `insert_transfer` opens a transaction at all. See
    /// `a_failure_on_the_second_leg_rolls_back_the_first` for the real proof.
    #[test]
    fn a_transfer_with_a_bad_source_account_writes_nothing() {
        let (db, checking, _, _) = fixture();
        // Account 9999 does not exist.
        let result = insert_transfer(
            &db,
            AccountId(9999),
            checking,
            day(2026, 8, 31),
            Cents(1_000),
            "Transfer",
            "Transfer",
        );
        assert!(result.is_err(), "expected the foreign key to reject this");

        let txns: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(txns, 0, "a leg survived a failed transfer");
    }

    /// `insert_transfer` decides each leg's sign from its account's kind, so a
    /// negative magnitude inverts both legs -- money moving the wrong way, in
    /// a pair of rows that still balance and so look correct. A debug-only
    /// assertion would let a release build write them.
    #[test]
    fn a_transfer_with_a_negative_magnitude_is_refused_and_writes_nothing() {
        let (db, checking, savings, _) = fixture();
        let err = insert_transfer(
            &db,
            checking,
            savings,
            day(2026, 8, 31),
            Cents(-1_000),
            "Transfer",
            "Transfer",
        )
        .unwrap_err();
        assert!(err.to_string().contains("positive magnitude"), "{err}");

        let txns: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(txns, 0, "a leg was written for a refused transfer");
    }

    /// The real rollback proof: the first leg is written successfully, and
    /// only the *second* leg's insert fails (forced by a trigger, since the
    /// public API cannot otherwise get a valid first leg to succeed and a
    /// valid second leg to fail). Without `insert_transfer`'s transaction,
    /// the first leg would survive under SQLite's default autocommit
    /// behavior, leaving a half-transfer -- exactly the corruption the
    /// atomic-write design exists to prevent.
    #[test]
    fn a_failure_on_the_second_leg_rolls_back_the_first() {
        let (db, checking, savings, _) = fixture();
        let before: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 0, "fixture should start with no transactions");

        // Abort the second insert, so the first leg is already written when
        // the failure lands.
        db.conn
            .execute_batch(
                "CREATE TEMP TRIGGER fail_second_leg
               AFTER INSERT ON txn
               WHEN (SELECT COUNT(*) FROM txn) = 2
             BEGIN
               SELECT RAISE(ABORT, 'second leg rejected');
             END;",
            )
            .unwrap();

        let result = insert_transfer(
            &db,
            checking,
            savings,
            day(2026, 8, 31),
            Cents(1_000),
            "Transfer",
            "Transfer",
        );
        assert!(
            result.is_err(),
            "the trigger should have aborted the transfer"
        );

        let after: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM txn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "the first leg survived a failed transfer");
    }

    #[test]
    fn a_card_payment_is_a_transfer_with_different_leg_descriptions() {
        let (db, _, savings, card_one) = fixture();
        // Paying the Card One card from Rainy Day: both legs are negative, because
        // credit is stored as debt.
        insert_transfer(
            &db,
            savings,
            card_one,
            day(2026, 9, 8),
            Cents(450_085),
            "CC1 Payment",
            "Payment",
        )
        .unwrap();
        assert_eq!(
            balance_at(&db, savings, day(2026, 9, 8)).unwrap(),
            Cents(-450_085)
        );
        // The credit leg is negative too -- it reduces the debt. This is the
        assert_eq!(
            balance_at(&db, card_one, day(2026, 9, 8)).unwrap(),
            Cents(-450_085)
        );
    }

    #[test]
    fn autocomplete_ranks_by_frequency_then_recency() {
        let (db, checking, _, card_one) = fixture();
        for i in 1..=5 {
            add(&db, card_one, day(2026, 1, i), 240, "MBTA");
        }
        // Two rows with different accounts and amounts so the assertion
        // below can only pass if the query actually picks the later-dated
        // row's bare columns, not merely a row that happens to match.
        add(&db, checking, day(2026, 6, 1), 1_000, "Movies");
        add(&db, card_one, day(2026, 7, 1), 1_499, "Movies");
        add(&db, card_one, day(2026, 8, 1), 950, "Mother's Day");

        let hits = autocomplete(&db, "M", 10).unwrap();
        assert_eq!(hits[0].description, "MBTA");
        assert_eq!(hits[1].description, "Movies");
        // The prefill carries the most recent (2026-07-01) account and
        // amount, not the earlier (2026-06-01) row's.
        assert_eq!(hits[1].cents, Cents(1_499));
        assert_eq!(hits[1].account_id, card_one);

        // Prefix must actually filter.
        let hits = autocomplete(&db, "MB", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn autocomplete_breaks_ties_on_use_count_by_recency() {
        let (db, _, _, card_one) = fixture();
        // Equal use counts (2 each), so the tie can only be broken by which
        // description was used more recently.
        add(&db, card_one, day(2026, 1, 1), 100, "Zebra");
        add(&db, card_one, day(2026, 1, 2), 100, "Zebra");
        add(&db, card_one, day(2026, 1, 10), 200, "Zoo");
        add(&db, card_one, day(2026, 1, 20), 200, "Zoo");

        let hits = autocomplete(&db, "Z", 10).unwrap();
        assert_eq!(
            hits[0].description, "Zoo",
            "more recent last use must rank first on a tie"
        );
        assert_eq!(hits[1].description, "Zebra");
    }

    fn descriptions(rows: &[Txn]) -> Vec<&str> {
        rows.iter().map(|t| t.description.as_str()).collect()
    }

    fn cash_between(from: NaiveDate, to: NaiveDate) -> Filter {
        Filter {
            kind: Kind::Cash,
            account_id: None,
            from,
            to,
            search: None,
        }
    }

    /// The window the ledger opens on is one or two months wide, so the tests
    /// that are about something other than dates use a year to stay out of
    /// the way.
    fn cash_2026() -> Filter {
        cash_between(day(2026, 1, 1), day(2026, 12, 31))
    }

    #[test]
    fn list_returns_one_kinds_rows_within_the_date_range_in_date_order() {
        let (db, checking, savings, card_one) = fixture();
        add(&db, checking, day(2026, 6, 30), 100, "before");
        add(&db, checking, day(2026, 8, 15), 100, "august");
        add(&db, savings, day(2026, 7, 1), 100, "july");
        add(&db, card_one, day(2026, 7, 4), 100, "a charge");
        add(&db, checking, day(2026, 9, 1), 100, "after");

        let rows = list(&db, &cash_between(day(2026, 7, 1), day(2026, 8, 31))).unwrap();
        assert_eq!(descriptions(&rows), vec!["july", "august"]);
    }

    /// Both bounds are inclusive: a window is built from the first and last
    /// day of a month, so an exclusive end would drop every month's last day.
    #[test]
    fn list_includes_rows_dated_on_either_bound() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 8, 1), 100, "first");
        add(&db, checking, day(2026, 8, 31), 100, "last");

        let rows = list(&db, &cash_between(day(2026, 8, 1), day(2026, 8, 31))).unwrap();
        assert_eq!(descriptions(&rows), vec!["first", "last"]);
    }

    #[test]
    fn list_filters_to_one_account_when_asked() {
        let (db, checking, savings, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "checking");
        add(&db, savings, day(2026, 1, 2), 100, "savings");

        let all = list(&db, &cash_2026()).unwrap();
        assert_eq!(all.len(), 2);

        let filtered = list(
            &db,
            &Filter {
                account_id: Some(savings),
                ..cash_2026()
            },
        )
        .unwrap();
        assert_eq!(descriptions(&filtered), vec!["savings"]);
    }

    #[test]
    fn list_searches_descriptions_anywhere_in_the_string() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "Whole Foods");
        add(&db, checking, day(2026, 1, 2), 100, "Foods R Us");
        add(&db, checking, day(2026, 1, 3), 100, "Mortgage");

        let rows = list(
            &db,
            &Filter {
                search: Some("Foods".to_string()),
                ..cash_2026()
            },
        )
        .unwrap();
        assert_eq!(descriptions(&rows), vec!["Whole Foods", "Foods R Us"]);
    }

    /// A `%` typed into the search box is a literal. Without the escape it is
    /// a wildcard, so searching for it matches the entire ledger -- the
    /// search box would look broken exactly when it is used on a real
    /// description containing a percent sign.
    #[test]
    fn a_percent_in_a_search_string_is_a_literal_not_a_wildcard() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "5% APY bonus");
        add(&db, checking, day(2026, 1, 2), 100, "Mortgage");

        let rows = list(
            &db,
            &Filter {
                search: Some("%".to_string()),
                ..cash_2026()
            },
        )
        .unwrap();
        assert_eq!(descriptions(&rows), vec!["5% APY bonus"]);
    }

    #[test]
    fn list_reads_back_every_column_it_was_given() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 5, 4), -4_500, "Phone");

        let rows = list(&db, &cash_2026()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, day(2026, 5, 4));
        assert_eq!(rows[0].cents, Cents(-4_500));
        assert_eq!(rows[0].account_id, checking);
        assert_eq!(rows[0].description, "Phone");
        assert_eq!(rows[0].recurring_txn_id, None);
        assert!(!rows[0].edited);
    }

    #[test]
    fn date_range_spans_the_oldest_and_newest_row() {
        let (db, checking, _, card_one) = fixture();
        add(&db, checking, day(2024, 12, 31), 100, "oldest");
        add(&db, card_one, day(2026, 3, 9), 100, "newest");

        assert_eq!(
            date_range(&db).unwrap(),
            Some((day(2024, 12, 31), day(2026, 3, 9)))
        );
    }

    /// The range covers both ledgers. `[` and `]` clamp against it on the
    /// Cash and Credit screens alike, and a credit-only month is still a
    /// month worth being able to reach.
    #[test]
    fn date_range_covers_every_kind_not_just_the_one_being_viewed() {
        let (db, _, _, card_one) = fixture();
        add(&db, card_one, day(2026, 5, 1), 100, "a charge");

        assert_eq!(
            date_range(&db).unwrap(),
            Some((day(2026, 5, 1), day(2026, 5, 1)))
        );
    }

    #[test]
    fn date_range_of_an_empty_ledger_is_none() {
        let (db, _, _, _) = fixture();
        assert_eq!(date_range(&db).unwrap(), None);
    }

    /// Inserts a paycheck recurring transaction and returns its id.
    fn paycheck_recurring_txn(db: &Db, account_id: AccountId) -> RecurringTxnId {
        let id = crate::db::recurring_txn::insert(
            db,
            &crate::db::recurring_txn::NewRecurringTxn {
                description: "Paycheck".to_string(),
                cents: Cents(500_000),
                account_id,
                cadence: crate::db::recurring_txn::Cadence::Biweekly,
                anchor_date: day(2026, 1, 2),
                horizon: None,
            },
        )
        .unwrap();
        crate::db::recurring_txn::set_paycheck(db, id).unwrap();
        id
    }

    #[test]
    fn update_overwrites_every_editable_column() {
        let (db, checking, savings, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "typo");
        let id = list(&db, &cash_2026()).unwrap()[0].id;

        update(
            &db,
            id,
            &NewTxn {
                date: day(2026, 2, 2),
                cents: Cents(-4_500),
                account_id: savings,
                description: "Phone".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();

        let row = list(&db, &cash_2026()).unwrap().remove(0);
        assert_eq!(row.date, day(2026, 2, 2));
        assert_eq!(row.cents, Cents(-4_500));
        assert_eq!(row.account_id, savings);
        assert_eq!(row.description, "Phone");
    }

    /// The odd cent is the case worth pinning: eleven paychecks, ten at
    /// 5,000.00 and one at 4,999.99. Stage 5 regenerates a recurring transaction's rows
    /// with `DELETE ... WHERE recurring_txn_id = ? AND edited = 0`, so without
    /// this flag the correction is silently thrown away and the balance is
    /// wrong by a cent forever.
    #[test]
    fn editing_a_rule_generated_row_marks_it_edited_and_keeps_its_rule() {
        let (db, checking, _, _) = fixture();
        let recurring_txn = paycheck_recurring_txn(&db, checking);
        insert(
            &db,
            &NewTxn {
                date: day(2026, 1, 2),
                cents: Cents(500_000),
                account_id: checking,
                description: "Paycheck".to_string(),
                recurring_txn_id: Some(recurring_txn),
            },
        )
        .unwrap();
        let row = list(&db, &cash_2026()).unwrap().remove(0);
        assert!(!row.edited, "a freshly generated row is not edited");

        update(
            &db,
            row.id,
            &NewTxn {
                cents: Cents(499_999),
                // The forms carry no recurring transaction id, so `update`
                // must ignore this field rather than orphan the row from its
                // recurring transaction.
                recurring_txn_id: None,
                ..row.clone().into_new()
            },
        )
        .unwrap();

        let edited = list(&db, &cash_2026()).unwrap().remove(0);
        assert_eq!(edited.cents, Cents(499_999));
        assert!(
            edited.edited,
            "a hand-edited recurring-transaction row must be flagged"
        );
        assert_eq!(
            edited.recurring_txn_id,
            Some(recurring_txn),
            "the row lost its recurring transaction"
        );
    }

    #[test]
    fn editing_a_hand_entered_row_leaves_it_unflagged() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "Coffee");
        let row = list(&db, &cash_2026()).unwrap().remove(0);

        update(
            &db,
            row.id,
            &NewTxn {
                cents: Cents(250),
                ..row.into_new()
            },
        )
        .unwrap();

        let edited = list(&db, &cash_2026()).unwrap().remove(0);
        assert!(
            !edited.edited,
            "nothing regenerates a row with no recurring transaction, so nothing needs flagging"
        );
    }

    #[test]
    fn update_of_a_missing_row_is_an_error() {
        let (db, checking, _, _) = fixture();
        let err = update(
            &db,
            TxnId(9999),
            &NewTxn {
                date: day(2026, 1, 1),
                cents: Cents(1),
                account_id: checking,
                description: "ghost".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("9999"), "{err}");
    }

    #[test]
    fn delete_removes_exactly_one_row() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100, "keep");
        add(&db, checking, day(2026, 1, 2), 100, "drop");
        let rows = list(&db, &cash_2026()).unwrap();

        delete(&db, rows[1].id).unwrap();

        assert_eq!(
            descriptions(&list(&db, &cash_2026()).unwrap()),
            vec!["keep"]
        );
    }

    #[test]
    fn delete_of_a_missing_row_is_an_error() {
        let (db, _, _, _) = fixture();
        let err = delete(&db, TxnId(9999)).unwrap_err();
        assert!(err.to_string().contains("9999"), "{err}");
    }

    /// Joining outward from `account` is the whole point: a `GROUP BY` over
    /// `txn` drops an account with no rows in range entirely, and an account
    /// missing from Overview reads as an account that does not exist rather
    /// than one sitting at zero.
    #[test]
    fn balances_at_reports_an_account_with_no_transactions_as_zero() {
        let (db, checking, savings, card_one) = fixture();
        add(&db, checking, day(2026, 1, 1), 100_000, "opening");
        add(&db, card_one, day(2026, 1, 1), 7_000, "charge");
        // SAV deliberately gets no rows at all.

        let balances = balances_at(&db, day(2026, 1, 1)).unwrap();
        assert_eq!(balances.len(), 3, "every account must appear");
        let held = |id| {
            balances
                .iter()
                .find(|(a, _)| *a == id)
                .map(|(_, c)| *c)
                .expect("account missing from balances_at")
        };
        assert_eq!(held(checking), Cents(100_000));
        assert_eq!(held(savings), Cents::ZERO);
        // Credit is stored as debt: positive is a charge. Callers negate.
        assert_eq!(held(card_one), Cents(7_000));
    }

    #[test]
    fn balances_at_excludes_rows_after_the_date() {
        let (db, checking, _, _) = fixture();
        add(&db, checking, day(2026, 1, 1), 100_000, "today");
        add(&db, checking, day(2026, 9, 1), -120_000, "future");

        let at = |date| {
            balances_at(&db, date)
                .unwrap()
                .into_iter()
                .find(|(a, _)| *a == checking)
                .unwrap()
                .1
        };
        assert_eq!(at(day(2026, 1, 1)), Cents(100_000));
        assert_eq!(at(day(2026, 9, 1)), Cents(-20_000));
    }
}
