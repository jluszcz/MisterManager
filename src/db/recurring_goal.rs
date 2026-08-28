use super::{Db, RecurringGoalId};
use crate::money::Cents;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{OptionalExtension, Row, params};
use std::collections::HashMap;
use std::str::FromStr;

/// How often a recurring goal entry comes round.
///
/// The workbook's header says "Biannual Goals" but means every two years, so
/// the stored value is `biennial` and the translation happens once, at import.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Cadence {
    Annual,
    Biennial,
}

impl Cadence {
    pub const ALL: [Cadence; 2] = [Cadence::Annual, Cadence::Biennial];

    /// How many years one round covers -- the divisor beside
    /// `key::PAY_PERIODS_PER_YEAR` when a round's cost is spread over the
    /// paychecks before it comes round again. On the cadence rather than
    /// beside the screen that spreads it, for the reason [`Cadence`] is a
    /// type at all: `biennial` means two years in one place.
    pub fn years(self) -> i64 {
        match self {
            Cadence::Annual => 1,
            Cadence::Biennial => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Annual => "annual",
            Cadence::Biennial => "biennial",
        }
    }
}

impl FromStr for Cadence {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "annual" => Ok(Cadence::Annual),
            "biennial" => Ok(Cadence::Biennial),
            other => bail!("unknown recurring goal cadence {other:?}"),
        }
    }
}

/// A recurring goal to be created, before it has an id.
#[derive(Clone, Debug)]
pub struct NewEntry {
    pub name: String,
    /// 1-12. The schema's `CHECK` is the guard; nothing here re-checks it.
    pub month: i64,
    pub base_cents: Cents,
    pub taxed: bool,
    pub cadence: Cadence,
}

/// A recurring goal the owner re-creates each cycle -- the `Savings!O:Q`
/// block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: RecurringGoalId,
    pub name: String,
    /// 1-12. The schema's `CHECK` is the guard; nothing here re-checks it.
    pub month: i64,
    pub base_cents: Cents,
    pub taxed: bool,
    pub cadence: Cadence,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Entry> {
    let cadence: String = row.get(5)?;
    Ok(Entry {
        id: row.get(0)?,
        name: row.get(1)?,
        month: row.get(2)?,
        base_cents: Cents(row.get(3)?),
        taxed: row.get::<_, i64>(4)? != 0,
        cadence: cadence
            .parse()
            .expect("schema CHECK guarantees a valid cadence"),
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_recurring_goal {
    ($tail:literal) => {
        concat!(
            "SELECT id, name, month, base_cents, taxed, cadence FROM recurring_goal ",
            $tail
        )
    };
}

pub fn insert(db: &Db, entry: &NewEntry) -> Result<RecurringGoalId> {
    db.conn.execute(
        "INSERT INTO recurring_goal (name, month, base_cents, taxed, cadence)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            entry.name,
            entry.month,
            entry.base_cents.0,
            entry.taxed as i64,
            entry.cadence.as_str()
        ],
    )?;
    Ok(RecurringGoalId(db.conn.last_insert_rowid()))
}

pub fn list(db: &Db) -> Result<Vec<Entry>> {
    let mut stmt = db.conn.prepare(select_recurring_goal!("ORDER BY id"))?;
    let rows = stmt.query_map([], from_row)?;
    super::collect_rows(rows)
}

/// How many *open* goals each recurring goal entry currently has.
///
/// The picker's "Open?" column, and a hint only: goal names are not unique --
/// "Lego" appears three times in the workbook, each a separate purchase with
/// its own goal date -- so a second open goal against one entry is a
/// legitimate thing to want and is never blocked.
pub fn open_goal_counts(db: &Db) -> Result<HashMap<RecurringGoalId, i64>> {
    let mut stmt = db.conn.prepare(
        "SELECT recurring_goal_id, COUNT(*) FROM goal
          WHERE recurring_goal_id IS NOT NULL AND closed = 0
          GROUP BY recurring_goal_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// One entry by id. A missing entry is an error, not `None` -- the same rule
/// as [`super::account::get`]: an id read off another row is a foreign key,
/// and a dangling one is a corrupt database.
pub fn get(db: &Db, id: RecurringGoalId) -> Result<Entry> {
    db.conn
        .query_row(
            select_recurring_goal!("WHERE id = ?1"),
            params![id],
            from_row,
        )
        .optional()?
        .with_context(|| format!("no recurring goal with id {id}"))
}

pub fn update(db: &Db, id: RecurringGoalId, entry: &NewEntry) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE recurring_goal SET name = ?2, month = ?3, base_cents = ?4, taxed = ?5, cadence = ?6
          WHERE id = ?1",
        params![
            id,
            entry.name,
            entry.month,
            entry.base_cents.0,
            entry.taxed as i64,
            entry.cadence.as_str()
        ],
    )?;
    ensure!(changed == 1, "no recurring goal with id {id}");
    Ok(())
}

/// How many goals reference this entry, open or closed.
///
/// Closed ones count: `goal::has_goal_dated_in_year` reads them, and it is what
/// tells a biennial entry whether the year ahead is its turn or the one it
/// skips. A round that has been through and been closed out has still been
/// through.
pub fn goal_count(db: &Db, id: RecurringGoalId) -> Result<i64> {
    let n: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM goal WHERE recurring_goal_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Delete an entry, refusing while any goal still references it.
///
/// Orphaning `goal.recurring_goal_id` would leave the picker unable to date
/// the next round -- a foreign key SQLite would not complain about, because
/// the column is nullable and the row would simply dangle.
pub fn delete(db: &Db, id: RecurringGoalId) -> Result<()> {
    let goals = goal_count(db, id)?;
    ensure!(
        goals == 0,
        "{goals} goal(s), open or closed, still reference this recurring goal; \
         a recurring goal cannot be deleted while any round of it exists"
    );
    let removed = db
        .conn
        .execute("DELETE FROM recurring_goal WHERE id = ?1", params![id])?;
    ensure!(removed == 1, "no recurring goal with id {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn cadence_as_str_and_from_str_round_trip() {
        assert_eq!(Cadence::Annual.as_str(), "annual");
        assert_eq!(Cadence::Biennial.as_str(), "biennial");
        assert_eq!("annual".parse::<Cadence>().unwrap(), Cadence::Annual);
        assert_eq!("biennial".parse::<Cadence>().unwrap(), Cadence::Biennial);
    }

    /// The workbook says "biannual"; only `annual` and `biennial` are stored,
    /// and the schema's `CHECK` enforces the same pair.
    #[test]
    fn from_str_rejects_an_unknown_cadence() {
        assert!("biannual".parse::<Cadence>().is_err());
    }

    #[test]
    fn insert_and_list_round_trip_every_field() {
        let db = db::open_in_memory().unwrap();
        insert(
            &db,
            &NewEntry {
                name: "Car Insurance".to_string(),
                month: 3,
                base_cents: Cents::from_dollars(1_200),
                taxed: false,
                cadence: Cadence::Annual,
            },
        )
        .unwrap();
        insert(
            &db,
            &NewEntry {
                name: "Passport Renewal".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(165),
                taxed: true,
                cadence: Cadence::Biennial,
            },
        )
        .unwrap();

        let entries = list(&db).unwrap();
        assert_eq!(entries.len(), 2);
        // Distinct values in every field, so a transposed `select_recurring_goal!` ordering
        // cannot pass.
        assert_eq!(entries[0].name, "Car Insurance");
        assert_eq!(entries[0].month, 3);
        assert_eq!(entries[0].base_cents, Cents::from_dollars(1_200));
        assert!(!entries[0].taxed);
        assert_eq!(entries[0].cadence, Cadence::Annual);
        assert_eq!(entries[1].name, "Passport Renewal");
        assert_eq!(entries[1].month, 11);
        assert_eq!(entries[1].base_cents, Cents::from_dollars(165));
        assert!(entries[1].taxed);
        assert_eq!(entries[1].cadence, Cadence::Biennial);
    }

    #[test]
    fn list_is_empty_for_a_fresh_database() {
        let db = db::open_in_memory().unwrap();
        assert!(list(&db).unwrap().is_empty());
    }

    /// "Open?" is a hint, never a block: goal names are not unique, so a
    /// second open goal against one entry is a legitimate thing to want.
    /// Closed goals must not count, or last year's Lego would hide this
    /// year's.
    #[test]
    fn open_goal_counts_counts_only_open_goals_of_each_entry() {
        let db = db::open_in_memory().unwrap();
        let savings =
            crate::db::account::insert(&db, "SAV", "Rainy Day", crate::db::account::Kind::Cash, 0)
                .unwrap();
        let lego = insert(
            &db,
            &NewEntry {
                name: "Lego".to_string(),
                month: 12,
                base_cents: Cents::from_dollars(340),
                taxed: false,
                cadence: Cadence::Annual,
            },
        )
        .unwrap();
        let backblaze = insert(
            &db,
            &NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: Cadence::Biennial,
            },
        )
        .unwrap();

        let new_goal = |name: &str, recurring_goal_id| crate::db::goal::NewGoal {
            name: name.to_string(),
            container_account_id: savings,
            base_cents: Cents::from_dollars(340),
            goal_date: None,
            recurring_goal_id: Some(recurring_goal_id),
            interest_eligible: true,
            sort: 0,
            taxed: false,
            floating: false,
        };
        crate::db::goal::insert(&db, &new_goal("Lego", lego)).unwrap();
        crate::db::goal::insert(&db, &new_goal("Lego", lego)).unwrap();
        let closed = crate::db::goal::insert(&db, &new_goal("Backblaze", backblaze)).unwrap();
        crate::db::goal::close(&db, closed).unwrap();

        let counts = open_goal_counts(&db).unwrap();
        assert_eq!(counts.get(&lego), Some(&2));
        assert_eq!(counts.get(&backblaze), None, "a closed goal is not open");
    }

    #[test]
    fn update_rewrites_every_field_of_an_entry() {
        let db = db::open_in_memory().unwrap();
        let id = insert(
            &db,
            &NewEntry {
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: false,
                cadence: Cadence::Annual,
            },
        )
        .unwrap();

        update(
            &db,
            id,
            &NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: true,
                cadence: Cadence::Biennial,
            },
        )
        .unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.name, "Backblaze");
        assert_eq!(found.month, 11);
        assert_eq!(found.base_cents, Cents::from_dollars(99));
        assert!(found.taxed);
        assert_eq!(found.cadence, Cadence::Biennial);
    }

    /// Orphaning `goal.recurring_goal_id` would break
    /// `goal::has_goal_dated_in_year`, which is what decides whether a
    /// biennial entry is due next year or the year after. Closed goals count
    /// for exactly that reason -- a round that has been through and been
    /// closed out has still been through.
    #[test]
    fn deleting_an_entry_that_any_goal_references_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings =
            crate::db::account::insert(&db, "SAV", "Rainy Day", crate::db::account::Kind::Cash, 0)
                .unwrap();
        let id = insert(
            &db,
            &NewEntry {
                name: "Lego".to_string(),
                month: 12,
                base_cents: Cents::from_dollars(340),
                taxed: false,
                cadence: Cadence::Annual,
            },
        )
        .unwrap();
        let goal_id = crate::db::goal::insert(
            &db,
            &crate::db::goal::NewGoal {
                name: "Lego".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(340),
                goal_date: None,
                recurring_goal_id: Some(id),
                interest_eligible: true,
                sort: 0,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();
        crate::db::goal::close(&db, goal_id).unwrap();

        assert_eq!(
            goal_count(&db, id).unwrap(),
            1,
            "a closed goal still counts"
        );
        let err = delete(&db, id).unwrap_err();
        assert!(err.to_string().contains("1 goal(s)"), "{err}");
        assert!(get(&db, id).is_ok());
    }

    #[test]
    fn deleting_an_unreferenced_entry_removes_it() {
        let db = db::open_in_memory().unwrap();
        let id = insert(
            &db,
            &NewEntry {
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: false,
                cadence: Cadence::Annual,
            },
        )
        .unwrap();

        delete(&db, id).unwrap();

        assert!(get(&db, id).is_err());
        assert!(delete(&db, id).is_err(), "a second delete is an error");
    }
}
