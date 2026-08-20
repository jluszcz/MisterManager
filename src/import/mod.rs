pub mod cell;
pub mod constants;
pub mod fund;
pub mod ledger;
pub mod savings;

use anyhow::{Context, Result, bail};
use calamine::{Range, Reader, Xlsx, open_workbook};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub type Sheets = Xlsx<BufReader<File>>;

/// calamine's own cell-value type, re-exported under the crate's own name so
/// callers -- inside or outside `import` -- never need to name `calamine`
/// directly.
pub use calamine::Data as CellData;

/// A read sheet range. Re-exported so integration tests -- which compile as
/// separate crates and cannot name `calamine` directly -- can hold one in a
/// helper signature.
pub type SheetRange = Range<CellData>;

pub fn open(path: &Path) -> Result<Sheets> {
    open_workbook(path).with_context(|| format!("opening workbook {}", path.display()))
}

pub fn sheet(sheets: &mut Sheets, name: &str) -> Result<SheetRange> {
    sheets
        .worksheet_range(name)
        .with_context(|| format!("reading sheet {name:?}"))
}

use crate::db::account;
use crate::db::setting::key;
use crate::db::{self, Db, bill, setting};
use chrono::NaiveDate;

/// What a run of [`import_all`] did.
///
/// Two variants because there are two things a run can be, and the caller has
/// to say something different about each. A first import against an empty
/// database cannot read `Savings`: the sheet names its two blocks by position
/// and carries no account code, so the mapping from block to container is the
/// owner's to make on the Accounts screen. Reporting that as a `Full` run
/// with zero goals would be a healthy exit code over a database missing every
/// goal it has.
#[derive(Debug)]
pub enum Report {
    /// The `Savings` containers are not configured, so only `Constants` ran.
    /// The accounts are in the database, waiting to be pointed at the sheet's
    /// two blocks; the next `import` needs no flag, because a run that wrote
    /// no transactions and no goals leaves `has_imported_data` false.
    AccountsOnly {
        accounts: usize,
    },
    Full(Full),
}

#[derive(Debug)]
pub struct Full {
    pub ledger: ledger::Imported,
    pub savings: savings::Imported,
    /// Fund rows read from `Planning!I2:M<n>`. Zero for a workbook without
    /// the block, which is not an error -- the screen simply starts empty.
    pub funds: usize,
    /// Set when the fund block held rows but no birth date was on record, so
    /// every row -- including what would otherwise be the age-tracking bond
    /// row -- was stored as a `RemainderShare` with its cached percentage
    /// frozen permanently, per `import::fund::targets_frozen`. That
    /// collapse is otherwise invisible: it leaves every row with a resolved
    /// target, so the Funds screen's birth-date prompt can never open for
    /// this data. Reported so the owner can set the birth date and
    /// re-import with `--replace`, or fix the row's kind by hand with `E`.
    pub fund_targets_frozen: bool,
}

/// Import, in dependency order: accounts and settings, then the ledgers, then
/// savings. Runs inside one SQL transaction, so a failure partway through --
/// an unknown account code, half a bill row -- leaves the database exactly as
/// it was before the run, not half-populated.
///
/// **Self-resolving rather than two commands.** `Savings` cannot be read
/// until the owner has said which account each of its two blocks belongs to,
/// and that mapping is not in the workbook. So this looks: with the mapping
/// configured it runs the whole import in one pass, and without it, it writes
/// the accounts and stops, reporting [`Report::AccountsOnly`] for the caller
/// to turn into "go and configure them". Only the first import against an
/// empty database is ever two steps -- the mapping survives every later run,
/// including a `--replace`.
///
/// Refuses to import into a database that already holds transactions or
/// goals: re-running an import is not additive, so doing so unconditionally
/// would double every row on a second run with no signal beyond a healthy
/// exit code. Pass `replace: true` to explicitly clear the previously
/// imported tables first.
pub fn import_all(db: &Db, path: &Path, today: NaiveDate, replace: bool) -> Result<Report> {
    let mut sheets = open(path)?;

    db.transaction(|db| {
        // Read before the clear, not after: `setting` *is* an imported table,
        // so a `--replace` takes the mapping with it. The accounts it names
        // are not, so what is read here is still true afterwards.
        let containers = savings::containers(db)?;

        if db::has_imported_data(db)? {
            if !replace {
                bail!(
                    "database already holds imported data; pass --replace to overwrite it, \
                     or delete the database file and re-import"
                );
            }
            db::clear_imported_data(db)?;
        }

        constants::import(db, &mut sheets)?;

        let Some(containers) = containers else {
            return Ok(Report::AccountsOnly {
                accounts: account::list(db)?.len(),
            });
        };
        savings::set_containers(db, &containers)?;

        let (funds, fund_targets_frozen) = planning(db, &mut sheets, today)?;

        let ledger = ledger::import(db, &mut sheets)?;
        let savings = savings::import(db, &mut sheets, today, &containers)?;

        Ok(Report::Full(Full {
            ledger,
            savings,
            funds,
            fund_targets_frozen,
        }))
    })
}

/// Sheet `Planning` -> settings, the `bill` table, and the `fund` table. Cell
/// references are in the comments so the mapping can be checked against the
/// workbook.
///
/// `today` reaches only the fund block, and only as a fallback: the cached
/// `J` percentages were computed against `Constants!J2`, so that is the date
/// the classifying age is derived at wherever the sheet carries one.
fn planning(db: &Db, sheets: &mut Sheets, today: NaiveDate) -> Result<(usize, bool)> {
    let range = sheet(sheets, "Planning")?;
    let at = |row: usize, col: usize| cell::at(&range, row, col);
    let cents = |row: usize, col: usize| cell::as_cents(&at(row, col));

    for (row, col, setting_key) in [
        (0, 3, key::PLANNING_TARGET),     // D1
        (2, 3, key::PINNED_EXCESS),       // D3, Excess (Fixed)
        (10, 9, key::PLANNING_BUFFER),    // J11
        (18, 4, key::BILL_PAYMENT_CAP),   // E19
        (19, 4, key::MOM_AND_DAD_ANNUAL), // E20
        (23, 4, key::GOALS_FLOOR),        // E24
    ] {
        if let Some(v) = cents(row, col) {
            setting::set(db, setting_key, v)?;
        }
    }

    // The monthly bills, `C7:D12`: C7/C8 are housing, C9:C12 the rest. C6 is
    // the housing subtotal, not a bill, and is recomputed rather than read.
    // Labels are indented in the sheet ("  Mortgage"); `as_text` trims.
    for (rows, category) in [
        (6..=7, bill::Category::Housing),
        (8..=11, bill::Category::Other),
    ] {
        let mut sort = 0;
        for row in rows {
            let label = cell::as_text(&at(row, 2));
            let amount = cents(row, 3);
            match (label, amount) {
                (Some(label), Some(cents)) => {
                    bill::insert(
                        db,
                        &bill::NewBill {
                            label,
                            cents,
                            category,
                            sort,
                        },
                    )?;
                    sort += 1;
                }
                // Blank in both columns is the end of the block.
                (None, None) => {}
                // A dropped bill inflates the excess the waterfall has left to
                // allocate and skews every downstream transfer instruction, so
                // half a row is an error rather than a skip.
                (label, amount) => bail!(
                    "Planning row {} is half a bill: label {label:?}, amount {amount:?}",
                    row + 1
                ),
            }
        }
    }

    // Fractions, stored as whole percent: F19, then F25:F27.
    for (row, setting_key) in [
        (18, key::BILL_PAYMENT_PCT),
        (24, key::SPLIT_FUTURE_HOUSING_PCT),
        (25, key::SPLIT_RETIREMENT_PCT),
        (26, key::SPLIT_INVESTMENT_PCT),
    ] {
        if let Some(pct) = cell::as_percent(&at(row, 5)) {
            setting::set(db, setting_key, pct)?;
        }
    }

    // `Constants` has already run, so the birth date is on record if the
    // sheet carries one.
    let quoted_at = setting::get(db, key::WORKBOOK_TODAY)?.unwrap_or(today);
    let age = setting::get(db, key::BIRTH_DATE)?
        .map(|birth| crate::calc::fund::whole_years(birth, quoted_at));
    let count = fund::import(db, &range, age)?;
    Ok((count, fund::targets_frozen(count, age)))
}
