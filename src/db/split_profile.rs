//! The `split_profile` table: named sets of the three Planning splits, one
//! of them active.

use super::{Db, SplitProfileId};
use crate::rate::BasisPoints;
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Row, params};

/// The three shares a profile holds. Goals takes what they leave.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Shares {
    pub future_housing: BasisPoints,
    pub retirement: BasisPoints,
    pub investment: BasisPoints,
}

impl Shares {
    pub fn total(self) -> BasisPoints {
        self.future_housing + self.retirement + self.investment
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitProfile {
    pub id: SplitProfileId,
    pub name: String,
    pub shares: Shares,
    /// Whether the waterfall reads this profile. Exactly one row carries it:
    /// the schema's partial index holds it to at most one, and every writer
    /// here leaves one.
    pub active: bool,
}

// Column order is fixed by `select_split_profile!` below -- keep the two in
// sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<SplitProfile> {
    Ok(SplitProfile {
        id: row.get(0)?,
        name: row.get(1)?,
        shares: Shares {
            future_housing: BasisPoints(row.get(2)?),
            retirement: BasisPoints(row.get(3)?),
            investment: BasisPoints(row.get(4)?),
        },
        active: row.get(5)?,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_split_profile {
    ($tail:literal) => {
        concat!(
            "SELECT id, name, future_housing_bp, retirement_bp, investment_bp, active \
               FROM split_profile ",
            $tail
        )
    };
}

/// Every profile, in the order they were created -- the order `←`/`→` steps
/// through them in.
pub fn list(db: &Db) -> Result<Vec<SplitProfile>> {
    let mut stmt = db.conn.prepare(select_split_profile!("ORDER BY id"))?;
    let rows = stmt.query_map([], from_row)?;
    super::collect_rows(rows)
}

pub fn get(db: &Db, id: SplitProfileId) -> Result<SplitProfile> {
    db.conn
        .query_row(
            select_split_profile!("WHERE id = ?1"),
            params![id],
            from_row,
        )
        .optional()?
        .with_context(|| format!("no split profile with id {id}"))
}

/// The profile the waterfall reads.
///
/// None is a corrupt database rather than an unconfigured one: the migration
/// that created the table seeded an active row, and no writer here leaves the
/// table without one.
pub fn active(db: &Db) -> Result<SplitProfile> {
    db.conn
        .query_row(select_split_profile!("WHERE active = 1"), [], from_row)
        .optional()?
        .context("no active split profile")
}

/// A typed name, trimmed, refused when blank or when another profile already
/// carries it in any case -- the cycle is read by name, so two that read the
/// same are one too many. `except` is the profile being renamed, which may
/// keep its own name.
fn checked_name(db: &Db, name: &str, except: Option<SplitProfileId>) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "a split profile needs a name");
    let clash: Option<String> = db
        .conn
        .query_row(
            "SELECT name FROM split_profile WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2",
            params![name, except],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(existing) = clash {
        anyhow::bail!("a split profile is already called {existing:?}");
    }
    Ok(name.to_string())
}

/// A new profile, inactive. [`insert_active`] is the one that also takes
/// the flag.
pub fn insert(db: &Db, name: &str, shares: Shares) -> Result<SplitProfileId> {
    let name = checked_name(db, name, None)?;
    db.conn.execute(
        "INSERT INTO split_profile (name, future_housing_bp, retirement_bp, investment_bp)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            name,
            shares.future_housing.0,
            shares.retirement.0,
            shares.investment.0
        ],
    )?;
    Ok(SplitProfileId(db.conn.last_insert_rowid()))
}

/// A new profile that becomes the active one, in one transaction: an insert
/// committed without the flag would leave a profile the owner never sees
/// selected, under a name a retry is then refused for.
pub fn insert_active(db: &Db, name: &str, shares: Shares) -> Result<SplitProfileId> {
    db.transaction(|db| {
        let id = insert(db, name, shares)?;
        activate(db, id)?;
        Ok(id)
    })
}

pub fn rename(db: &Db, id: SplitProfileId, name: &str) -> Result<()> {
    get(db, id)?;
    let name = checked_name(db, name, Some(id))?;
    db.conn.execute(
        "UPDATE split_profile SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    Ok(())
}

pub fn set_shares(db: &Db, id: SplitProfileId, shares: Shares) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE split_profile
            SET future_housing_bp = ?2, retirement_bp = ?3, investment_bp = ?4
          WHERE id = ?1",
        params![
            id,
            shares.future_housing.0,
            shares.retirement.0,
            shares.investment.0
        ],
    )?;
    ensure!(changed == 1, "no split profile with id {id}");
    Ok(())
}

/// Make `id` the profile the waterfall reads, and no other.
///
/// Two statements in one transaction rather than one `UPDATE`: SQLite checks
/// a unique index row by row, so setting one row and clearing another in a
/// single statement can trip the one-active index halfway through.
pub fn set_active(db: &Db, id: SplitProfileId) -> Result<()> {
    // Read for the check alone: without it an unknown id would clear the flag
    // from every row and set it on none.
    get(db, id)?;
    db.transaction(|db| activate(db, id))
}

/// [`set_active`]'s writes, for a caller already inside a transaction.
fn activate(db: &Db, id: SplitProfileId) -> Result<()> {
    db.conn
        .execute("UPDATE split_profile SET active = 0 WHERE active = 1", [])?;
    db.conn.execute(
        "UPDATE split_profile SET active = 1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// Delete a profile, returning the one active afterwards.
///
/// The last profile is refused, since the waterfall has nothing to read
/// without one. Deleting the active profile hands the flag to the next by id,
/// or to the one before it when it was the last -- the neighbour beside it,
/// rather than wrapping round to the first the way `→` does.
pub fn delete(db: &Db, id: SplitProfileId) -> Result<SplitProfileId> {
    let profiles = list(db)?;
    let at = profiles
        .iter()
        .position(|p| p.id == id)
        .with_context(|| format!("no split profile with id {id}"))?;
    ensure!(
        profiles.len() > 1,
        "the last split profile cannot be deleted"
    );
    if !profiles[at].active {
        db.conn
            .execute("DELETE FROM split_profile WHERE id = ?1", params![id])?;
        return Ok(active(db)?.id);
    }
    let now_active = profiles.get(at + 1).unwrap_or_else(|| &profiles[at - 1]).id;
    db.transaction(|db| {
        db.conn
            .execute("DELETE FROM split_profile WHERE id = ?1", params![id])?;
        activate(db, now_active)
    })?;
    Ok(now_active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn shares(fh: i64, rt: i64, inv: i64) -> Shares {
        Shares {
            future_housing: BasisPoints(fh),
            retirement: BasisPoints(rt),
            investment: BasisPoints(inv),
        }
    }

    fn active_count(db: &Db) -> usize {
        list(db).unwrap().iter().filter(|p| p.active).count()
    }

    #[test]
    fn a_fresh_database_starts_with_one_active_default_profile_at_thirty_twenty_ten() {
        let db = db::open_in_memory().unwrap();
        let found = active(&db).unwrap();
        assert_eq!(found.name, "Default");
        assert_eq!(found.shares, shares(3_000, 2_000, 1_000));
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    #[test]
    fn shares_total_the_three_splits() {
        assert_eq!(shares(1_000, 2_000, 3_000).total(), BasisPoints(6_000));
    }

    #[test]
    fn a_new_profile_is_inserted_inactive_with_its_shares() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "  Maxed ", shares(1_000, 4_000, 2_000)).unwrap();
        let found = get(&db, id).unwrap();
        assert_eq!(found.name, "Maxed", "the name is trimmed");
        assert_eq!(found.shares, shares(1_000, 4_000, 2_000));
        assert!(!found.active);
        assert_eq!(active(&db).unwrap().name, "Default");
    }

    #[test]
    fn set_active_leaves_exactly_one_profile_active() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        set_active(&db, id).unwrap();
        assert_eq!(active(&db).unwrap().name, "Maxed");
        assert_eq!(active_count(&db), 1);
    }

    #[test]
    fn activating_a_missing_profile_is_refused_and_changes_nothing() {
        let db = db::open_in_memory().unwrap();
        assert!(set_active(&db, SplitProfileId(999)).is_err());
        assert_eq!(active(&db).unwrap().name, "Default");
    }

    #[test]
    fn insert_active_adds_the_profile_and_hands_it_the_flag() {
        let db = db::open_in_memory().unwrap();
        let id = insert_active(&db, "Maxed", shares(1_000, 4_000, 2_000)).unwrap();
        assert_eq!(active(&db).unwrap().id, id);
        assert_eq!(active_count(&db), 1);
        assert!(insert_active(&db, "maxed", shares(0, 0, 0)).is_err());
        assert_eq!(
            active(&db).unwrap().id,
            id,
            "a refused insert moves nothing"
        );
    }

    #[test]
    fn profiles_list_in_the_order_they_were_created() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        insert(&db, "Contributing", shares(0, 0, 0)).unwrap();
        let names: Vec<String> = list(&db).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Default", "Maxed", "Contributing"]);
    }

    #[test]
    fn deleting_the_active_profile_activates_the_next_one_by_id() {
        let db = db::open_in_memory().unwrap();
        let first = active(&db).unwrap().id;
        let next = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        insert(&db, "Contributing", shares(0, 0, 0)).unwrap();
        assert_eq!(delete(&db, first).unwrap(), next);
        assert_eq!(active(&db).unwrap().id, next);
        assert_eq!(active_count(&db), 1);
    }

    #[test]
    fn deleting_the_last_listed_active_profile_activates_the_one_before_it() {
        let db = db::open_in_memory().unwrap();
        let first = active(&db).unwrap().id;
        let last = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        set_active(&db, last).unwrap();
        assert_eq!(delete(&db, last).unwrap(), first);
        assert_eq!(active(&db).unwrap().id, first);
    }

    #[test]
    fn deleting_an_inactive_profile_leaves_the_active_one_alone() {
        let db = db::open_in_memory().unwrap();
        let first = active(&db).unwrap().id;
        let other = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        assert_eq!(delete(&db, other).unwrap(), first);
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    #[test]
    fn the_last_profile_cannot_be_deleted() {
        let db = db::open_in_memory().unwrap();
        let only = active(&db).unwrap().id;
        let err = delete(&db, only).unwrap_err().to_string();
        assert!(err.contains("last split profile"), "{err}");
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    #[test]
    fn a_name_already_used_in_another_case_is_refused() {
        let db = db::open_in_memory().unwrap();
        let err = insert(&db, "default", shares(0, 0, 0))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"Default\""),
            "names the profile already holding it: {err}"
        );
        let id = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        assert!(rename(&db, id, "DEFAULT").is_err());
    }

    #[test]
    fn a_blank_name_is_refused() {
        let db = db::open_in_memory().unwrap();
        assert!(insert(&db, "   ", shares(0, 0, 0)).is_err());
        let id = active(&db).unwrap().id;
        assert!(rename(&db, id, "").is_err());
    }

    #[test]
    fn renaming_a_profile_to_its_own_name_in_another_case_is_allowed() {
        let db = db::open_in_memory().unwrap();
        let id = active(&db).unwrap().id;
        rename(&db, id, "DEFAULT").unwrap();
        assert_eq!(get(&db, id).unwrap().name, "DEFAULT");
    }

    #[test]
    fn set_shares_rewrites_only_that_profile() {
        let db = db::open_in_memory().unwrap();
        let first = active(&db).unwrap().id;
        let other = insert(&db, "Maxed", shares(0, 0, 0)).unwrap();
        set_shares(&db, other, shares(500, 600, 700)).unwrap();
        assert_eq!(get(&db, other).unwrap().shares, shares(500, 600, 700));
        assert_eq!(get(&db, first).unwrap().shares, shares(3_000, 2_000, 1_000));
    }

    #[test]
    fn a_missing_profile_is_an_error_not_an_absence() {
        let db = db::open_in_memory().unwrap();
        assert!(get(&db, SplitProfileId(999)).is_err());
        assert!(rename(&db, SplitProfileId(999), "Maxed").is_err());
        assert!(set_shares(&db, SplitProfileId(999), shares(0, 0, 0)).is_err());
    }
}
