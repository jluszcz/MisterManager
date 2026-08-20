mod common;

use chrono::NaiveDate;
use common::{sheet_cents, workbook, workbook_today};
use mistermanager::db::account::{self, Kind};
use mistermanager::db::txn;
use mistermanager::money::Cents;
use mistermanager::{db, import};

/// Import everything and hand back the database, the open workbook, and the
/// ledger import's own report.
///
/// Only `Constants` and the ledgers: nothing here reads `Savings`, so this
/// needs neither of the two container settings and does not go through
/// `common::imported`.
fn loaded() -> Option<(db::Db, import::Sheets, import::ledger::Imported)> {
    let path = workbook()?;
    let db = db::open_in_memory().unwrap();
    let mut sheets = import::open(&path).unwrap();
    import::constants::import(&db, &mut sheets).unwrap();
    let report = import::ledger::import(&db, &mut sheets).unwrap();
    Some((db, sheets, report))
}

/// Every per-account balance the `Overview` sheet quotes, in one column: the
/// rows labelled `... To-Date` or `... Projection` that are not one of the
/// sheet's own subtotals.
///
/// Read out of the sheet rather than written down. Which account each row
/// belongs to is knowledge the workbook does not carry and this repository
/// may not hold -- the rows are labelled with the owner's real institutions
/// -- so what is compared is the *set* of figures rather than a row-by-row
/// mapping. A balance landing on the wrong account still fails, unless two
/// accounts' balances were exchanged; that the accounts are told apart at all
/// is `separates_two_accounts_sharing_a_code` below.
fn per_account_column(overview: &import::SheetRange, suffix: &str) -> Vec<Cents> {
    const SUBTOTALS: [&str; 3] = ["Savings", "Credit Card", "Net"];
    (0..overview.height())
        .filter(|row| {
            let Some(label) = overview.get((*row, 0)).and_then(import::cell::as_text) else {
                return false;
            };
            let Some(name) = label.strip_suffix(suffix).map(str::trim) else {
                return false;
            };
            !SUBTOTALS.contains(&name)
        })
        .map(|row| sheet_cents(overview, row, 2))
        .collect()
}

/// The same figures, sorted, so two lists can be compared as sets.
fn sorted(mut figures: Vec<Cents>) -> Vec<Cents> {
    figures.sort_by_key(|c| c.0);
    figures
}

#[test]
fn every_cash_and_credit_balance_agrees_with_the_workbook() {
    let Some((db, mut sheets, _report)) = loaded() else {
        eprintln!("skipping: Money.xlsx not found");
        return;
    };

    let today = workbook_today(&mut sheets);
    let month_end = mistermanager::calc::month_end_projection(today);
    let overview = import::sheet(&mut sheets, "Overview").unwrap();

    // Cash reads straight; credit is stored as debt and the Overview negates
    // it, so both kinds are quoted the sheet's way before being compared.
    let quoted = |at: NaiveDate| {
        let mut figures: Vec<Cents> = Vec::new();
        for account in account::list(&db).unwrap() {
            let balance = txn::balance_at(&db, account.id, at).unwrap();
            figures.push(match account.kind {
                Kind::Cash => balance,
                Kind::Credit => -balance,
            });
        }
        sorted(figures)
    };

    let to_date = per_account_column(&overview, "To-Date");
    assert!(
        to_date.len() >= 4,
        "the Overview sheet quotes only {} per-account balances",
        to_date.len()
    );
    assert_eq!(quoted(today), sorted(to_date), "to-date balances");
    assert_eq!(
        quoted(month_end),
        sorted(per_account_column(&overview, "Projection")),
        "month-end projections"
    );

    let net = |d: NaiveDate| {
        txn::balance_at_by_kind(&db, Kind::Cash, d).unwrap()
            - txn::balance_at_by_kind(&db, Kind::Credit, d).unwrap()
    };
    assert_eq!(net(today), sheet_cents(&overview, 28, 2), "net to-date");
    assert_eq!(
        net(month_end),
        sheet_cents(&overview, 29, 2),
        "net projection"
    );
}

/// The ad-hoc projection column (`Overview!E1:G14`), quoted at the workbook's
/// own hand-set projection date.
#[test]
fn the_adhoc_projection_column_agrees_with_the_workbook() {
    let Some((db, mut sheets, _report)) = loaded() else {
        return;
    };

    let overview = import::sheet(&mut sheets, "Overview").unwrap();
    let adhoc = overview
        .get((1, 4))
        .and_then(mistermanager::import::cell::as_date)
        .expect("Overview!E2 is the ad-hoc projection date");

    let net = txn::balance_at_by_kind(&db, Kind::Cash, adhoc).unwrap()
        - txn::balance_at_by_kind(&db, Kind::Credit, adhoc).unwrap();
    assert_eq!(net, sheet_cents(&overview, 13, 6), "net at the ad-hoc date");
}

/// One code in the live workbook names both a cash account and a card.
/// Collapsing them would make both balances wrong, so this pins that they
/// stay distinct -- and that neither has swallowed the other's rows.
///
/// The code is found rather than written down: it is whichever cash account's
/// code also names a card, which is exactly the property `UNIQUE (code, kind)`
/// exists for.
#[test]
fn separates_two_accounts_sharing_a_code() {
    let Some((db, mut sheets, _report)) = loaded() else {
        return;
    };

    let shared = account::list_by_kind(&db, Kind::Cash)
        .unwrap()
        .into_iter()
        .find(|cash| {
            account::by_code(&db, &cash.code, Kind::Credit)
                .unwrap()
                .is_some()
        })
        .expect("the workbook lists no code under both kinds");
    let card = account::by_code(&db, &shared.code, Kind::Credit)
        .unwrap()
        .unwrap();
    assert_ne!(shared.id, card.id);

    let today = workbook_today(&mut sheets);
    let overview = import::sheet(&mut sheets, "Overview").unwrap();
    let cards = per_account_column(&overview, "To-Date");
    let card_balance = -txn::balance_at(&db, card.id, today).unwrap();
    assert!(
        cards.contains(&card_balance),
        "the card's balance is not one the sheet quotes -- it has taken the \
         cash account's rows: {card_balance:?} not in {cards:?}"
    );
}

/// Row counts drift as the owner adds transactions, so compare against what
/// the sheets actually hold rather than a remembered number.
#[test]
fn imports_every_row_the_sheets_contain() {
    let Some((_conn, mut sheets, report)) = loaded() else {
        return;
    };

    let mut dated_rows = |name: &str| -> usize {
        let range = import::sheet(&mut sheets, name).unwrap();
        (1..range.height())
            .filter(|r| {
                range
                    .get((*r, 0))
                    .and_then(mistermanager::import::cell::as_date)
                    .is_some()
            })
            .count()
    };

    assert_eq!(report.cash_rows, dated_rows("Cash"));
    assert_eq!(report.credit_rows, dated_rows("Credit"));
    assert!(
        report.skipped.is_empty(),
        "skipped rows: {:?}",
        report.skipped
    );
    assert!(report.cash_rows > 300, "suspiciously few cash rows");
    assert!(report.credit_rows > 900, "suspiciously few credit rows");
}
