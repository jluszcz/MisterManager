//! What the workbook-oracle tests share.
//!
//! Integration tests compile as separate crates, so this is reached with
//! `mod common;` rather than by an import. Not every file uses every helper,
//! hence the `dead_code` allowance: unused-in-this-crate is the normal state
//! of a shared test module.
#![allow(dead_code)]

use chrono::NaiveDate;
use mistermanager::db::account::{self, Group, Kind};
use mistermanager::db::{AccountId, Db, setting};
use mistermanager::savings_block::Block;
use mistermanager::{db, import};
use std::path::PathBuf;

/// The workbook is personal data and is not in the repository. Tests that
/// need it skip -- loudly, so a run is never silently a no-op -- unless
/// `MM_REQUIRE_WORKBOOK=1` is set, in which case a missing workbook is a hard
/// failure instead of a skip. See README.md.
///
/// `MM_WORKBOOK` says where it is. There is deliberately no default: where
/// the owner keeps their finances is the same kind of fact as the account
/// codes below, and a fallback path would put it back in the repository the
/// one place nobody would think to grep.
pub fn workbook() -> Option<PathBuf> {
    let Ok(raw) = std::env::var("MM_WORKBOOK") else {
        missing("MM_WORKBOOK is not set (the path to Money.xlsx)");
        return None;
    };
    let path = PathBuf::from(raw);
    if path.exists() {
        return Some(path);
    }
    missing(&format!(
        "MM_WORKBOOK names {}, which does not exist",
        path.display()
    ));
    None
}

/// The three accounts the app cannot work out for itself, by code.
///
/// Every one of them is a fact about the *owner's* accounts that the workbook
/// does not carry: the `Savings` sheet names its two blocks by position and
/// no cell anywhere says which account is the current one. The app asks for
/// all three on the Accounts screen. A test has no screen to ask on, and the
/// codes cannot live in this repository -- see the root `CLAUDE.md` -- so
/// they come from the environment, in this order:
///
/// ```sh
/// MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<workbook> \
///   MM_ACCOUNTS=<checking>,<goal block>,<bucket block> \
///   cargo test --features import
/// ```
///
/// Unset, the tests that need it skip, exactly as they do for a missing
/// workbook, and `MM_REQUIRE_WORKBOOK=1` turns the skip into a failure.
pub struct Roles {
    pub checking: String,
    pub goals: String,
    pub buckets: String,
}

pub fn roles() -> Option<Roles> {
    let raw = match std::env::var("MM_ACCOUNTS") {
        Ok(raw) => raw,
        Err(_) => {
            missing("MM_ACCOUNTS is not set (<checking>,<goal block>,<bucket block>)");
            return None;
        }
    };
    let codes: Vec<&str> = raw.split(',').map(str::trim).collect();
    let [checking, goals, buckets] = codes[..] else {
        panic!("MM_ACCOUNTS must be three account codes: <checking>,<goal block>,<bucket block>");
    };
    Some(Roles {
        checking: checking.to_string(),
        goals: goals.to_string(),
        buckets: buckets.to_string(),
    })
}

/// Report a fixture the run does not have: a hard failure under
/// `MM_REQUIRE_WORKBOOK=1`, and otherwise a line on stderr so a skipped run
/// is never a silent one.
fn missing(what: &str) {
    if std::env::var("MM_REQUIRE_WORKBOOK").as_deref() == Ok("1") {
        panic!("MM_REQUIRE_WORKBOOK=1 but {what}");
    }
    eprintln!("skipping: {what} (set MM_REQUIRE_WORKBOOK=1 to fail instead of skip)");
}

/// Say what the Accounts screen would say, against a database that has
/// already imported `Constants`: which account is the current one, and which
/// holds each `Savings` block.
pub fn configure(db: &Db, roles: &Roles) {
    let by_code = |code: &str| {
        account::by_code(db, code, Kind::Cash)
            .unwrap()
            .unwrap_or_else(|| panic!("MM_ACCOUNTS names {code:?}, which the workbook does not"))
            .id
    };
    account::set_group(db, by_code(&roles.checking), Group::Checking).unwrap();
    setting::set(db, Block::Goals.key(), by_code(&roles.goals)).unwrap();
    setting::set(db, Block::Buckets.key(), by_code(&roles.buckets)).unwrap();
}

/// The account a `Savings` block's setting names -- how every test reaches a
/// container, since nothing in the repository may name one by code.
pub fn container(db: &Db, block: Block) -> AccountId {
    setting::get(db, block.key())
        .unwrap()
        .unwrap_or_else(|| panic!("{} was never configured", block.key()))
}

/// The date the workbook itself considers "today" (`Overview!B29`, driven by
/// `Constants!J2 = TODAY()`). Balances must be quoted at this date, not at the
/// system clock, or every comparison is meaningless.
pub fn workbook_today(sheets: &mut import::Sheets) -> NaiveDate {
    import::sheet(sheets, "Overview")
        .unwrap()
        .get((28, 1))
        .and_then(import::cell::as_date)
        .expect("Overview!B29 holds the workbook's today")
}

/// A fully imported database, the two-step first import done for you.
///
/// Pass one, with the roles unconfigured, can only write the accounts; the
/// roles are then set as the Accounts screen would set them, and pass two --
/// which needs no `--replace`, since pass one wrote neither a transaction nor
/// a goal -- runs the whole import.
pub fn imported(today: NaiveDate) -> Option<(Db, import::Sheets)> {
    let path = workbook()?;
    let roles = roles()?;
    let db = db::open_in_memory().unwrap();

    match import::import_all(&db, &path, today, false).unwrap() {
        import::Report::AccountsOnly { accounts } => assert!(accounts > 0),
        import::Report::Full(_) => panic!("an unconfigured database imported Savings"),
    }
    configure(&db, &roles);
    match import::import_all(&db, &path, today, false).unwrap() {
        import::Report::Full(_) => {}
        import::Report::AccountsOnly { .. } => panic!("a configured database skipped Savings"),
    }

    Some((db, import::open(&path).unwrap()))
}

/// A cell as `Cents`, zero where the sheet is blank.
///
/// The workbook is a live document: its owner edits it and `TODAY()`
/// recalculates, so row counts and balances drift between runs. Every figure
/// is therefore compared against the workbook's **own cached value** for the
/// same cell rather than against a literal. That is still a real check --
/// Excel's formula engine and this crate's SQL are independent routes to the
/// same number -- and it stays true across edits.
pub fn sheet_cents(
    range: &import::SheetRange,
    row: usize,
    col: usize,
) -> mistermanager::money::Cents {
    range
        .get((row, col))
        .and_then(import::cell::as_cents)
        .unwrap_or(mistermanager::money::Cents::ZERO)
}

/// Whether the sheet's `Overview!E2` is a day the app would quote at all.
///
/// The ad-hoc date is the first paycheck eve strictly *after* today, so on the
/// eve itself the app rolls over to the eve of the paycheck after while the
/// sheet's hand-typed `E2` still reads today. They disagree by design that one
/// day in the cycle, and the workbook holds no column quoted at the day the app
/// means -- so the comparison is dropped for that run rather than pinned
/// against a figure the sheet does not carry. Everything the other two columns
/// assert is unaffected.
///
/// **Only that day.** An `E2` already in the past is a sheet the owner has not
/// bumped, which is a real disagreement between the two and goes on failing
/// loudly: `B29` recalculates and `E2` does not, so a skip there would hide a
/// stale oracle for a whole cadence at a time.
///
/// A loud line on stderr, like every other fixture a run does not have, so the
/// day it happens is never a silently thinner test.
pub fn eve_is_comparable(today: NaiveDate, eve: NaiveDate) -> bool {
    if today != eve {
        return true;
    }
    eprintln!(
        "skipping the Paycheck-Eve comparison: the workbook's today ({today}) is its own \
         Overview!E2, which the app rolls past by design"
    );
    false
}

/// Whether the sheet's `Overview!C30` is quoted at the day the app's Month-End
/// column names.
///
/// The sheet's projection is `EOMONTH(today, 0) + 1`; the app derives Month-End
/// from the paycheck eve instead, so once that eve crosses into the next month
/// the app deliberately quotes the month after the one the sheet does, and the
/// workbook holds no cached column at that date. The comparison is dropped for
/// that run rather than pinned against a figure the sheet does not carry --
/// the same trade `eve_is_comparable` makes, for the other derived column.
///
/// A loud line on stderr, like every other fixture a run does not have, so the
/// runs it happens on are never a silently thinner test.
pub fn month_end_is_comparable(today: NaiveDate, month_end: NaiveDate) -> bool {
    if month_end == mistermanager::calc::month_end_projection(today) {
        return true;
    }
    eprintln!(
        "skipping the Month-End comparison: the app quotes {month_end}, a month past the \
         workbook's today ({today}), which Overview!C30 does not reach"
    );
    false
}
