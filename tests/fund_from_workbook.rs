//! Needs the `import` feature: the workbook is what these assert against, and
//! the importer is what puts it in a database. Without it the file compiles to
//! nothing rather than failing to build.
#![cfg(feature = "import")]

mod common;

use mistermanager::db::{self, fund as db_fund};
use mistermanager::import;

fn sheet_text(range: &import::SheetRange, row: usize, col: usize) -> Option<String> {
    range.get((row, col)).and_then(import::cell::as_text)
}

use common::sheet_cents;

/// An imported database and the `Planning` sheet it came from.
fn imported() -> Option<(db::Db, import::SheetRange)> {
    let today = chrono::Local::now().date_naive();
    let (db, mut sheets) = common::imported(today)?;
    let range = import::sheet(&mut sheets, "Planning").unwrap();
    Some((db, range))
}

/// The names and values are transcribed, not computed, so they must match the
/// sheet exactly.
#[test]
fn every_imported_fund_is_the_sheets_own_name_and_value() {
    let Some((db, range)) = imported() else {
        return;
    };
    let stored = db_fund::list(&db).unwrap();
    assert!(!stored.is_empty(), "the workbook carries a fund block");

    for (i, fund) in stored.iter().enumerate() {
        let row = i + 1; // `I2` is row index 1
        assert_eq!(Some(fund.name.clone()), sheet_text(&range, row, 8));
        assert_eq!(fund.actual, sheet_cents(&range, row, 12));
        assert_eq!(fund.ord, i as i64);
    }
}
