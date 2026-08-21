use mistermanager::db::account::{self, Group, Kind};
use mistermanager::db::setting::{self, key};
use mistermanager::money::Cents;
use mistermanager::{db, import};

mod common;

use common::workbook;

/// Every code in one of the `Constants` account columns, in sheet order.
///
/// Read out of the sheet rather than written down: the codes name real
/// accounts at real institutions, and this repository holds neither.
fn codes(range: &import::SheetRange, column: usize) -> Vec<String> {
    (1..range.height())
        .filter_map(|row| import::cell::as_text(&import::cell::at(range, row, column)))
        .collect()
}

/// The import turns each code into an account and stops there: the name is
/// the code, the band is the kind's default, and the order is the sheet's.
/// Everything else about how an account is shown is the owner's, typed on the
/// Accounts screen and outliving a `--replace`.
#[test]
fn every_code_becomes_an_account_named_by_itself_in_sheet_order() {
    let Some(path) = workbook() else {
        eprintln!("skipping: Money.xlsx not found");
        return;
    };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    let constants = import::sheet(&mut sheets, "Constants").unwrap();
    import::constants::import(&db, &mut sheets).unwrap();

    for (column, kind, default_band) in [
        (0usize, Kind::Cash, Group::Savings),
        (2usize, Kind::Credit, Group::Credit),
    ] {
        let expected = codes(&constants, column);
        assert!(!expected.is_empty(), "the sheet lists no {kind:?} accounts");
        let imported = account::list_by_kind(&db, kind).unwrap();

        assert_eq!(
            imported
                .iter()
                .map(|a| a.code.as_str().to_string())
                .collect::<Vec<_>>(),
            expected,
            "{kind:?} accounts are not in sheet order"
        );
        for account in &imported {
            assert_eq!(
                account.name.as_str(),
                account.code.as_str(),
                "an account arrived under a name the sheet does not carry"
            );
            assert_eq!(
                account.group,
                default_band,
                "{} was placed",
                account.code.as_str()
            );
        }
        assert_eq!(
            imported.iter().map(|a| a.sort).collect::<Vec<_>>(),
            (0..imported.len() as i64).collect::<Vec<_>>(),
            "{kind:?} sorts are not 0..n-1"
        );
    }
}

/// The settings the sheet carries, against the sheet's own cells. Literals
/// here would be the owner's figures written into the repository, and would
/// rot besides.
#[test]
fn the_constants_settings_are_the_sheets_own_cells() {
    let Some(path) = workbook() else {
        return;
    };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    let constants = import::sheet(&mut sheets, "Constants").unwrap();
    import::constants::import(&db, &mut sheets).unwrap();

    let at = |row: usize, col: usize| import::cell::at(&constants, row, col);
    assert_eq!(
        setting::get(&db, key::TAX_RATE).unwrap(),
        import::cell::as_rate_bp(&at(1, 4)),
        "Constants!E2"
    );
    assert_eq!(
        setting::get(&db, key::PAY_PERIODS_PER_YEAR).unwrap(),
        import::cell::as_i64(&at(1, 6)),
        "Constants!G2"
    );
    assert_eq!(
        setting::get(&db, key::PAY_PERIOD_DAYS).unwrap(),
        import::cell::as_i64(&at(1, 7)),
        "Constants!H2"
    );

    // Sanity: the rate that arrived is a rate, not a fraction stored at the
    // wrong scale. The amount is invented; only the rate comes off the sheet.
    let bp = setting::get(&db, key::TAX_RATE).unwrap().unwrap();
    let taxed = mistermanager::calc::tax(Cents::from_dollars(1_000), bp).unwrap();
    assert!(
        taxed > Cents::from_dollars(1_000) && taxed < Cents::from_dollars(1_500),
        "a sales tax rate read at the wrong scale: {taxed:?}"
    );
}

/// The import skips a code it has already made an account for, so re-running
/// it is a no-op rather than a `UNIQUE (code, kind)` failure -- which is what
/// makes the first pass of a two-pass import safe to repeat.
#[test]
fn importing_twice_does_not_duplicate_accounts() {
    let Some(path) = workbook() else {
        return;
    };
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    import::constants::import(&db, &mut sheets).unwrap();
    let once = account::list(&db).unwrap();
    import::constants::import(&db, &mut sheets).unwrap();
    let twice = account::list(&db).unwrap();

    assert!(!once.is_empty(), "the sheet lists no accounts at all");
    assert_eq!(
        once.iter().map(|a| a.id).collect::<Vec<_>>(),
        twice.iter().map(|a| a.id).collect::<Vec<_>>()
    );
}

/// The naming, banding and ordering on the Accounts screen are the owner's,
/// and `account` is not an imported table -- so a `--replace` rebuilds every
/// figure in the database and leaves all three exactly as they were.
#[test]
fn the_owners_naming_and_ordering_survive_a_replace_import() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    let Some((db, _sheets)) = common::imported(today) else {
        return;
    };
    let path = workbook().expect("common::imported already found it");

    // Rename the last cash account, move it to the front, and re-band it --
    // three edits the import can never reproduce.
    let cash = account::list_by_kind(&db, Kind::Cash).unwrap();
    assert!(cash.len() > 1, "the sheet lists too few cash accounts");
    let moved = cash.last().unwrap().id;
    account::set_name(&db, moved, "Rainy Day").unwrap();
    account::set_group(&db, moved, Group::Savings).unwrap();
    account::reorder(&db, moved, 0).unwrap();
    let before = account::list(&db).unwrap();

    import::import_all(&db, &path, today, true).unwrap();

    let after = account::list(&db).unwrap();
    assert_eq!(
        after
            .iter()
            .map(|a| (a.id, a.name.clone(), a.group, a.sort))
            .collect::<Vec<_>>(),
        before
            .iter()
            .map(|a| (a.id, a.name.clone(), a.group, a.sort))
            .collect::<Vec<_>>(),
        "a --replace overwrote the owner's own naming or ordering"
    );
}
