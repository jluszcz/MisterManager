//! Needs the `import` feature: the workbook is what these assert against, and
//! the importer is what puts it in a database. Without it the file compiles to
//! nothing rather than failing to build.
#![cfg(feature = "import")]

use chrono::NaiveDate;
use mistermanager::money::Cents;
use mistermanager::overview::Overview;
use mistermanager::{db, import, projection};
use std::path::Path;

mod common;

use common::{eve_is_comparable, sheet_cents, workbook, workbook_today};

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
///
/// `C29` and `C30` are read at a today-only `Dates`, since neither cell is
/// quoted at a date the paycheck eve moves; only `G14` is, so only `G14` is
/// gated on the eve being a day the app quotes at all.
#[test]
fn the_overview_screens_net_agrees_with_the_workbook() {
    let Some(path) = workbook() else { return };
    let today = workbook_today(&mut import::open(&path).unwrap());
    let Some((db, mut sheets)) = common::imported(today) else {
        return;
    };
    let paycheck_eve = add_paycheck_rule(&db, &path);

    let sheet = import::sheet(&mut sheets, "Overview").unwrap();

    // `C29` and `C30` are quoted at today and at `EOMONTH(today, 0) + 1`, and
    // the eve decides neither -- so both are read off a `Dates` built from
    // today alone, where Month-End is that same cell's date whatever the
    // paycheck rule says. Read off the derived dates instead, the month-end
    // comparison would drop out on every run whose eve has crossed into the
    // next month, which is the workbook's only oracle for the Net aggregation
    // going quiet over a column it does not depend on.
    let today_only = Overview::load(&db, projection::Dates::new(today, today)).unwrap();
    assert_eq!(
        today_only.net.to_date,
        sheet_cents(&sheet, 28, 2),
        "C29, net to-date"
    );
    assert_eq!(
        today_only.net.month_end,
        sheet_cents(&sheet, 29, 2),
        "C30, net at month-end"
    );

    // The ad-hoc column is the one the paycheck recurring transaction decides,
    // and `G14` the one cell that can disagree with it by design.
    if eve_is_comparable(today, paycheck_eve) {
        let dates = projection::dates(&db, today).unwrap();
        assert_eq!(
            dates.adhoc, paycheck_eve,
            "Paycheck-Eve, derived from the paycheck recurring_txn"
        );
        assert_eq!(
            Overview::load(&db, dates).unwrap().net.adhoc,
            sheet_cents(&sheet, 13, 6),
            "G14, net at the ad-hoc date"
        );
    }
}
