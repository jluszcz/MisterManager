mod common;

use chrono::NaiveDate;
use common::{container, imported, sheet_cents, workbook_today};
use mistermanager::db::setting::{self, key};
use mistermanager::db::{self, goal};
use mistermanager::goal as goal_engine;
use mistermanager::import;
use mistermanager::money::Cents;
use mistermanager::reading::Reading;
use mistermanager::savings::{self, Row};
use mistermanager::savings_block::Block;
use mistermanager::tui::savings::Savings;
use std::collections::HashMap;

/// The `occurrence`-th (0-indexed) row holding `name` in `column`.
///
/// A few goal names repeat in the live workbook -- one three times -- so
/// matching by name alone is ambiguous. A per-name occurrence counter kept
/// alongside an iteration in `sheet_order` recovers the right row: the Nth
/// goal named X is then the Nth row named X.
fn nth_row_of(range: &import::SheetRange, column: usize, name: &str, occurrence: usize) -> usize {
    (0..range.height())
        .filter(|r| {
            range
                .get((*r, column))
                .and_then(import::cell::as_text)
                .as_deref()
                == Some(name)
        })
        .nth(occurrence)
        .unwrap_or_else(|| panic!("no occurrence #{occurrence} in column {column} named {name:?}"))
}

/// Each of a container's goals by id, at its position in the sheet's own row
/// order -- the order an occurrence counter has to walk to line up with
/// `nth_row_of`.
///
/// The screen's order cannot be walked directly: it shows undated goals by
/// hand and dated ones by deadline, and a deadline has nothing to do with the
/// row a goal was imported from. `sort` is assigned per block as the import
/// walks down the sheet and `id` increases with it, so the pair recovers that
/// walk.
fn sheet_order(db: &db::Db, container_id: db::AccountId) -> HashMap<db::GoalId, usize> {
    let mut goals = goal::list_with_balances(db, container_id).unwrap();
    goals.sort_by_key(|g| (g.goal.sort, g.goal.id));
    goals
        .iter()
        .enumerate()
        .map(|(position, g)| (g.goal.id, position))
        .collect()
}

/// A fully imported database and the open workbook, at the workbook's own
/// today.
fn loaded() -> Option<(db::Db, import::Sheets)> {
    let path = common::workbook()?;
    let today = workbook_today(&mut import::open(&path).unwrap());
    imported(today)
}

/// Build the Savings screen exactly as `App` does, from a freshly imported
/// database.
fn screen(db: &db::Db, today: NaiveDate) -> Savings {
    let period_days = setting::get_or(db, key::PAY_PERIOD_DAYS, 14).unwrap();
    let mut savings = Savings::new(db::account::list(db).unwrap(), today, period_days);
    savings
        .set_goals(goal_engine::all_with_balances(db, Reading::Strict).unwrap())
        .unwrap();
    let excess = savings::containers_with_excess(db).unwrap();
    let containers = excess.iter().map(|(id, _)| *id).collect();
    savings.set_containers(containers);
    savings.set_excess(excess);
    savings
}

/// Every goal-block goal's `Current`, `%` and `$/Pay` against its own row of
/// the sheet — columns B, C, D and F — computed by two independent
/// implementations of the same formulas over the same data.
///
/// Compared against the workbook's own cached cells rather than literals: the
/// owner keeps editing it, so literals would rot.
#[test]
fn the_savings_screens_goal_rows_agree_with_the_workbook() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let today = workbook_today(&mut sheets);
    let screen = screen(&db, today);
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();

    let goals = container(&db, Block::Goals);
    let mut rows: Vec<&Row> = screen
        .rows()
        .iter()
        .filter(|r| r.container.id() == goals)
        .copied()
        .collect();
    assert!(rows.len() > 40, "only {} goals on screen", rows.len());

    // A few names repeat, so the Nth row named X has to be paired with the Nth
    // sheet row named X by counting occurrences -- which needs the rows walked
    // in the sheet's order rather than the screen's.
    let order = sheet_order(&db, goals);
    rows.sort_by_key(|r| order[&r.goal_id]);
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut compared_pay = 0;
    for row in &rows {
        let occurrence = seen.entry(row.name.as_str()).or_insert(0);
        let sheet_row = nth_row_of(&sheet, 0, &row.name, *occurrence);
        *occurrence += 1;

        assert_eq!(
            row.current,
            sheet_cents(&sheet, sheet_row, 1),
            "{} current (sheet row {})",
            row.name,
            sheet_row + 1
        );
        assert_eq!(
            row.goal,
            sheet_cents(&sheet, sheet_row, 2),
            "{} target",
            row.name
        );
        // Column D is `% Complete`, a fraction cell. Blank or errored where
        // the target is zero, which is exactly where the screen shows `—`.
        // Both sides are `Percent`, so the sheet's own scaling and the
        // screen's cannot silently differ by the factor of 100 that separates
        // `Percent` from `BasisPoints`.
        let sheet_percent = sheet.get((sheet_row, 3)).and_then(import::cell::as_percent);
        assert_eq!(row.percent, sheet_percent, "{} percent complete", row.name);
        // Column F goes blank for undated and already-met goals, and so does
        // `$/Pay`.
        let sheet_pay = sheet.get((sheet_row, 5)).and_then(import::cell::as_cents);
        assert_eq!(row.per_paycheck, sheet_pay, "{} $/paycheck", row.name);
        if sheet_pay.is_some() {
            compared_pay += 1;
        }
    }
    assert!(
        compared_pay > 20,
        "only {compared_pay} goals had a per-paycheck figure to compare"
    );
}

/// The buckets' `current` and `goal`, against columns J and K, and both
/// derived blanks: `goal_date` and `per_paycheck` are `None`, the buckets
/// being undated. The `%` column is not compared -- see the comment below.
#[test]
fn the_savings_screens_bucket_rows_agree_with_the_workbook() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let today = workbook_today(&mut sheets);
    let screen = screen(&db, today);
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();

    let buckets = container(&db, Block::Buckets);
    let rows: Vec<&Row> = screen
        .rows()
        .iter()
        .filter(|r| r.container.id() == buckets)
        .copied()
        .collect();
    assert!(!rows.is_empty(), "no buckets on screen");

    for row in &rows {
        let sheet_row = nth_row_of(&sheet, 8, &row.name, 0);
        assert_eq!(
            row.current,
            sheet_cents(&sheet, sheet_row, 9),
            "{} current",
            row.name
        );
        assert_eq!(
            row.goal,
            sheet_cents(&sheet, sheet_row, 10),
            "{} target",
            row.name
        );
        // The buckets are undated, so column F's equivalent is blank for all
        // of them -- which is the case `$/Pay` must not invent a figure for.
        assert_eq!(row.goal_date, None, "{} goal date", row.name);
        assert_eq!(row.per_paycheck, None, "{} $/paycheck", row.name);
        // Column L is not the screen's `%`: it divides by an unlabeled
        // column M rather than by K, so it reads as a different figure
        // entirely. J and K are asserted above, and in the goal block's
        // equivalent columns by the test above; L is not a source of truth.
    }
}

/// The reconciliation line under the list, against `Savings!B3` and the
/// bucket block's structural zero. This screen is the only one that shows the
/// figures, so this is the only place they are pinned against the sheet.
#[test]
fn the_savings_screens_reconciliation_agrees_with_the_savings_sheet() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let today = workbook_today(&mut sheets);
    let screen = screen(&db, today);
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();

    let goals = container(&db, Block::Goals);
    let buckets = container(&db, Block::Buckets);
    let excess = |id| {
        screen
            .excess()
            .iter()
            .find(|(account, _)| *account == id)
            .expect("both containers must be on the reconciliation line")
            .1
    };

    assert_eq!(excess(goals), sheet_cents(&sheet, 2, 1), "Savings!B3");
    assert_eq!(excess(buckets), Cents::ZERO);
    assert_eq!(screen.excess().len(), 2);
    // The goal container's few cents of drift are the workbook's steady state,
    // and the footer truncates them away to nothing rather than reporting
    // them.
    assert_eq!(savings::unallocated(excess(goals)), Cents::ZERO);
}
