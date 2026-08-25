//! Needs the `import` feature: the workbook is what these assert against, and
//! the importer is what puts it in a database. Without it the file compiles to
//! nothing rather than failing to build.
#![cfg(feature = "import")]

use chrono::NaiveDate;
use mistermanager::calc::planning;
use mistermanager::db::bill;
use mistermanager::db::setting::{Key, key};
use mistermanager::db::{goal, txn};
use mistermanager::gate::Gate;
use mistermanager::money::Cents;
use mistermanager::plan_line::Line;
use mistermanager::{db, import, plan, projection, transfer};
use std::path::Path;

mod common;

use common::{sheet_cents, workbook};

/// The date the waterfall quotes the checking balance at: the day before the
/// next paycheck, the same figure the workbook's own `Overview!E2` held.
fn adhoc(db: &db::Db, today: NaiveDate) -> NaiveDate {
    projection::dates(db, today).unwrap().adhoc
}

/// The waterfall, run the way both sinks run it: the settings read once and
/// handed to it, quoted at the day before the next paycheck.
fn computed_plan(db: &db::Db, today: NaiveDate) -> planning::Plan {
    plan::compute_from_db(db, &plan::settings_from_db(db).unwrap(), adhoc(db, today)).unwrap()
}

/// Import the whole workbook into `db`, doing the first import's two steps
/// -- accounts, then the roles the Accounts screen would set, then the rest.
///
/// `None` when the run has no `MM_ACCOUNTS`, so a caller skips exactly as it
/// does for a missing workbook. Every caller here already holds its own
/// database, so this fills one rather than handing one back the way
/// `common::imported` does.
fn import_all(db: &db::Db, path: &Path, today: NaiveDate) -> Option<()> {
    let roles = common::roles()?;
    match import::import_all(db, path, today, false).unwrap() {
        import::Report::AccountsOnly { .. } => {}
        import::Report::Full(_) => panic!("an unconfigured database imported Savings"),
    }
    common::configure(db, &roles);
    match import::import_all(db, path, today, false).unwrap() {
        import::Report::Full(_) => {}
        import::Report::AccountsOnly { .. } => panic!("a configured database skipped Savings"),
    }
    Some(())
}

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

/// The waterfall must reproduce the workbook's own three transfer
/// instructions. Those are compared against `Planning!D29`, `D34`, `D38`
/// rather than against literals, because the owner edits the workbook and
/// re-pins the excess.
///
/// The three Planning gates are derived from goal balances, while the sheet
/// hardcodes all three to FALSE. They agree today because all three goals
/// are funded. When one goes underfunded this test will fail, and the app
/// will be the correct one -- reconcile by fixing the sheet, not by pinning
/// the gates back off.
#[test]
fn the_waterfall_reproduces_the_workbooks_transfer_instructions() {
    let Some(path) = workbook() else {
        eprintln!("skipping: Money.xlsx not found");
        return;
    };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    let today = import::sheet(&mut sheets, "Overview")
        .unwrap()
        .get((28, 1))
        .and_then(mistermanager::import::cell::as_date)
        .expect("Overview!B29 holds the workbook's today");

    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);
    let computed = computed_plan(&db, today);

    let planning = import::sheet(&mut sheets, "Planning").unwrap();
    // The first group's sub-lines, each against its own cell. Without these,
    // the group total alone cannot catch an error in any one of them: because
    // `lines.goals` is defined as the plug, the total algebraically reduces
    // to `excess_used` minus the other two groups, so a bug in `lines.bills`,
    // `lines.current_housing`, or `lines.roth` would be silently absorbed
    // and the total would stay unchanged.
    assert_eq!(computed.lines.bills, sheet_cents(&planning, 29, 3), "D30");
    assert_eq!(
        computed.lines.current_housing,
        sheet_cents(&planning, 30, 3),
        "D31"
    );
    assert_eq!(computed.lines.goals, sheet_cents(&planning, 31, 3), "D32");
    assert_eq!(computed.lines.roth, sheet_cents(&planning, 32, 3), "D33");
    assert_eq!(
        computed.lines.bills
            + computed.lines.current_housing
            + computed.lines.goals
            + computed.lines.roth,
        sheet_cents(&planning, 28, 3),
        "D29"
    );
    assert_eq!(
        computed.lines.future_housing + computed.lines.mom_and_dad + computed.lines.emergency_fund,
        sheet_cents(&planning, 33, 3),
        "D34"
    );
    assert_eq!(
        computed.lines.retirement + computed.lines.investment,
        sheet_cents(&planning, 37, 3),
        "D38"
    );
    assert_eq!(computed.lines.total(), computed.excess_used, "the lines");
}

/// Every Planning constant must arrive from the sheet, not from a fallback.
///
/// `compute_from_db` defaults each setting, and several defaults happen to
/// equal the workbook's current values — so comparing the waterfall's totals
/// alone cannot distinguish "imported correctly" from "import silently failed
/// and fell back". These assertions pin each value against its own cell.
#[test]
fn every_planning_constant_comes_from_the_sheet() {
    let Some(path) = workbook() else { return };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);

    let planning = import::sheet(&mut sheets, "Planning").unwrap();
    let imported = |key: Key<Cents>| {
        mistermanager::db::setting::get(&db, key)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} was never imported"))
    };

    for (setting_key, row, col, cell) in [
        (key::PLANNING_TARGET, 0, 3, "D1"),
        (key::PINNED_EXCESS, 2, 3, "D3"),
        (key::PLANNING_BUFFER, 10, 9, "J11"),
        (key::BILL_PAYMENT_CAP, 18, 4, "E19"),
        (key::MOM_AND_DAD_ANNUAL, 19, 4, "E20"),
        (key::GOALS_FLOOR, 23, 4, "E24"),
    ] {
        assert_eq!(
            imported(setting_key),
            sheet_cents(&planning, row, col),
            "{cell}"
        );
    }

    let sheet_pct = |row: usize| {
        planning
            .get((row, 5))
            .and_then(mistermanager::import::cell::as_percent)
            .unwrap_or_else(|| panic!("no percentage at row {row}"))
    };
    for (setting_key, row, cell) in [
        (key::BILL_PAYMENT_PCT, 18, "F19"),
        (key::SPLIT_FUTURE_HOUSING_PCT, 24, "F25"),
        (key::SPLIT_RETIREMENT_PCT, 25, "F26"),
        (key::SPLIT_INVESTMENT_PCT, 26, "F27"),
    ] {
        let got = mistermanager::db::setting::get(&db, setting_key)
            .unwrap()
            .unwrap_or_else(|| panic!("{setting_key} was never imported"));
        assert_eq!(got, sheet_pct(row), "{cell}");
    }
}

/// The importer must be deterministic: run against two independent, freshly
/// created databases, it must land on the same figures both times.
///
/// This is a determinism check, not an idempotency check -- each run gets
/// its own empty database, so it cannot exercise what happens when an
/// import lands on a database that already holds a previous import's data.
/// `importing_twice_into_the_same_connection_is_idempotent` below covers
/// that.
#[test]
fn importing_twice_into_fresh_databases_gives_the_same_plan() {
    let Some(path) = workbook() else { return };
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    if common::roles().is_none() {
        return;
    }

    let run = || {
        let db = db::open_in_memory().unwrap();
        import_all(&db, &path, today).expect("the roles were checked above");
        add_paycheck_rule(&db, &path);
        computed_plan(&db, today)
    };
    assert_eq!(run(), run());
}

/// The real idempotency proof: import the same workbook twice into the
/// *same* connection with `--replace`, and confirm the row counts and the
/// computed plan come out identical to a single import. An import that
/// appended instead of replacing would double the entire ledger on the
/// second run -- every transaction, every goal, every balance -- while
/// exiting 0 and printing healthy output.
#[test]
fn importing_twice_into_the_same_connection_is_idempotent() {
    let Some(path) = workbook() else { return };
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    let db = db::open_in_memory().unwrap();

    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);
    let txns_once = txn::count(&db).unwrap();
    let goals_once = goal::count(&db).unwrap();
    let plan_once = computed_plan(&db, today);

    import::import_all(&db, &path, today, true).unwrap();
    add_paycheck_rule(&db, &path);
    let txns_twice = txn::count(&db).unwrap();
    let goals_twice = goal::count(&db).unwrap();
    let plan_twice = computed_plan(&db, today);

    assert_eq!(txns_once, txns_twice, "transaction count must not double");
    assert_eq!(goals_once, goals_twice, "goal count must not double");
    assert_eq!(plan_once, plan_twice, "the computed plan must not change");
}

/// A second import without `--replace` must fail loudly and leave the
/// database exactly as the first import left it -- not doubled, not
/// partially overwritten.
#[test]
fn a_second_import_without_replace_is_refused_and_changes_nothing() {
    let Some(path) = workbook() else { return };
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    let db = db::open_in_memory().unwrap();

    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);
    let txns_before = txn::count(&db).unwrap();
    let goals_before = goal::count(&db).unwrap();

    let err = import::import_all(&db, &path, today, false).unwrap_err();
    assert!(err.to_string().contains("--replace"), "{err}");

    let txns_after = txn::count(&db).unwrap();
    let goals_after = goal::count(&db).unwrap();
    assert_eq!(
        txns_before, txns_after,
        "the refused import changed txn rows"
    );
    assert_eq!(
        goals_before, goals_after,
        "the refused import changed goal rows"
    );
}

/// Recurring Transactions are typed on the Recurring Transactions screen and
/// the import writes none, so a `--replace` must leave them alone -- with
/// `account` no longer an imported table, the rows they reference outlive the
/// replace and the rules can too. Losing the paycheck one would revert
/// Paycheck-Eve to today and move the Planning waterfall's excess.
#[test]
fn replace_keeps_every_recurring_transaction() {
    let Some(path) = workbook() else { return };
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    let db = db::open_in_memory().unwrap();

    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);
    let before = db::recurring_txn::list(&db).unwrap();
    assert_eq!(before.len(), 1);

    import::import_all(&db, &path, today, true).unwrap();

    let after = db::recurring_txn::list(&db).unwrap();
    assert_eq!(after, before, "a --replace changed the recurring rules");
}

/// The bill block, with its labels, against `Planning!C7:D12`.
///
/// The two categories reach two different waterfall lines and only housing
/// reaches `lines.current_housing`, so the split is load-bearing rather than
/// presentation. Compared against the sheet's own cells: the owner edits the
/// block, and literals would rot.
#[test]
fn the_bill_block_arrives_with_its_labels_in_sheet_order() {
    let Some(path) = workbook() else { return };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);

    let planning = import::sheet(&mut sheets, "Planning").unwrap();
    let label = |row: usize| {
        planning
            .get((row, 2))
            .and_then(mistermanager::import::cell::as_text)
            .unwrap_or_else(|| panic!("no label at Planning row {}", row + 1))
    };

    for (category, rows) in [
        (bill::Category::Housing, 6..=7),
        (bill::Category::Other, 8..=11),
    ] {
        let imported = bill::list(&db, category).unwrap();
        let expected: Vec<usize> = rows.collect();
        assert_eq!(
            imported.len(),
            expected.len(),
            "{category:?} bills: {imported:?}"
        );
        for (bill, row) in imported.iter().zip(expected) {
            assert_eq!(bill.label, label(row), "C{}", row + 1);
            assert_eq!(bill.cents, sheet_cents(&planning, row, 3), "D{}", row + 1);
            assert_eq!(bill.category, category);
        }
    }
}

/// Every destination key the app reads must resolve to a goal the sheet
/// actually names. A substring that has stopped matching produces no error at
/// import -- just a key that reads as "not configured", which for a
/// destination means the money leaves the tracked system. No in-memory test
/// can catch that, because none of them knows what the sheet says.
#[test]
fn every_planning_destination_key_resolves_against_the_workbook() {
    let Some(path) = workbook() else { return };
    let db = db::open_in_memory().unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    if import_all(&db, &path, today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);

    for gate in Gate::ALL {
        let id = mistermanager::db::setting::get(&db, gate.key())
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "{} is unset: {:?} matched no goal",
                    gate.key(),
                    gate.substring()
                )
            });
        assert!(
            goal::get(&db, id).unwrap().is_some(),
            "{} = {id} names no goal",
            gate.key()
        );
    }
    for line in Line::ALL {
        let Some((key, substring)) = line.owned_goal() else {
            continue;
        };
        let id = mistermanager::db::setting::get(&db, key)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} is unset: {substring:?} matched no goal"));
        assert!(
            goal::get(&db, id).unwrap().is_some(),
            "{key} = {id} names no goal"
        );
    }
}

/// The transfers are the plan, grouped: they must sum to exactly what the
/// waterfall said there was to allocate.
#[test]
fn the_transfer_rows_sum_to_the_excess_used() {
    let Some(path) = workbook() else { return };
    let db = db::open_in_memory().unwrap();
    let import_today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    if import_all(&db, &path, import_today).is_none() {
        return;
    }
    add_paycheck_rule(&db, &path);

    let today = mistermanager::db::setting::get(&db, key::WORKBOOK_TODAY)
        .unwrap()
        .expect("the workbook carries its own today");
    let computed = computed_plan(&db, today);
    let rows = transfer::plan(&db, &computed.lines).unwrap();
    let total: Cents = rows.iter().map(|r| r.cents()).sum();
    assert_eq!(total, computed.excess_used);
    assert_eq!(computed.lines.total(), computed.excess_used);
}
