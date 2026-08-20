//! The `fund` table: the asset-allocation block of `Planning!I1:M5`.
//!
//! Three rows in the workbook, each with a target share the sheet derives, a
//! value typed by hand, and the gap between them. Only the name, the order,
//! the rule that decides the target, and the value are stored — the target
//! percentage itself is derived by `calc::fund` on every read, because a
//! birthday moves it with no write.

use super::{Db, FundId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{OptionalExtension, Row, params};

/// What decides a fund's target share.
///
/// The variants are exactly the schema's two `CHECK` clauses: the kind
/// strings it lists, and the pairing of each with whether `share_bp` is
/// present. Keep the three in step, or an insert that type-checks will fail
/// against the constraint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// Bonds track age: one percentage point per year over thirty. The
    /// thirty is `calc::fund::BONDS_START_AGE`; nothing here knows it.
    AgeOver30,
    /// This share of whatever the age rule leaves.
    RemainderShare(BasisPoints),
}

impl Target {
    /// One of each variant, for callers that must cover both: the form's kind
    /// selector, and the test that checks the schema still accepts them.
    ///
    /// The share carried here is a placeholder — a caller cycling the
    /// selector reads the *variant* and takes the share from its own field.
    pub const KINDS: [Target; 2] = [Target::AgeOver30, Target::RemainderShare(BasisPoints::ZERO)];

    pub fn kind_str(self) -> &'static str {
        match self {
            Target::AgeOver30 => "age_over_30",
            Target::RemainderShare(_) => "remainder_share",
        }
    }

    /// What the `share_bp` column holds for this target. `None` is not a
    /// missing value — it is the age row saying it claims no share.
    pub fn share_bp(self) -> Option<i64> {
        match self {
            Target::AgeOver30 => None,
            Target::RemainderShare(share) => Some(share.0),
        }
    }

    /// The two columns, back into one value.
    ///
    /// Every mismatch here is refused by the schema's paired `CHECK`, so this
    /// only ever errors against a database written by something other than
    /// this crate.
    pub fn from_stored(kind: &str, share_bp: Option<i64>) -> Result<Target> {
        match (kind, share_bp) {
            ("age_over_30", None) => Ok(Target::AgeOver30),
            ("remainder_share", Some(share)) => Ok(Target::RemainderShare(BasisPoints(share))),
            ("age_over_30" | "remainder_share", _) => {
                bail!("fund kind {kind:?} does not go with share_bp {share_bp:?}")
            }
            other => bail!("unknown fund kind {other:?}"),
        }
    }
}

/// A fund to be recorded, before it has an id.
#[derive(Clone, Debug)]
pub struct NewFund {
    pub name: String,
    /// Display order: sheet row order at import, `next_ord` for one added by
    /// hand.
    pub ord: i64,
    pub target: Target,
    /// What the fund holds now. Typed or imported; nothing reconciles it
    /// against a balance.
    pub actual: Cents,
}

/// The editable half of a fund. `ord` is not an edit.
#[derive(Clone, Debug)]
pub struct FundEdit {
    pub name: String,
    pub target: Target,
    pub actual: Cents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fund {
    pub id: FundId,
    pub name: String,
    pub ord: i64,
    pub target: Target,
    pub actual: Cents,
}

// Column order is fixed by `select_fund!` below -- keep the two in sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<Fund> {
    let kind: String = row.get(3)?;
    let share_bp: Option<i64> = row.get(4)?;
    Ok(Fund {
        id: row.get(0)?,
        name: row.get(1)?,
        ord: row.get(2)?,
        target: Target::from_stored(&kind, share_bp)
            .expect("the schema's CHECK clauses guarantee a valid target"),
        actual: Cents(row.get(5)?),
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. One list per table -- see [`crate::db`] for the
/// idiom.
macro_rules! select_fund {
    ($tail:literal) => {
        concat!(
            "SELECT id, name, ord, kind, share_bp, actual_cents FROM fund ",
            $tail
        )
    };
}

pub fn insert(db: &Db, fund: &NewFund) -> Result<FundId> {
    db.conn.execute(
        "INSERT INTO fund (name, ord, kind, share_bp, actual_cents) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            fund.name,
            fund.ord,
            fund.target.kind_str(),
            fund.target.share_bp(),
            fund.actual.0
        ],
    )?;
    Ok(FundId(db.conn.last_insert_rowid()))
}

/// Every fund, in the order the screen shows them.
pub fn list(db: &Db) -> Result<Vec<Fund>> {
    let mut stmt = db.conn.prepare(select_fund!("ORDER BY ord, id"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One fund by id. A missing fund is an error, not `None` -- the same
/// rule as [`super::account::get`]: an id read off another row is a foreign
/// key, and a dangling one is a corrupt database.
pub fn get(db: &Db, id: FundId) -> Result<Fund> {
    db.conn
        .query_row(select_fund!("WHERE id = ?1"), params![id], from_row)
        .optional()?
        .with_context(|| format!("no fund with id {id}"))
}

pub fn count(db: &Db) -> Result<i64> {
    Ok(db
        .conn
        .query_row("SELECT COUNT(*) FROM fund", [], |r| r.get(0))?)
}

/// Where a newly added fund lands: at the end.
pub fn next_ord(db: &Db) -> Result<i64> {
    let next: i64 = db
        .conn
        .query_row("SELECT COALESCE(MAX(ord) + 1, 0) FROM fund", [], |r| {
            r.get(0)
        })?;
    Ok(next)
}

pub fn update(db: &Db, id: FundId, edit: &FundEdit) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE fund SET name = ?2, kind = ?3, share_bp = ?4, actual_cents = ?5 WHERE id = ?1",
        params![
            id,
            edit.name,
            edit.target.kind_str(),
            edit.target.share_bp(),
            edit.actual.0
        ],
    )?;
    ensure!(changed == 1, "no fund with id {id}");
    Ok(())
}

/// Rewrite one fund's value, which is what the screen's `e` edits: the row
/// shows a figure, so that is what the one-field editor sets.
pub fn set_actual(db: &Db, id: FundId, actual: Cents) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE fund SET actual_cents = ?2 WHERE id = ?1",
        params![id, actual.0],
    )?;
    ensure!(changed == 1, "no fund with id {id}");
    Ok(())
}

pub fn delete(db: &Db, id: FundId) -> Result<()> {
    let removed = db
        .conn
        .execute("DELETE FROM fund WHERE id = ?1", params![id])?;
    ensure!(removed == 1, "no fund with id {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn bonds() -> NewFund {
        NewFund {
            name: "Bonds".to_string(),
            ord: 0,
            target: Target::AgeOver30,
            actual: Cents::from_dollars(30_000),
        }
    }

    fn international() -> NewFund {
        NewFund {
            name: "International".to_string(),
            ord: 1,
            target: Target::RemainderShare(BasisPoints(4_000)),
            actual: Cents::from_dollars(60_000),
        }
    }

    /// The workbook's three funds, in sheet order.
    fn seeded() -> db::Db {
        let db = db::open_in_memory().unwrap();
        insert(&db, &bonds()).unwrap();
        insert(&db, &international()).unwrap();
        insert(
            &db,
            &NewFund {
                name: "Domestic".to_string(),
                ord: 2,
                target: Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        db
    }

    /// Distinct values in every field, so a transposed `select_fund!` ordering
    /// cannot pass.
    #[test]
    fn insert_and_get_round_trip_every_field() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &international()).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.name, "International");
        assert_eq!(found.ord, 1);
        assert_eq!(found.target, Target::RemainderShare(BasisPoints(4_000)));
        assert_eq!(found.actual, Cents::from_dollars(60_000));
    }

    /// The enum and the schema's two `CHECK` clauses are independent
    /// statements of the same rule. A variant the constraint refuses
    /// type-checks and then fails at runtime.
    #[test]
    fn every_target_kind_satisfies_the_schema_constraints() {
        let db = db::open_in_memory().unwrap();
        for target in Target::KINDS {
            insert(
                &db,
                &NewFund {
                    name: "Anything".to_string(),
                    ord: 0,
                    target,
                    actual: Cents::ZERO,
                },
            )
            .unwrap_or_else(|e| panic!("{target:?} is not storable: {e}"));
        }
    }

    /// The age row claims no share of the remainder, and says so by storing
    /// none: a zero would read as a real share of nothing.
    #[test]
    fn an_age_row_stores_no_share_and_a_share_row_stores_its_basis_points() {
        let db = seeded();
        let stored: Vec<Option<i64>> = list(&db)
            .unwrap()
            .iter()
            .map(|f| f.target.share_bp())
            .collect();
        assert_eq!(stored, vec![None, Some(4_000), Some(6_000)]);
    }

    #[test]
    fn list_is_ordered_by_ord_then_id() {
        let db = db::open_in_memory().unwrap();
        insert(
            &db,
            &NewFund {
                ord: 2,
                ..international()
            },
        )
        .unwrap();
        insert(&db, &bonds()).unwrap();

        let names: Vec<String> = list(&db).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["Bonds", "International"]);
    }

    #[test]
    fn next_ord_is_one_past_the_last_fund() {
        assert_eq!(next_ord(&seeded()).unwrap(), 3);
        assert_eq!(next_ord(&db::open_in_memory().unwrap()).unwrap(), 0);
    }

    #[test]
    fn update_rewrites_the_name_target_and_value_but_not_the_order() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &international()).unwrap();

        update(
            &db,
            id,
            &FundEdit {
                name: "Intl".to_string(),
                target: Target::AgeOver30,
                actual: Cents::from_dollars(1),
            },
        )
        .unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.name, "Intl");
        assert_eq!(found.target, Target::AgeOver30);
        assert_eq!(found.actual, Cents::from_dollars(1));
        assert_eq!(found.ord, 1, "ord is not the edit's to change");
    }

    /// `e` on a fund row edits the figure the row shows and nothing else.
    #[test]
    fn set_actual_leaves_the_name_and_target_alone() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &bonds()).unwrap();

        set_actual(&db, id, Cents::from_dollars(20_000)).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.actual, Cents::from_dollars(20_000));
        assert_eq!(found.name, "Bonds");
        assert_eq!(found.target, Target::AgeOver30);
    }

    #[test]
    fn deleting_a_fund_removes_only_that_row() {
        let db = seeded();
        let intl = list(&db).unwrap()[1].id;

        delete(&db, intl).unwrap();

        let names: Vec<String> = list(&db).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["Bonds", "Domestic"]);
    }

    /// A silent no-op would let the screen report a delete that did not
    /// happen, and the row would come back on the next reload.
    #[test]
    fn updating_deleting_or_repricing_a_missing_fund_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(delete(&db, FundId(999)).is_err());
        assert!(set_actual(&db, FundId(999), Cents::ZERO).is_err());
        assert!(
            update(
                &db,
                FundId(999),
                &FundEdit {
                    name: "Ghost".to_string(),
                    target: Target::AgeOver30,
                    actual: Cents::ZERO,
                },
            )
            .is_err()
        );
        assert!(get(&db, FundId(999)).is_err());
    }

    #[test]
    fn count_is_how_many_rows_the_table_holds() {
        assert_eq!(count(&seeded()).unwrap(), 3);
        assert_eq!(count(&db::open_in_memory().unwrap()).unwrap(), 0);
    }
}
