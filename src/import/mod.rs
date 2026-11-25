pub mod cell;
pub mod constants;
pub mod ledger;
pub mod savings;

use anyhow::{Context, Result, bail, ensure};
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
use crate::db::{self, AccountId, Db, bill, setting, split_profile};
use crate::default_source::Source;
use crate::gate;
use crate::rate::BasisPoints;
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
        let defaults = default_sources(db)?;
        // The Accounts screen's third such setting, carried across for the
        // same reason and with the same tolerance: a stale id reads as unset.
        let investment = setting::get(db, key::INVESTMENT_ACCOUNT)?;
        // Carried for the same reason again: no sheet has a salary in it.
        let salary = setting::get(db, key::ANNUAL_SALARY)?;
        // And the gates' order and names, which the sheet states nowhere.
        let priority = setting::get(db, gate::PRIORITY)?;
        let gate_names = gate::Names::load(db)?;

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
        // Before the early return below, not after it: a `--replace` that
        // stops after the accounts has cleared `setting` just the same.
        set_default_sources(db, &defaults)?;
        if let Some(id) = investment {
            setting::set(db, key::INVESTMENT_ACCOUNT, id)?;
        }
        if let Some(salary) = salary {
            setting::set(db, key::ANNUAL_SALARY, salary)?;
        }
        gate_names.store(db)?;
        if let Some(priority) = priority {
            setting::set(db, gate::PRIORITY, priority)?;
        }

        let Some(containers) = containers else {
            return Ok(Report::AccountsOnly {
                // What the sheet can name, which is what "imported" means on
                // the line this count goes into. `Constants` carries cash and
                // credit codes and nothing else, so an investment account is
                // one the owner made on the Accounts screen and `account`
                // being in `PRESERVED_TABLES` is what kept it -- counting it
                // here would report it as something this run wrote.
                accounts: account::list_ledger(db)?.len(),
            });
        };
        savings::set_containers(db, &containers)?;

        planning(db, &mut sheets)?;

        let ledger = ledger::import(db, &mut sheets)?;
        let savings = savings::import(db, &mut sheets, today, &containers)?;

        Ok(Report::Full(Full { ledger, savings }))
    })
}

/// The account each money form's `From` opens on, read before a `--replace`
/// clears `setting`.
///
/// [`savings::containers`]' dance, for its reason: these are the owner's, set
/// on the Accounts screen beside the `Savings` blocks and the interest
/// policy, and the accounts they name are not imported, so what was true
/// before the clear is still true after it. An owner who lost them to a
/// `--replace` would find the screen's other five settings had survived it.
///
/// What differs is the tolerance. A dangling block key is a corrupt database
/// and `containers` says so; a dangling default is the same state as an unset
/// one -- a form opening on the head of its list -- so the ids are carried
/// across without being resolved, and an absent key stays absent.
fn default_sources(db: &Db) -> Result<Vec<(Source, AccountId)>> {
    let mut found = Vec::with_capacity(Source::ALL.len());
    for source in Source::ALL {
        if let Some(id) = setting::get(db, source.key())? {
            found.push((source, id));
        }
    }
    Ok(found)
}

/// Write them back, once `constants::import` has run.
fn set_default_sources(db: &Db, defaults: &[(Source, AccountId)]) -> Result<()> {
    for (source, id) in defaults {
        setting::set(db, source.key(), *id)?;
    }
    Ok(())
}

/// `F25:F27` into the active split profile, and only that one: the sheet
/// carries one unnamed set, and the owner's other profiles are theirs.
///
/// Merged over what the profile already holds, so a sheet carrying only two
/// of the three leaves the third standing; checked once all three are in,
/// since that is the set the waterfall will divide. The whole import is one
/// transaction, so a refusal here takes the rest of it with it.
fn splits(db: &Db, cells: [Option<BasisPoints>; 3]) -> Result<()> {
    let profile = split_profile::active(db)?;
    let mut shares = profile.shares;
    let [fh, rt, inv] = cells;
    for (share, cell) in [
        (&mut shares.future_housing, fh),
        (&mut shares.retirement, rt),
        (&mut shares.investment, inv),
    ] {
        if let Some(pct) = cell {
            *share = pct;
        }
    }
    crate::plan::check_shares(shares)?;
    split_profile::set_shares(db, profile.id, shares)
}

/// Sheet `Planning` -> settings and the `bill` table. Cell references are in
/// the comments so the mapping can be checked against the workbook.
fn planning(db: &Db, sheets: &mut Sheets) -> Result<()> {
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

    // Fractions, stored as basis points: F19 as a setting.
    if let Some(pct) = cell::as_rate_bp(&at(18, 5)) {
        setting::set(db, key::BILL_PAYMENT_PCT, pct)?;
    }

    splits(
        db,
        [24, 25, 26].map(|row| cell::as_rate_bp(&at(row, 5))), // F25:F27
    )?;

    // And the pin, for the reason the splits are checked at all: `D3` is a
    // hand-typed cell, and what it writes is the figure every line below it
    // is measured against rather than one the waterfall divides.
    crate::plan::check_pinned_excess(db)?;

    // The sheet carries both equity targets as shares of the whole
    // portfolio. What is stored is the split between them, because the bond
    // target moves with a birthday and these two would go stale beside it.
    let intl = cell::as_rate_bp(&at(2, 9)).context("Planning!J3 is not a rate")?;
    let us = cell::as_rate_bp(&at(3, 9)).context("Planning!J4 is not a rate")?;
    let equity = intl.0 + us.0;
    ensure!(equity > 0, "Planning!J3:J4 leave no equity to split");
    setting::set(
        db,
        key::INTL_EQUITY_SHARE,
        BasisPoints(intl.0 * 10_000 / equity),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::split_profile::Shares;

    fn shares(fh: i64, rt: i64, inv: i64) -> Shares {
        Shares {
            future_housing: BasisPoints(fh),
            retirement: BasisPoints(rt),
            investment: BasisPoints(inv),
        }
    }

    #[test]
    fn the_sheets_splits_land_in_the_active_profile_and_leave_the_others_alone() {
        let db = db::open_in_memory().unwrap();
        let first = split_profile::active(&db).unwrap().id;
        let maxed = split_profile::insert(&db, "Maxed", shares(1_000, 4_000, 2_000)).unwrap();
        split_profile::set_active(&db, maxed).unwrap();

        splits(
            &db,
            [Some(BasisPoints(2_500)), Some(BasisPoints(1_500)), None],
        )
        .unwrap();

        assert_eq!(
            split_profile::get(&db, maxed).unwrap().shares,
            shares(2_500, 1_500, 2_000),
            "a cell the sheet leaves blank keeps the profile's own share"
        );
        assert_eq!(
            split_profile::get(&db, first).unwrap().shares,
            shares(3_000, 2_000, 1_000)
        );
    }

    #[test]
    fn a_sheet_set_over_one_hundred_is_refused_and_writes_nothing() {
        let db = db::open_in_memory().unwrap();
        let err = splits(
            &db,
            [
                Some(BasisPoints(6_000)),
                Some(BasisPoints(3_000)),
                Some(BasisPoints(3_000)),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("120"), "{err}");
        assert_eq!(
            split_profile::active(&db).unwrap().shares,
            shares(3_000, 2_000, 1_000)
        );
    }
}
