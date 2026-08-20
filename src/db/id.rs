//! Row ids, one type per table.
//!
//! Every id was an `i64`, so any of them could be passed where any other
//! belonged. Nothing rejected it: `goal::balance` and `goal::container_excess`
//! both take an id and both return a plausible `Cents`, and `shortfall` errors
//! only for an id that matches *no* row -- not for one that matches the wrong
//! row. Fresh tables make that easy to hit, since the first row of each is
//! id 1.
//!
//! `ToSql`/`FromSql` live here so the conversion stays inside `src/db/` and
//! these types can cross the boundary without dragging `rusqlite` with them.
//!
//! A container lookup takes the account id it means:
//!
//! ```
//! use mistermanager::db::{self, account::Kind};
//! let db = db::open_in_memory().unwrap();
//! let savings = db::account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
//! db::goal::container_excess(&db, savings).unwrap();
//! ```
//!
//! and refuses a goal id, which before this was the same type and returned a
//! plausible number instead:
//!
//! ```compile_fail
//! use mistermanager::db;
//! let db = db::open_in_memory().unwrap();
//! db::goal::container_excess(&db, db::GoalId(1)).unwrap();
//! ```

use rusqlite::types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Result as SqlResult, ToSql};
use std::fmt;

macro_rules! row_id {
    ($name:ident, $table:literal) => {
        #[doc = concat!("A `", $table, "` row id.")]
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub i64);

        impl ToSql for $name {
            fn to_sql(&self) -> SqlResult<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(value).map($name)
            }
        }

        /// Prints the bare number, so ids read the same in error messages as
        /// they do in the database.
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

row_id!(AccountId, "account");
row_id!(TxnId, "txn");
row_id!(RecurringTxnId, "recurring_txn");
row_id!(GoalId, "goal");
row_id!(RecurringGoalId, "recurring_goal");
row_id!(BatchId, "batch");
row_id!(AllocationId, "allocation");
row_id!(BillId, "bill");
row_id!(FundId, "fund");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Ids go out as integers and come back as the same integers -- if
    /// `ToSql` stored the debug form, or `FromSql` read the wrong column
    /// type, every lookup would miss.
    #[test]
    fn an_id_round_trips_through_sqlite() {
        let db = db::open_in_memory().unwrap();
        let id = db::account::insert(&db, "SAV", "Rainy Day", db::account::Kind::Cash, 0).unwrap();
        let found = db::account::by_code(&db, "SAV", db::account::Kind::Cash)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, id);
    }

    #[test]
    fn ids_display_as_the_bare_number() {
        assert_eq!(GoalId(999).to_string(), "999");
        assert_eq!(format!("{}", AccountId(7)), "7");
    }
}
