mod common;

use mistermanager::db::setting::key;
use mistermanager::db::{self, fund as db_fund, setting};
use mistermanager::rate::BasisPoints;
use mistermanager::{fund, import};

/// A cell of the `Planning` sheet as a fraction in basis points.
fn sheet_bp(range: &import::SheetRange, row: usize, col: usize) -> BasisPoints {
    range
        .get((row, col))
        .and_then(import::cell::as_rate_bp)
        .unwrap_or(BasisPoints::ZERO)
}

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

/// The whole point of the screen: `J`, `K` and `L` recomputed from what was
/// stored must land on the sheet's own cached columns.
///
/// Computed at the workbook's `Today` rather than the machine's, because the
/// sheet's `J2` is `DATEDIF(..., Dates[Today], "y")` and the two dates can
/// straddle a birthday.
#[test]
fn the_derived_columns_match_the_sheets_own_j_k_and_l() {
    let Some((db, range)) = imported() else {
        return;
    };
    let quoted_at = setting::get(&db, key::WORKBOOK_TODAY)
        .unwrap()
        .expect("the workbook carries Constants!J2");
    let allocation = fund::compute_from_db(&db, quoted_at).unwrap();

    for (i, row) in allocation.rows.iter().enumerate() {
        let sheet_row = i + 1;
        let target = row.target.expect("the workbook carries a birth date");
        assert_eq!(
            target,
            sheet_bp(&range, sheet_row, 9),
            "J of {:?}",
            row.name
        );
        // `K` and `L` are exact in the sheet and truncated here, so they may
        // sit one basis point apart and no further.
        let actual = sheet_bp(&range, sheet_row, 10);
        assert!(
            (row.actual_share.0 - actual.0).abs() <= 1,
            "K of {:?}: {:?} against the sheet's {actual:?}",
            row.name,
            row.actual_share
        );
        let delta = sheet_bp(&range, sheet_row, 11);
        let computed = row.delta.expect("a row with a target has a delta");
        assert!(
            (computed.0 - delta.0).abs() <= 1,
            "L of {:?}: {computed:?} against the sheet's {delta:?}",
            row.name
        );
    }
}

/// `M5` is the sheet's own total, one row past the block.
#[test]
fn the_total_is_the_sheets_own_sum() {
    let Some((db, range)) = imported() else {
        return;
    };
    let quoted_at = setting::get(&db, key::WORKBOOK_TODAY).unwrap().unwrap();
    let allocation = fund::compute_from_db(&db, quoted_at).unwrap();

    let sheet_total = sheet_cents(&range, allocation.rows.len() + 1, 12);
    assert_eq!(allocation.total, sheet_total);
}
