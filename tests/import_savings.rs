mod common;

use common::{container, imported, sheet_cents, workbook, workbook_today};
use mistermanager::db::goal;
use mistermanager::db::recurring_goal::{self, Cadence};
use mistermanager::db::setting::{self, key};
use mistermanager::gate::Gate;
use mistermanager::money::Cents;
use mistermanager::plan_line::Line;
use mistermanager::savings_block::Block;
use mistermanager::{db, import};

/// Find the sheet row holding `name` in the given column, since the goal
/// blocks are not contiguous -- the goal block has a gap in it, where the
/// bucket block has a bucket but column A is empty.
fn row_of(range: &import::SheetRange, column: usize, name: &str) -> usize {
    nth_row_of(range, column, name, 0)
}

/// Like `row_of`, but returns the `occurrence`-th (0-indexed) row holding
/// `name`. A few goal names repeat in the live workbook -- one three times --
/// so matching by name alone is ambiguous. A per-name occurrence counter kept
/// alongside an iteration in `in_sheet_order` recovers the right row: the Nth
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

/// Imported goals in the sheet's own row order, which is what an occurrence
/// counter has to walk to line up with `nth_row_of`.
///
/// Deliberately not the order `goal::list_with_balances` returns: that is the
/// order the screens show, undated goals by hand and dated ones by deadline,
/// and a deadline has nothing to do with the row a goal was imported from.
/// `sort` is assigned per block as the import walks down the sheet and `id`
/// increases with it, so the pair recovers that walk.
fn in_sheet_order(mut goals: Vec<goal::GoalWithBalance>) -> Vec<goal::GoalWithBalance> {
    goals.sort_by_key(|g| (g.goal.sort, g.goal.id));
    goals
}

/// A fully imported database and the open workbook, at the workbook's own
/// today.
fn loaded() -> Option<(db::Db, import::Sheets)> {
    let path = workbook()?;
    let today = workbook_today(&mut import::open(&path).unwrap());
    imported(today)
}

/// `Savings!J1` equals the sum of its bucket rows exactly, so the bucket
/// container must reconcile to zero. That is a structural property of the
/// sheet, not a snapshot, so it is asserted as a literal zero.
#[test]
fn the_bucket_container_reconciles_exactly() {
    let Some((db, _sheets)) = loaded() else {
        eprintln!("skipping: Money.xlsx not found");
        return;
    };
    let buckets = container(&db, Block::Buckets);
    assert_eq!(goal::container_excess(&db, buckets).unwrap(), Cents::ZERO);
    assert!(
        !goal::list_with_balances(&db, buckets).unwrap().is_empty(),
        "no buckets imported"
    );
}

/// Each bucket's balance must match its own cell in column J.
#[test]
fn bucket_balances_match_the_sheet() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();
    let buckets = container(&db, Block::Buckets);
    let goals = goal::list_with_balances(&db, buckets).unwrap();

    // Every contiguous named row in column I must have become a bucket, and
    // no more -- derived from the sheet rather than pinned to a count, so a
    // fourth bucket would pass and a phantom one would fail.
    let named_in_column_i = (5..sheet.height())
        .take_while(|r| sheet.get((*r, 8)).and_then(import::cell::as_text).is_some())
        .count();
    assert_eq!(goals.len(), named_in_column_i);

    // Column I holds the bucket names, J their current values.
    for g in &goals {
        let row = row_of(&sheet, 8, &g.goal.name);
        assert_eq!(
            g.current,
            sheet_cents(&sheet, row, 9),
            "{} current",
            g.goal.name
        );
    }

    // Down Payment is the one bucket excluded from interest allocation.
    let down_payment = goals
        .iter()
        .find(|g| g.goal.name.contains("Down Payment"))
        .expect("a Home Down Payment bucket");
    assert!(!down_payment.goal.interest_eligible);
    assert!(goals.iter().filter(|g| g.goal.interest_eligible).count() >= 1);
}

/// The goal container's unallocated remainder must match `Savings!B3`,
/// whatever it is today.
#[test]
fn the_goal_containers_excess_matches_the_sheet() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();
    let goals = container(&db, Block::Goals);
    assert_eq!(
        goal::container_excess(&db, goals).unwrap(),
        sheet_cents(&sheet, 2, 1),
        "the goal container's excess must match Savings!B3"
    );
    assert!(goal::list_with_balances(&db, goals).unwrap().len() > 40);
}

/// Rows 6-26 have no `Goal Date`; rows 27+ do. Both must import, and every
/// goal's current, target, and date must match its own row.
#[test]
fn every_goal_matches_its_row() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();
    let goals =
        in_sheet_order(goal::list_with_balances(&db, container(&db, Block::Goals)).unwrap());

    // Match by name: the block is not contiguous, so an index offset would
    // silently compare later goals against blank cells. A few names repeat,
    // so track how many times each has been seen and take the matching
    // occurrence -- which only lines up because `in_sheet_order` put the
    // goals back in the order the rows are read in.
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for g in &goals {
        let occurrence = seen.entry(g.goal.name.as_str()).or_insert(0);
        let row = nth_row_of(&sheet, 0, &g.goal.name, *occurrence);
        *occurrence += 1;
        assert_eq!(
            g.current,
            sheet_cents(&sheet, row, 1),
            "{} current",
            g.goal.name
        );
        assert_eq!(
            g.goal.goal_cents,
            sheet_cents(&sheet, row, 2),
            "{} target",
            g.goal.name
        );
        let sheet_date = sheet.get((row, 4)).and_then(import::cell::as_date);
        assert_eq!(g.goal.goal_date, sheet_date, "{} goal date", g.goal.name);
    }

    assert!(
        goals.iter().any(|g| g.goal.goal_date.is_none()),
        "no undated goals"
    );
    assert!(
        goals.iter().any(|g| g.goal.goal_date.is_some()),
        "no dated goals"
    );
}

/// The strongest check available: `calc::per_paycheck` must reproduce column F,
/// which Excel computed with its own `PerPaycheck` lambda. Two independent
/// implementations of the same formula over the same data.
#[test]
fn per_paycheck_reproduces_the_sheets_column_f() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let today = workbook_today(&mut sheets);
    let period_days = setting::get(&db, key::PAY_PERIOD_DAYS).unwrap().unwrap();
    let sheet = import::sheet(&mut sheets, "Savings").unwrap();
    let goals =
        in_sheet_order(goal::list_with_balances(&db, container(&db, Block::Goals)).unwrap());

    // A few names repeat; see the comment in `every_goal_matches_its_row`
    // for why an occurrence counter is needed to disambiguate them.
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut compared = 0;
    for g in &goals {
        let occurrence = seen.entry(g.goal.name.as_str()).or_insert(0);
        let row = nth_row_of(&sheet, 0, &g.goal.name, *occurrence);
        *occurrence += 1;
        // Column F is blank for undated and already-met goals.
        let Some(expected) = sheet.get((row, 5)).and_then(import::cell::as_cents) else {
            continue;
        };
        let computed = mistermanager::calc::per_paycheck(
            g.current,
            g.goal.goal_cents,
            g.goal.goal_date,
            today,
            period_days,
        )
        .unwrap();
        assert_eq!(
            computed,
            Some(expected),
            "{} per-paycheck (sheet row {})",
            g.goal.name,
            row + 1
        );
        compared += 1;
    }
    assert!(
        compared > 20,
        "only compared {compared} goals; expected many more"
    );
}

#[test]
fn catalog_splits_annual_from_biennial() {
    let Some((db, _sheets)) = loaded() else {
        return;
    };

    let entries = recurring_goal::list(&db).unwrap();
    let count = |want: Cadence| entries.iter().filter(|e| e.cadence == want).count();
    let annual = count(Cadence::Annual);
    let biennial = count(Cadence::Biennial);

    assert!(annual > 40, "expected many annual entries, got {annual}");
    assert!(biennial >= 1, "expected at least one biennial entry");

    // Every entry must carry a usable month and a positive amount.
    let bad: Vec<&str> = entries
        .iter()
        .filter(|e| !(1..=12).contains(&e.month) || e.base_cents <= Cents::ZERO)
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        bad.is_empty(),
        "recurring_goal entries with an invalid month or amount: {bad:?}"
    );
}

/// Each Planning gate, and each `Line` matched by name at import, must come
/// out of an import pointing at a real goal.
///
/// Without this, a workbook rename would leave the settings unwritten, every
/// gate or destination would quietly resolve to zero -- or leave as a
/// withdrawal -- and `plan::compute_from_db` would go on emitting transfer
/// instructions from the wrong branch, with nothing in the output to show
/// that a key had gone missing.
#[test]
fn the_planning_gates_record_their_goal_ids() {
    let Some((db, _sheets)) = loaded() else {
        eprintln!("skipping: Money.xlsx not found");
        return;
    };

    let goals = container(&db, Block::Goals);
    let buckets = container(&db, Block::Buckets);

    // Every gate, from `Gate::ALL` rather than a list written out here, so a
    // third gate cannot be added and left untested.
    let expected_container = |gate: Gate| match gate {
        Gate::Roth => goals,
        Gate::EmergencyFund => buckets,
    };

    for gate in Gate::ALL {
        let key = gate.key();
        let id = setting::get(&db, key)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} was never recorded"));
        let found = goal::get(&db, id)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} = {id} points at no goal"));
        assert!(
            found.name.contains(gate.substring()),
            "{key} = {id} points at {:?}, which does not contain {:?}",
            found.name,
            gate.substring()
        );
        // `import::savings` offers each gate goals from one block only --
        // Roth from the goal block, the other from the bucket block. Pin that
        // here, not just the name substring, so a future goal that happens to
        // share a name but lives in the wrong container is caught.
        assert_eq!(
            found.container_account_id,
            expected_container(gate),
            "{key} = {id} ({:?}) is in the wrong container",
            found.name
        );
    }

    // Every `Line` matched by name at import, paired with the container its
    // goal lives in.
    let matched_lines = [
        (Line::FutureHousing, buckets),
        (Line::Bills, goals),
        (Line::CurrentHousing, goals),
        (Line::MomAndDad, buckets),
    ];

    for (line, expected_container) in matched_lines {
        let (key, substring) = line
            .owned_goal()
            .expect("every listed line is matched by name at import");
        let id = setting::get(&db, key)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} was never recorded"));
        let found = goal::get(&db, id)
            .unwrap()
            .unwrap_or_else(|| panic!("{key} = {id} points at no goal"));
        assert!(
            found.name.contains(substring),
            "{key} = {id} points at {:?}, which does not contain {:?}",
            found.name,
            substring
        );
        assert_eq!(
            found.container_account_id, expected_container,
            "{key} = {id} ({:?}) is in the wrong container",
            found.name
        );
    }
}

/// The whole point of the two-pass import, end to end against the real
/// workbook: with the containers unconfigured there is nothing to read
/// `Savings` into, so the run writes the accounts and stops.
#[test]
fn an_import_with_unresolved_containers_creates_accounts_and_stops() {
    let Some(path) = workbook() else { return };
    let today = workbook_today(&mut import::open(&path).unwrap());
    let db = db::open_in_memory().unwrap();

    let report = import::import_all(&db, &path, today, false).unwrap();

    match report {
        import::Report::AccountsOnly { accounts } => assert!(accounts > 0),
        import::Report::Full(_) => panic!("an unconfigured database imported Savings"),
    }
    assert!(goal::count(&db).unwrap() == 0, "goals were imported");
    for block in Block::ALL {
        assert!(
            setting::get(&db, block.key()).unwrap().is_none(),
            "{} was written by an import",
            block.key()
        );
    }
}

/// And the second pass needs no `--replace`: the first wrote neither a
/// transaction nor a goal, so `has_imported_data` is still false and the
/// import has nothing to refuse.
#[test]
fn a_second_import_after_configuring_containers_needs_no_replace_flag() {
    let Some(path) = workbook() else { return };
    let Some(roles) = common::roles() else { return };
    let today = workbook_today(&mut import::open(&path).unwrap());
    let db = db::open_in_memory().unwrap();

    import::import_all(&db, &path, today, false).unwrap();
    common::configure(&db, &roles);

    match import::import_all(&db, &path, today, false).unwrap() {
        import::Report::Full(report) => {
            assert!(report.savings.goals > 0, "no goals imported");
            assert!(report.savings.buckets > 0, "no buckets imported");
        }
        import::Report::AccountsOnly { .. } => panic!("a configured database skipped Savings"),
    }
}

/// A database that already knows its containers imports in one pass, which is
/// what makes the two-step a first-run cost rather than a standing one --
/// `--replace` included, since `savings::set_containers` puts the mapping
/// back after the clear takes it.
#[test]
fn an_import_with_resolved_containers_completes_in_one_pass() {
    let Some((db, mut sheets)) = loaded() else {
        return;
    };
    let path = workbook().expect("loaded already found it");
    let today = workbook_today(&mut sheets);

    match import::import_all(&db, &path, today, true).unwrap() {
        import::Report::Full(report) => assert!(report.savings.goals > 0),
        import::Report::AccountsOnly { .. } => {
            panic!("a --replace lost the container mapping and reopened the two-step")
        }
    }
    for block in Block::ALL {
        assert!(
            setting::get(&db, block.key()).unwrap().is_some(),
            "{} did not survive a --replace",
            block.key()
        );
    }
}
