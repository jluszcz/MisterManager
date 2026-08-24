//! The `bill` table: the monthly recurring costs of `Planning!C6:E12`.
//!
//! Two categories rather than one list with a flag, because `calc::planning`
//! reports `housing_biweekly` and `other_bills_biweekly` separately and only
//! the first reaches `lines.current_housing`. The split is therefore
//! load-bearing, not presentation.

use super::{BillId, Db};
use crate::money::Cents;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{OptionalExtension, Row, params};
use std::str::FromStr;

/// Which subtotal a bill belongs to.
///
/// The variants are exactly the schema's `CHECK (category IN (...))` list:
/// keep the two in step, or an insert that type-checks will fail against the
/// constraint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Category {
    Housing,
    Other,
}

impl Category {
    /// Both categories, for callers that must cover each.
    pub const ALL: [Category; 2] = [Category::Housing, Category::Other];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Housing => "housing",
            Category::Other => "other",
        }
    }
}

impl FromStr for Category {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "housing" => Ok(Category::Housing),
            "other" => Ok(Category::Other),
            other => bail!("unknown bill category {other:?}"),
        }
    }
}

/// A bill to be recorded, before it has an id.
#[derive(Clone, Debug)]
pub struct NewBill {
    pub label: String,
    /// The monthly figure, as sheet column D holds it. The biweekly column is
    /// derived by `calc::biweekly` and never stored.
    pub cents: Cents,
    pub category: Category,
    pub sort: i64,
}

/// The editable half of a bill. `sort` is the sheet's own ordering and is not
/// an edit: a bill moved between categories keeps its number, and ties break
/// by id.
#[derive(Clone, Debug)]
pub struct BillEdit {
    pub label: String,
    pub cents: Cents,
    pub category: Category,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bill {
    pub id: BillId,
    pub label: String,
    pub cents: Cents,
    pub category: Category,
    pub sort: i64,
}

// Column order is fixed by `select_bill!` below -- keep the two in sync.
fn from_row(row: &Row<'_>) -> rusqlite::Result<Bill> {
    let category: String = row.get(3)?;
    Ok(Bill {
        id: row.get(0)?,
        label: row.get(1)?,
        cents: Cents(row.get(2)?),
        category: category
            .parse()
            .expect("schema CHECK guarantees a valid category"),
        sort: row.get(4)?,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_bill {
    ($tail:literal) => {
        concat!("SELECT id, label, cents, category, sort FROM bill ", $tail)
    };
}

pub fn insert(db: &Db, bill: &NewBill) -> Result<BillId> {
    db.conn.execute(
        "INSERT INTO bill (label, cents, category, sort) VALUES (?1, ?2, ?3, ?4)",
        params![bill.label, bill.cents.0, bill.category.as_str(), bill.sort],
    )?;
    Ok(BillId(db.conn.last_insert_rowid()))
}

/// One category's bills, in the order the sheet lists them.
pub fn list(db: &Db, category: Category) -> Result<Vec<Bill>> {
    let mut stmt = db
        .conn
        .prepare(select_bill!("WHERE category = ?1 ORDER BY sort, id"))?;
    let rows = stmt.query_map(params![category.as_str()], from_row)?;
    super::collect_rows(rows)
}

/// Just the amounts, in the same order -- what `PlanInputs` takes.
pub fn amounts(db: &Db, category: Category) -> Result<Vec<Cents>> {
    Ok(list(db, category)?.into_iter().map(|b| b.cents).collect())
}

/// One bill by id. A missing bill is an error, not `None` -- the same
/// rule as [`super::account::get`]: an id read off another row is a foreign
/// key, and a dangling one is a corrupt database.
pub fn get(db: &Db, id: BillId) -> Result<Bill> {
    db.conn
        .query_row(select_bill!("WHERE id = ?1"), params![id], from_row)
        .optional()?
        .with_context(|| format!("no bill with id {id}"))
}

/// Where a newly added bill lands: at the end of its own category.
pub fn next_sort(db: &Db, category: Category) -> Result<i64> {
    let next: i64 = db.conn.query_row(
        "SELECT COALESCE(MAX(sort) + 1, 0) FROM bill WHERE category = ?1",
        params![category.as_str()],
        |r| r.get(0),
    )?;
    Ok(next)
}

pub fn update(db: &Db, id: BillId, edit: &BillEdit) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE bill SET label = ?2, cents = ?3, category = ?4 WHERE id = ?1",
        params![id, edit.label, edit.cents.0, edit.category.as_str()],
    )?;
    ensure!(changed == 1, "no bill with id {id}");
    Ok(())
}

/// Rewrite one bill's monthly amount, which is what the Planning screen's `e`
/// edits: the row shows an amount, so that is what the one-field editor sets.
pub fn set_amount(db: &Db, id: BillId, cents: Cents) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE bill SET cents = ?2 WHERE id = ?1",
        params![id, cents.0],
    )?;
    ensure!(changed == 1, "no bill with id {id}");
    Ok(())
}

pub fn delete(db: &Db, id: BillId) -> Result<()> {
    let removed = db
        .conn
        .execute("DELETE FROM bill WHERE id = ?1", params![id])?;
    ensure!(removed == 1, "no bill with id {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn housing(label: &str, cents: i64, sort: i64) -> NewBill {
        NewBill {
            label: label.to_string(),
            cents: Cents::from_dollars(cents),
            category: Category::Housing,
            sort,
        }
    }

    fn other(label: &str, cents: i64, sort: i64) -> NewBill {
        NewBill {
            label: label.to_string(),
            cents: Cents::from_dollars(cents),
            category: Category::Other,
            sort,
        }
    }

    /// The workbook's whole bill block, in sheet order.
    fn seeded() -> db::Db {
        let db = db::open_in_memory().unwrap();
        insert(&db, &housing("Mortgage", 1_200, 0)).unwrap();
        insert(&db, &housing("HOA", 300, 1)).unwrap();
        insert(&db, &other("Plumber", 90, 0)).unwrap();
        insert(&db, &other("Phone", 60, 1)).unwrap();
        insert(&db, &other("Newspaper", 25, 2)).unwrap();
        insert(&db, &other("Coworking", 1_000, 3)).unwrap();
        db
    }

    #[test]
    fn category_as_str_and_from_str_round_trip() {
        for category in Category::ALL {
            assert_eq!(category.as_str().parse::<Category>().unwrap(), category);
        }
        assert!("rent".parse::<Category>().is_err());
    }

    /// The enum and the schema's `CHECK (category IN (...))` are two
    /// independent lists of the same two strings. A variant missing from the
    /// constraint type-checks and then fails at runtime.
    #[test]
    fn every_category_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        for category in Category::ALL {
            insert(
                &db,
                &NewBill {
                    label: "Anything".to_string(),
                    cents: Cents::from_dollars(1),
                    category,
                    sort: 0,
                },
            )
            .unwrap_or_else(|e| panic!("{category:?} is not in the schema's CHECK list: {e}"));
        }
    }

    /// Distinct values in every field, so a transposed `select_bill!` ordering
    /// cannot pass.
    #[test]
    fn insert_and_list_round_trip_every_field() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &housing("Mortgage", 1_200, 7)).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.label, "Mortgage");
        assert_eq!(found.cents, Cents::from_dollars(1_200));
        assert_eq!(found.category, Category::Housing);
        assert_eq!(found.sort, 7);
    }

    /// Housing and the other bills are two separate subtotals in the
    /// waterfall, and only the first reaches `lines.current_housing`. A
    /// `list` that leaked one into the other would move money between two
    /// transfer instructions.
    #[test]
    fn list_is_scoped_to_its_category_and_ordered_by_sort() {
        let db = seeded();
        let labels = |category| -> Vec<String> {
            list(&db, category)
                .unwrap()
                .into_iter()
                .map(|b| b.label)
                .collect()
        };
        assert_eq!(labels(Category::Housing), ["Mortgage", "HOA"]);
        assert_eq!(
            labels(Category::Other),
            ["Plumber", "Phone", "Newspaper", "Coworking"]
        );
    }

    /// `PlanInputs` takes bare amounts, in sheet order. These are
    /// `Planning!D7:D12`.
    #[test]
    fn amounts_are_the_column_d_figures_in_sheet_order() {
        let db = seeded();
        let d = Cents::from_dollars;
        assert_eq!(
            amounts(&db, Category::Housing).unwrap(),
            vec![d(1_200), d(300)]
        );
        assert_eq!(
            amounts(&db, Category::Other).unwrap(),
            vec![d(90), d(60), d(25), d(1_000)]
        );
    }

    #[test]
    fn amounts_of_a_database_with_no_bills_is_empty() {
        let db = db::open_in_memory().unwrap();
        assert!(amounts(&db, Category::Housing).unwrap().is_empty());
    }

    #[test]
    fn next_sort_is_one_past_the_last_bill_of_its_category() {
        let db = seeded();
        assert_eq!(next_sort(&db, Category::Housing).unwrap(), 2);
        assert_eq!(next_sort(&db, Category::Other).unwrap(), 4);

        let empty = db::open_in_memory().unwrap();
        assert_eq!(next_sort(&empty, Category::Housing).unwrap(), 0);
    }

    #[test]
    fn update_rewrites_the_label_amount_and_category() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &other("Coworking", 1_000, 3)).unwrap();

        update(
            &db,
            id,
            &BillEdit {
                label: "Office".to_string(),
                cents: Cents::from_dollars(900),
                category: Category::Housing,
            },
        )
        .unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.label, "Office");
        assert_eq!(found.cents, Cents::from_dollars(900));
        assert_eq!(found.category, Category::Housing);
        assert_eq!(found.sort, 3, "sort is not the edit's to change");
    }

    /// `e` on a bill row edits the figure the row shows and nothing else.
    #[test]
    fn set_amount_leaves_the_label_and_category_alone() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, &housing("Mortgage", 1_200, 0)).unwrap();

        set_amount(&db, id, Cents::from_dollars(1_500)).unwrap();

        let found = get(&db, id).unwrap();
        assert_eq!(found.cents, Cents::from_dollars(1_500));
        assert_eq!(found.label, "Mortgage");
        assert_eq!(found.category, Category::Housing);
    }

    #[test]
    fn deleting_a_bill_removes_only_that_row() {
        let db = seeded();
        let plumber = list(&db, Category::Other).unwrap()[0].id;

        delete(&db, plumber).unwrap();

        let labels: Vec<String> = list(&db, Category::Other)
            .unwrap()
            .into_iter()
            .map(|b| b.label)
            .collect();
        assert_eq!(labels, ["Phone", "Newspaper", "Coworking"]);
        assert_eq!(list(&db, Category::Housing).unwrap().len(), 2);
    }

    /// A silent no-op here would let the screen report a delete that did not
    /// happen, and the row would come back on the next reload.
    #[test]
    fn updating_deleting_or_repricing_a_missing_bill_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(delete(&db, BillId(999)).is_err());
        assert!(set_amount(&db, BillId(999), Cents::ZERO).is_err());
        assert!(
            update(
                &db,
                BillId(999),
                &BillEdit {
                    label: "Ghost".to_string(),
                    cents: Cents::ZERO,
                    category: Category::Other,
                },
            )
            .is_err()
        );
        assert!(get(&db, BillId(999)).is_err());
    }
}
