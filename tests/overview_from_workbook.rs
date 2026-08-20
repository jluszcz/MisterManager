use chrono::NaiveDate;
use mistermanager::money::Cents;
use mistermanager::tui::overview::Overview;
use mistermanager::{db, import, projection};
use std::path::Path;

mod common;

use common::{sheet_cents, workbook, workbook_today};

/// The workbook's paycheck, as a recurring transaction -- anchored the day after
/// `Overview!E2` (Paycheck-Eve), read from the sheet like every other golden
/// value here rather than transcribed, since the owner keeps editing the
/// workbook and a frozen anchor would go stale the next time payday moves.
///
/// The amount is the one figure here that is not read from a cell, and the
/// horizon is left open. Nothing in these tests regenerates rows: the recurring transaction
/// exists so `recurring_txn::next_paycheck` has something to read, and that uses
/// only the anchor and the cadence.
///
/// Returns the `Overview!E2` date itself, so a caller that wants to pin
/// `projection::dates`'s derivation against it does not have to read the
/// sheet a second time.
fn add_paycheck_rule(db: &db::Db, path: &Path) -> NaiveDate {
    let mut sheets = import::open(path).unwrap();
    let eve = import::sheet(&mut sheets, "Overview")
        .unwrap()
        .get((1, 4))
        .and_then(mistermanager::import::cell::as_date)
        .expect("Overview!E2 is the ad-hoc projection date");
    let anchor = eve
        .succ_opt()
        .expect("Overview!E2 is not the last representable date");

    let checking = db::account::checking(db).expect("MM_ACCOUNTS names the current account");
    let id = db::recurring_txn::insert(
        db,
        &db::recurring_txn::NewRecurringTxn {
            description: "Salary".to_string(),
            cents: Cents(500_000),
            account_id: checking.id,
            cadence: db::recurring_txn::Cadence::Biweekly,
            anchor_date: anchor,
            horizon: None,
        },
    )
    .unwrap();
    db::recurring_txn::set_paycheck(db, id).unwrap();
    eve
}

/// The screen's own arithmetic, not just the queries beneath it: the three
/// Net figures the design's §7 golden values name, at 2026-08-12,
/// 2026-08-27, and 2026-09-01.
///
/// Compared against the workbook's **own cached cells** — `Overview!C29`,
/// `C30`, and `G14` — rather than against those literals. The owner keeps
/// editing the workbook, so literals would rot; those cells hold exactly the
/// design's figures today, and go on being right after the next edit.
///
/// This exercises the negation and the two subtotals as well as the queries,
/// which `tests/import_ledger.rs` cannot: it compares one balance at a time.
#[test]
fn the_overview_screens_net_agrees_with_the_workbook() {
    let Some(path) = workbook() else { return };
    let today = workbook_today(&mut import::open(&path).unwrap());
    let Some((db, mut sheets)) = common::imported(today) else {
        return;
    };
    let paycheck_eve = add_paycheck_rule(&db, &path);

    // `projection::dates` derives the ad-hoc date from the paycheck recurring transaction, so
    // it has to exist before any of the three columns lines up with what the
    // sheet quotes.
    let dates = projection::dates(&db, today).unwrap();
    assert_eq!(
        dates.adhoc, paycheck_eve,
        "Paycheck-Eve, derived from the paycheck recurring_txn"
    );
    let overview = Overview::load(&db, dates).unwrap();

    let sheet = import::sheet(&mut sheets, "Overview").unwrap();
    assert_eq!(
        overview.net.to_date,
        sheet_cents(&sheet, 28, 2),
        "C29, net to-date"
    );
    assert_eq!(
        overview.net.month_end,
        sheet_cents(&sheet, 29, 2),
        "C30, net at month-end"
    );
    assert_eq!(
        overview.net.adhoc,
        sheet_cents(&sheet, 13, 6),
        "G14, net at the ad-hoc date"
    );
}
