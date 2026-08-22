//! Everything that knows SQLite lives here.
//!
//! `rusqlite` is named nowhere else in the crate: `calc` is pure, and
//! `import`, `plan`, `projection`, and the CLI reach the database only
//! through the per-aggregate query modules below.
//!
//! Each module here states the columns its `from_row` reads in a `select_*!`
//! macro sitting beside it, and builds every `SELECT` of that row from it.
//! `concat!` does the building at compile time, so the queries stay
//! `&'static str` rather than strings formatted afresh on every call. A query
//! wanting those columns in another shape -- table-qualified for a join, with
//! an aggregate after them -- takes an arm of the same macro rather than a
//! list of its own, so what a reader checks `from_row`'s indices against is
//! in one place per table.

pub mod account;
pub mod bill;
pub mod date;
pub mod fund;
pub mod goal;
pub mod id;
mod migration;
pub mod recurring_goal;
pub mod recurring_txn;
pub mod setting;
pub mod txn;

pub use id::{
    AccountId, AllocationId, BatchId, BillId, FundId, GoalId, RecurringGoalId, RecurringTxnId,
    TxnId,
};

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

#[cfg(test)]
use migration::SCHEMA_VERSION;

/// An open database.
///
/// The connection is private, and deliberately not reachable through `Deref`:
/// handing out a `&Connection` would put every rusqlite method back within
/// reach of `import` and `plan`, and the query modules in this directory
/// would stop being the only way in. The submodules here can still read the
/// field, since privacy extends to a module's descendants.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Run `f` inside one SQL transaction, committing only if it returns
    /// `Ok`. Dropping the guard on the error path rolls back.
    ///
    /// `f` receives this same `Db`: the transaction lives on the connection,
    /// so every query made through it during the closure is inside the
    /// transaction.
    ///
    /// **Not reentrant.** This issues a bare `BEGIN`, which SQLite rejects
    /// while a transaction is already open ("cannot start a transaction
    /// within a transaction"). Callers reachable from inside another
    /// `transaction` must not use it.
    pub fn transaction<T>(&self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let value = f(self)?;
        tx.commit()?;
        Ok(value)
    }
}

pub fn open(path: &Path) -> Result<Db> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening database at {}", path.display()))?;
    prepare(&conn).with_context(|| format!("preparing database at {}", path.display()))?;
    Ok(Db { conn })
}

pub fn open_in_memory() -> Result<Db> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn)?;
    Ok(Db { conn })
}

/// Write a consistent copy of the database at `src` to `dest`, which must not
/// already exist.
///
/// `VACUUM INTO` rather than a file copy: the database runs in WAL mode, so
/// the `.db` file on its own is a torn read of whatever had not been
/// checkpointed. This writes a checkpointed, compacted copy in one statement.
///
/// Deliberately opens the connection without `prepare`: taking a backup
/// must not migrate the database as a side effect.
pub fn snapshot(src: &Path, dest: &Path) -> Result<()> {
    let dest = dest
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", dest.display()))?;
    let conn =
        Connection::open(src).with_context(|| format!("opening database at {}", src.display()))?;
    conn.execute("VACUUM INTO ?1", [dest])
        .with_context(|| format!("snapshotting to {dest}"))?;
    Ok(())
}

/// Tables an import writes to, in foreign-key-safe delete order.
///
/// `allocation.goal_id` cascades from `goal` (`ON DELETE CASCADE`), but it is
/// deleted explicitly anyway so this order is self-documenting rather than
/// relying on a cascade a reader has to go look up.
///
/// **Two tables the schema creates are deliberately not here.** `account`
/// holds the owner's own naming, banding and ordering, which the import no
/// longer supplies — it writes a row per code and nothing more — so clearing
/// it would throw that away on every `--replace`. And `recurring_txn` only
/// ever sat here because its `account_id` referenced rows the replace
/// rebuilt; with `account` surviving, so do the rules, and the `txn` rows
/// they own come back out of the workbook for `g` to adopt.
const IMPORTED_TABLES: &[&str] = &[
    "allocation",
    "batch",
    "goal",
    "recurring_goal",
    "txn",
    "bill",
    "fund",
    "setting",
];

/// Tables `--replace` leaves alone, and why each is exempt — the reasons are
/// on [`IMPORTED_TABLES`], which is where a reader looking for the clear list
/// arrives first.
///
/// Nothing reads it outside the tests, because a preserved table is preserved
/// by not being named rather than by being named. It exists so that between
/// them the two lists cover every table `schema.sql` creates, which is what
/// `every_table_the_schema_creates_is_either_cleared_or_deliberately_kept`
/// checks: a table added to the schema and forgotten in both would survive a
/// `--replace` silently.
#[cfg(test)]
const PRESERVED_TABLES: &[&str] = &["account", "recurring_txn"];

/// Whether this database already holds imported data.
///
/// Transactions and goals are the two tables an import cannot produce an
/// empty version of, so they stand in for the whole set.
pub fn has_imported_data(db: &Db) -> Result<bool> {
    Ok(txn::count(db)? > 0 || goal::count(db)? > 0)
}

/// Empty every table an import writes to, leaving [`PRESERVED_TABLES`]
/// exactly as they were.
///
/// Not wrapped in its own transaction: the only caller runs it inside the
/// import's, and `Db::transaction` is not reentrant.
pub fn clear_imported_data(db: &Db) -> Result<()> {
    for table in IMPORTED_TABLES {
        db.conn
            .execute(&format!("DELETE FROM {table}"), [])
            .with_context(|| format!("clearing {table}"))?;
    }
    Ok(())
}

fn prepare(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    migration::run(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runner's own semantics -- the chain, the arms above a version, the
    /// atomicity -- are tested in `db::migration` against a synthetic chain.
    /// What these two hold up is the real `schema.sql` going through it, and
    /// they run before there is a `Db` to speak of, so they hold a bare
    /// `Connection` -- the only place outside `open` that does.
    #[test]
    fn applying_the_real_schema_twice_is_a_no_op() {
        let conn = Connection::open_in_memory().unwrap();
        migration::run(&conn).unwrap();
        // A second run must do nothing. Re-applying would fail with
        // "table account already exists".
        migration::run(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// Zero is the one version that is not "some other schema": it is an
    /// empty file, and filling it is the whole job. A guard written as
    /// `current < SCHEMA_VERSION` would refuse it instead -- and every
    /// database the app creates starts there, whatever the chain's length.
    #[test]
    fn a_version_zero_database_is_created_rather_than_refused() {
        let conn = Connection::open_in_memory().unwrap();
        migration::run(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// The refusal has to reach the caller that actually opens a file, not
    /// just the private helper: `open` is what the CLI and the TUI call.
    #[test]
    fn opening_a_database_at_another_version_fails_by_path() {
        let path = std::env::temp_dir().join(format!(
            "mistermanager_test_version_{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }

        let err = match open(&path) {
            Err(err) => err,
            Ok(_) => panic!("a database at another version was opened"),
        };
        assert!(
            format!("{err:#}").contains("newer than this build"),
            "{err:#}"
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_file_backed_database_survives_being_reopened() {
        let path = std::env::temp_dir().join(format!(
            "mistermanager_test_reopen_{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let k: setting::Key<i64> = setting::Key::new("k");
        {
            let db = open(&path).unwrap();
            setting::set(&db, k, 1).unwrap();
        }
        {
            let db = open(&path).unwrap();
            assert_eq!(setting::get(&db, k).unwrap(), Some(1));
        }

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = open_in_memory().unwrap();
        // Account 999 does not exist.
        let result = db.conn.execute(
            "INSERT INTO txn (date, cents, account_id, description)
             VALUES ('2026-01-01', 100, 999, 'orphan')",
            [],
        );
        assert!(result.is_err(), "foreign key constraint was not enforced");
    }

    #[test]
    fn deleting_a_goal_cascades_to_its_allocations() {
        let db = open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO account (id, code, name, kind, grp)
                 VALUES (1, 'SAV', 'Rainy Day', 'cash', 'savings')",
                [],
            )
            .unwrap();
        db.conn.execute(
            "INSERT INTO goal (id, name, container_account_id, base_cents) VALUES (1, 'G', 1, 100)",
            [],
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO allocation (goal_id, date, cents) VALUES (1, '2026-01-01', 50)",
                [],
            )
            .unwrap();
        db.conn
            .execute("DELETE FROM goal WHERE id = 1", [])
            .unwrap();
        let remaining: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM allocation", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "allocations did not cascade");
    }

    /// The whole point of wrapping `import_all` in this: a failure partway
    /// through must leave nothing behind. Writes made through the `Db` handed
    /// to the closure are inside the transaction, so an `Err` return discards
    /// them.
    #[test]
    fn a_transaction_rolls_back_when_the_closure_fails() {
        let db = open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();

        let result: Result<()> = db.transaction(|db| {
            txn::insert(
                db,
                &txn::NewTxn {
                    date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                    cents: crate::money::Cents::from_dollars(10),
                    account_id: savings,
                    description: "written then abandoned".to_string(),
                    recurring_txn_id: None,
                },
            )?;
            // The row is visible from inside the transaction...
            assert_eq!(txn::count(db).unwrap(), 1);
            anyhow::bail!("something went wrong halfway through")
        });

        assert!(result.is_err());
        // ...and gone once the closure fails.
        assert_eq!(txn::count(&db).unwrap(), 0, "the write was not rolled back");
    }

    #[test]
    fn a_transaction_commits_and_returns_the_closures_value() {
        let db = open_in_memory().unwrap();
        let savings = db
            .transaction(|db| account::insert(db, "SAV", "Rainy Day", account::Kind::Cash, 0))
            .unwrap();
        assert_eq!(
            account::by_code(&db, "SAV", account::Kind::Cash)
                .unwrap()
                .unwrap()
                .id,
            savings
        );
    }

    /// `import_all` refuses to run against a populated database on the
    /// strength of this, so it must not read empty for a database holding
    /// only one of the two.
    #[test]
    fn has_imported_data_sees_transactions_and_goals_separately() {
        let db = open_in_memory().unwrap();
        assert!(!has_imported_data(&db).unwrap());

        let savings = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        // An account alone is not imported data: `constants::import` writes
        // accounts before anything else, and a run that failed there must
        // still be re-runnable.
        assert!(!has_imported_data(&db).unwrap());

        goal::insert(
            &db,
            &goal::NewGoal {
                name: "Vacation".to_string(),
                container_account_id: savings,
                base_cents: crate::money::Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        assert!(has_imported_data(&db).unwrap(), "a goal was not noticed");

        clear_imported_data(&db).unwrap();
        assert!(!has_imported_data(&db).unwrap());

        // And the other way round: transactions alone must register too. The
        // account is still there -- a replace does not clear it.
        txn::insert(
            &db,
            &txn::NewTxn {
                date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                cents: crate::money::Cents::from_dollars(10),
                account_id: savings,
                description: "opening".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        assert!(
            has_imported_data(&db).unwrap(),
            "a transaction was not noticed"
        );
    }

    /// The other half of the test below, which can only show that the listed
    /// tables clear: a table added to `schema.sql` and forgotten in both
    /// lists would survive a `--replace` and leave rows the import believes
    /// it removed.
    ///
    /// So every table the schema creates has to be named by exactly one of
    /// the two lists -- cleared, or kept on purpose with a reason written
    /// beside it -- and this is what says so.
    #[test]
    fn every_table_the_schema_creates_is_either_cleared_or_deliberately_kept() {
        let db = open_in_memory().unwrap();
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let mut created: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap();
        created.sort();

        let mut named: Vec<String> = IMPORTED_TABLES
            .iter()
            .chain(PRESERVED_TABLES)
            .map(|t| t.to_string())
            .collect();
        named.sort();

        assert_eq!(created, named);
        for table in PRESERVED_TABLES {
            assert!(!IMPORTED_TABLES.contains(table), "{table} is in both lists");
        }
    }

    /// Every table in `IMPORTED_TABLES` empties and every table in
    /// `PRESERVED_TABLES` does not, with rows present in each beforehand so a
    /// foreign key or a wrong delete order would surface as a failure rather
    /// than a vacuous pass. That the two lists are *complete* is the test
    /// above.
    #[test]
    fn clear_imported_data_empties_the_imported_tables_and_keeps_the_rest() {
        let db = open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        recurring_txn::insert(
            &db,
            &recurring_txn::NewRecurringTxn {
                description: "Paycheck".to_string(),
                cents: crate::money::Cents::from_dollars(2_000),
                account_id: savings,
                cadence: recurring_txn::Cadence::Biweekly,
                anchor_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
                horizon: None,
            },
        )
        .unwrap();
        let batch = goal::insert_batch(
            &db,
            goal::BatchKind::Import,
            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        )
        .unwrap();
        let recurring_goal_id = recurring_goal::insert(
            &db,
            &recurring_goal::NewEntry {
                name: "Car Insurance".to_string(),
                month: 3,
                base_cents: crate::money::Cents::from_dollars(1_200),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        let goal_id = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Vacation".to_string(),
                container_account_id: savings,
                base_cents: crate::money::Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: Some(recurring_goal_id),
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            goal_id,
            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            crate::money::Cents::from_dollars(100),
            None,
            Some(batch),
        )
        .unwrap();
        txn::insert(
            &db,
            &txn::NewTxn {
                date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                cents: crate::money::Cents::from_dollars(10),
                account_id: savings,
                description: "opening".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        setting::set(&db, setting::Key::<i64>::new("k"), 1).unwrap();
        fund::insert(
            &db,
            &fund::NewFund {
                name: "Bonds".to_string(),
                ord: 0,
                target: fund::Target::AgeOver30,
                actual: crate::money::Cents::from_dollars(30_000),
            },
        )
        .unwrap();

        clear_imported_data(&db).unwrap();

        for table in IMPORTED_TABLES {
            let remaining: i64 = db
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(remaining, 0, "{table} was not cleared");
        }
        for table in PRESERVED_TABLES {
            let remaining: i64 = db
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(remaining, 1, "{table} was cleared");
        }
    }

    /// A plain file copy of a live database with a hot WAL is not a consistent
    /// file. This is what would fail if `VACUUM INTO` were ever replaced by one.
    #[test]
    fn a_snapshot_carries_the_rows_and_the_schema_version() {
        let dir =
            std::env::temp_dir().join(format!("mistermanager_snapshot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("money.db");
        let dest = dir.join("snapshot.db");

        {
            let db = open(&src).unwrap();
            account::insert(&db, "CHK", "Everyday", account::Kind::Cash, 0).unwrap();
            setting::set(&db, setting::key::PAY_PERIODS_PER_YEAR, 26).unwrap();
            snapshot(&src, &dest).unwrap();
        }

        let copy = open(&dest).unwrap();
        assert_eq!(account::list(&copy).unwrap().len(), 1);
        assert_eq!(
            setting::get(&copy, setting::key::PAY_PERIODS_PER_YEAR).unwrap(),
            Some(26)
        );

        let version: i64 = Connection::open(&dest)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
