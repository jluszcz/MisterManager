//! The chain of schema edits, and the runner that applies it.
//!
//! `schema.sql` is frozen at version 1 and is never edited again. Every
//! change since is an arm in [`MIGRATIONS`], and a fresh database takes the
//! baseline and then the whole chain -- the same SQL, in the same order, that
//! an existing database takes the tail of.
//!
//! One code path is the whole point. There is no second statement of the
//! schema for the chain to drift from, and every test in the crate builds its
//! database through `db::open_in_memory`, so the chain is replayed from
//! version 1 on every `cargo test`. An arm that stops working because a later
//! arm changed the ground under it fails the suite rather than waiting to
//! fail on the one database that has not been migrated yet.
//!
//! What that costs is a `schema.sql` that stops describing the database as it
//! actually is, once the chain is more than a few arms long. The remedy is to
//! squash: with the owner's database at the head version, dump the schema it
//! actually has into `schema.sql` as the new version 1, empty [`MIGRATIONS`],
//! and delete-and-reimport once. That is a periodic operation rather than a
//! one-off -- it is what keeps the staleness bounded, and never doing it is
//! what would turn "frozen" into a trap.

use anyhow::Result;
use rusqlite::Connection;

/// The frozen baseline: the schema as it stood at version 1.
///
/// Never edit this to describe a change. A change is an arm in
/// [`MIGRATIONS`]; editing the baseline instead would give a fresh database a
/// schema no existing one can reach, which is the one failure this whole
/// arrangement exists to make impossible. It is rewritten only by a squash,
/// and then wholesale.
const SCHEMA: &str = include_str!("schema.sql");

/// One edit to the schema, and the version it leaves a database at.
///
/// Three rules, which replaying the chain from version 1 makes binding rather
/// than advisory:
///
/// - **An arm names the schema as it stood when the arm was written, never as
///   it stands today.** An arm two versions before a table is renamed still
///   calls that table by its old name, because that is the name the database
///   it runs against has.
/// - **An arm must survive replaying against an empty database**, which is
///   what a fresh install is: every arm runs there too, in order, with no rows
///   anywhere. A `data` half that finds nothing to move returns `Ok` rather
///   than erroring.
/// - **A change of meaning is an arm too** -- a `data` half with no `sql`. A
///   column whose type does not change but whose interpretation does leaves an
///   existing database holding values under the old reading, and only an arm
///   can put that right. Version 2 of this schema was exactly that case:
///   `account` stopped being an imported table, so its name, band and order
///   became the owner's rather than the importer's.
pub(super) struct Migration {
    /// The version this arm leaves the database at. Its position in
    /// [`MIGRATIONS`] gives the same number, and
    /// `every_arm_declares_the_version_its_position_gives_it` is what holds
    /// the two together.
    pub version: i64,
    /// Run as a batch, so several statements separated by `;` are fine.
    pub sql: &'static str,
    /// Run after `sql`, inside the same transaction. The column a data half
    /// writes to does not exist until its own `sql` has made room.
    pub data: Option<fn(&Connection) -> Result<()>>,
}

/// Every change to the schema since version 1, in order.
///
/// Adding one means appending an arm and nothing else: the head version is one
/// plus the length, so there is no separate constant to bump and therefore no
/// way to forget to. Empty is a legitimate state rather than an unfinished
/// one -- it is what the chain looks like just after a squash, and
/// `schema.sql` alone is then the whole schema.
pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 2,
        // The color an account's name draws in, on every screen that names it.
        // Nullable because that is the state every existing row is already in
        // and a real choice in its own right: `account::set_color` writes `NULL`
        // back, and `tui::style::account_color` draws it in the shade the id
        // derives, which is exactly what the app looked like before the column
        // existed. The `CHECK` list is `account::AccountColor::as_str` written
        // out, the way `kind`, `grp` and `interest_policy` are above it.
        sql: "ALTER TABLE account ADD COLUMN color TEXT \
          CHECK (color IN ('blue', 'copper', 'violet', 'teal', \
                           'rose', 'olive', 'indigo', 'tan'))",
        // Nothing to move: every account keeps the derived color it already had,
        // by holding no color at all.
        data: None,
    },
    Migration {
        version: 3,
        // Whether the Savings screen draws this goal's row as a band. The owner's
        // own mark, the way an account's color is -- so `NOT NULL DEFAULT 0`
        // rather than nullable: there is no third state between favorited and
        // not, and every existing goal is already in the second one.
        sql: "ALTER TABLE goal ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0",
        // Nothing to move: the default is the state every existing goal is in.
        data: None,
    },
    Migration {
        version: 4,
        // A taxed goal stores what the item costs on the shelf and derives
        // what it costs at the register, so the column stops being the goal
        // and starts being the base the goal is computed from. Renaming it is
        // what makes the compiler visit every reader: a change of meaning is
        // one no `CHECK` can catch.
        //
        // `NOT NULL DEFAULT 0` rather than nullable, for the reason `favorite`
        // is: there is no third state between taxed and not.
        sql: "ALTER TABLE goal RENAME COLUMN goal_cents TO base_cents;
              ALTER TABLE goal ADD COLUMN taxed INTEGER NOT NULL DEFAULT 0",
        // Nothing to move, and nothing that *could* be moved. Every existing
        // goal's stored figure already is its target: an imported one came off
        // a sheet whose goal column holds whatever the owner put there, and a
        // picker-created one had `calc::tax` applied before the insert.
        // `taxed = 0` is exactly the reading those rows want.
        //
        // Deliberately no back-fill from `recurring_goal.taxed` through
        // `goal.recurring_goal_id`: those goals hold a taxed figure already,
        // so flagging them would tax it twice, and the lambda ceilings, so the
        // base cannot be recovered by inverting it.
        data: None,
    },
    Migration {
        version: 5,
        // Two codes differing only in case name one account, so the baseline's
        // `UNIQUE (code, kind)` is a case short of what `account::by_code`
        // matches on. It has to be an index rather than an edit to that
        // constraint -- SQLite cannot alter one, and the baseline is frozen --
        // and being stricter, it makes the constraint beside it redundant
        // rather than contradicted.
        //
        // The guard in `account::insert` is what an owner actually meets;
        // this is the backstop under it, and what makes a database holding
        // both `chk` and `CHK` in one kind unreachable rather than merely
        // unwritten. `by_code` would have to pick one of the two, and which
        // is not a question a row can answer.
        sql: "CREATE UNIQUE INDEX account_code_kind \
                ON account (code COLLATE NOCASE, kind)",
        // Nothing to move. A database already holding a pair the index
        // refuses has two rows for one account, and the arm failing here is
        // the first anything has said about it -- one of them has to be
        // renamed, and only the owner knows which.
        data: None,
    },
];

/// The version this build's chain leaves a database at.
///
/// Nothing in production names it: [`apply`] works out the head of whatever
/// chain it is handed, so the only readers are the tests that assert a
/// database came out at the right version. It is here so they have one name
/// for it rather than each recomputing the sum.
#[cfg(test)]
pub(super) const SCHEMA_VERSION: i64 = head(MIGRATIONS);

/// The version a chain leaves a database at: one per arm, above the frozen
/// baseline.
const fn head(chain: &[Migration]) -> i64 {
    1 + chain.len() as i64
}

/// Bring `conn` up to this build's schema.
///
/// Split from [`apply`] so the tests can drive the runner with a synthetic
/// baseline and a synthetic chain. With [`MIGRATIONS`] empty there would
/// otherwise be nothing to exercise, and the machinery would ship untested
/// until the first arm that needed it.
pub(super) fn run(conn: &Connection) -> Result<()> {
    apply(conn, SCHEMA, MIGRATIONS)
}

/// Apply `chain` to whatever version `conn` is at.
///
/// Zero is the one version that is not "some other schema": it is an empty
/// file, and filling it is the whole job, so it takes `schema` and then every
/// arm. A version between the baseline and the head takes the arms above it
/// and not the baseline, which it already holds. A version above the head is a
/// database some later build wrote, and it is refused rather than
/// best-effort opened -- the arms that produced it do not exist here, and a
/// column that is not there surfaces as a failed query somewhere far from
/// this function, with figures already on screen.
///
/// The baseline, the arms, their data halves and the version stamp are all one
/// transaction. `PRAGMA user_version` is transactional in SQLite, so a failure
/// partway through leaves the database at the version it came in at rather
/// than half-migrated and stamped as though it had succeeded -- which would be
/// permanent, since the next run would skip the arms it never finished.
fn apply(conn: &Connection, schema: &str, chain: &[Migration]) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let head = head(chain);
    if current == head {
        return Ok(());
    }
    if current > head {
        // The rebuild is a way out only where the build has an importer, and
        // this is the one path where the owner is already stuck: handing them
        // `mm import` on a build whose binary has no such subcommand spends
        // the only instruction they get on a command that cannot run.
        #[cfg(feature = "import")]
        const REBUILD: &str = "delete the file and re-run `mm import <workbook>`";
        #[cfg(not(feature = "import"))]
        const REBUILD: &str =
            "delete the file and rebuild it with a build carrying the `import` feature";
        anyhow::bail!(
            "database is at schema version {current}, newer than this build ({head}); \
             open it with the build that wrote it, or {REBUILD}, then re-enter the \
             recurring transactions"
        );
    }
    let tx = conn.unchecked_transaction()?;
    if current == 0 {
        tx.execute_batch(schema)?;
    }
    for arm in chain.iter().filter(|arm| arm.version > current) {
        tx.execute_batch(arm.sql)?;
        if let Some(data) = arm.data {
            data(&tx)?;
        }
    }
    tx.pragma_update(None, "user_version", head)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A baseline small enough to read, so a test can say what the schema
    /// looks like before and after an arm rather than asserting against the
    /// real one.
    const BASELINE: &str = "CREATE TABLE t (a INTEGER)";

    /// One arm, which is the shortest chain that can tell "applied the
    /// baseline" apart from "applied the baseline and then the chain".
    const ONE_ARM: &[Migration] = &[Migration {
        version: 2,
        sql: "ALTER TABLE t ADD COLUMN b INTEGER",
        data: None,
    }];

    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    fn columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('t')")
            .unwrap();
        let found = stmt.query_map([], |r| r.get(0)).unwrap();
        found.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    /// An empty file is the one version that is not "some other schema", and
    /// filling it is the whole job: it takes the baseline and then every arm,
    /// because the baseline is frozen at version 1 and the chain is where
    /// every change since is written.
    #[test]
    fn an_empty_database_takes_the_baseline_and_then_the_whole_chain() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, BASELINE, ONE_ARM).unwrap();
        assert_eq!(version(&conn), 2);
        assert_eq!(columns(&conn), ["a", "b"]);
    }

    /// A database already at the head has nothing to do, and doing it anyway
    /// fails with "table t already exists" -- which is what every run after
    /// the first would be.
    #[test]
    fn a_database_at_the_head_version_is_left_alone() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, BASELINE, ONE_ARM).unwrap();
        apply(&conn, BASELINE, ONE_ARM).unwrap();
        assert_eq!(version(&conn), 2);
        assert_eq!(columns(&conn), ["a", "b"]);
    }

    /// A database written before the chain existed is at version 1 and holds
    /// the baseline already. Re-applying it would fail with "table t already
    /// exists"; what it needs is the arms above it and nothing else.
    #[test]
    fn a_database_at_version_one_takes_the_chain_and_not_the_baseline() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(BASELINE).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        apply(&conn, BASELINE, ONE_ARM).unwrap();

        assert_eq!(version(&conn), 2);
        assert_eq!(columns(&conn), ["a", "b"]);
    }

    /// A version above the head is a database some later build wrote, and
    /// there is nothing this one can do with it: the arms that produced it do
    /// not exist here. Opening it anyway would surface as a failed query
    /// somewhere far from here, with figures already on screen.
    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(BASELINE).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();

        let err = apply(&conn, BASELINE, ONE_ARM).unwrap_err().to_string();

        assert!(err.contains("version 3"), "{err}");
        assert!(err.contains("newer than this build"), "{err}");
        assert_eq!(version(&conn), 3, "the database was written to anyway");
    }

    fn double_a_into_b(conn: &Connection) -> Result<()> {
        conn.execute("UPDATE t SET b = a * 2", [])?;
        Ok(())
    }

    /// An arm's data half is the interesting one: the SQL makes room and the
    /// data half fills it, so it has to run, and it has to run after its own
    /// SQL rather than before it -- the column it writes to does not exist
    /// until then.
    #[test]
    fn an_arms_data_half_runs_after_its_sql() {
        const WITH_DATA: &[Migration] = &[Migration {
            version: 2,
            sql: "ALTER TABLE t ADD COLUMN b INTEGER",
            data: Some(double_a_into_b),
        }];

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(BASELINE).unwrap();
        conn.execute("INSERT INTO t (a) VALUES (21)", []).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        apply(&conn, BASELINE, WITH_DATA).unwrap();

        let b: i64 = conn.query_row("SELECT b FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(b, 42);
    }

    /// The whole chain is one transaction, and `PRAGMA user_version` is
    /// transactional in SQLite, so a failure partway through leaves the
    /// database at the version it came in at rather than half-migrated and
    /// stamped as though it had succeeded -- which would be permanent, since
    /// the next run would skip the arms it never finished.
    #[test]
    fn an_arm_that_fails_leaves_the_version_and_the_schema_untouched() {
        const SECOND_ARM_IS_BROKEN: &[Migration] = &[
            Migration {
                version: 2,
                sql: "ALTER TABLE t ADD COLUMN b INTEGER",
                data: None,
            },
            Migration {
                version: 3,
                sql: "ALTER TABLE nonexistent ADD COLUMN c INTEGER",
                data: None,
            },
        ];

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(BASELINE).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        apply(&conn, BASELINE, SECOND_ARM_IS_BROKEN).unwrap_err();

        assert_eq!(version(&conn), 1);
        assert_eq!(columns(&conn), ["a"], "arm 2 was left half-applied");
    }

    /// [`SCHEMA_VERSION`] is one plus the number of arms, so an arm declaring
    /// a version its position does not agree with makes the head version and
    /// the chain disagree about what a database is at. The declared number is
    /// still worth having -- it is what the arm's own prose names, and what
    /// the filter in [`apply`] reads -- so this is what holds the two
    /// together.
    #[test]
    fn every_arm_declares_the_version_its_position_gives_it() {
        for (index, arm) in MIGRATIONS.iter().enumerate() {
            let expected = index as i64 + 2;
            assert_eq!(
                arm.version, expected,
                "the arm at index {index} declares version {}",
                arm.version
            );
        }
        assert_eq!(SCHEMA_VERSION, head(MIGRATIONS));
    }

    fn stamp_b(conn: &Connection) -> Result<()> {
        conn.execute("UPDATE t SET b = 1", [])?;
        Ok(())
    }

    /// A change of meaning is an arm with a data half and no SQL, so an empty
    /// batch has to be a no-op rather than an error -- otherwise the pattern
    /// the docs describe would fail on its first use.
    #[test]
    fn an_arm_may_carry_a_data_half_and_no_sql() {
        const MEANING_ONLY: &[Migration] = &[
            Migration {
                version: 2,
                sql: "ALTER TABLE t ADD COLUMN b INTEGER",
                data: None,
            },
            Migration {
                version: 3,
                sql: "",
                data: Some(stamp_b),
            },
        ];

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(BASELINE).unwrap();
        conn.execute_batch("ALTER TABLE t ADD COLUMN b INTEGER")
            .unwrap();
        conn.execute("INSERT INTO t (a) VALUES (1)", []).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();

        apply(&conn, BASELINE, MEANING_ONLY).unwrap();

        let b: i64 = conn.query_row("SELECT b FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(b, 1);
        assert_eq!(version(&conn), 3);
    }

    /// The real chain rather than the synthetic one, because this arm renames
    /// a column that existing rows carry values in: what matters is that the
    /// values are still there afterwards, under the new name and under the
    /// reading `taxed = 0` gives them.
    ///
    /// `MIGRATIONS[..2]` is where a database written by the previous build
    /// sits -- the head of a two-arm chain is version 3.
    #[test]
    fn arm_four_renames_the_goal_column_and_leaves_every_goal_untaxed() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, SCHEMA, &MIGRATIONS[..2]).unwrap();
        assert_eq!(version(&conn), 3);
        conn.execute(
            "INSERT INTO account (id, code, name, kind, grp)
             VALUES (1, 'SAV', 'Rainy Day', 'cash', 'savings')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO goal (id, name, container_account_id, goal_cents)
             VALUES (1, 'Couch', 1, 106500)",
            [],
        )
        .unwrap();

        apply(&conn, SCHEMA, MIGRATIONS).unwrap();

        assert_eq!(version(&conn), SCHEMA_VERSION);
        let (base, taxed): (i64, i64) = conn
            .query_row("SELECT base_cents, taxed FROM goal WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(base, 106_500, "the figure did not survive the rename");
        assert_eq!(
            taxed, 0,
            "an existing goal already holds its target, so it is not taxed"
        );
    }

    /// The real chain again, because what this arm adds is a constraint on
    /// rows an existing database already holds: a code differing from one
    /// there only in case has to stop being insertable, and one differing in
    /// more than case has to go on being inserted.
    #[test]
    fn arm_five_makes_a_code_unique_within_its_kind_whatever_its_case() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, SCHEMA, &MIGRATIONS[..3]).unwrap();
        conn.execute(
            "INSERT INTO account (id, code, name, kind, grp)
             VALUES (1, 'CHK', 'Everyday', 'cash', 'checking')",
            [],
        )
        .unwrap();

        apply(&conn, SCHEMA, MIGRATIONS).unwrap();

        assert_eq!(version(&conn), SCHEMA_VERSION);
        let clash = conn.execute(
            "INSERT INTO account (id, code, name, kind, grp)
             VALUES (2, 'chk', 'Everyday Again', 'cash', 'checking')",
            [],
        );
        assert!(
            clash.is_err(),
            "two codes differing only in case name one account"
        );
        // The same code under the other kind is what the index is keyed on
        // two columns to allow: one bank is both a checking account and the
        // card drawn on it.
        conn.execute(
            "INSERT INTO account (id, code, name, kind, grp)
             VALUES (3, 'chk', 'Everyday Card', 'credit', 'credit')",
            [],
        )
        .unwrap();
    }
}
