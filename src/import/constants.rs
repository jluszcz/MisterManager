use super::cell::{self, as_date, as_i64, as_rate_bp, as_text};
use super::{Sheets, sheet};
use crate::db::Db;
use crate::db::account::{self, Kind};
use crate::db::setting::{self, key};
use anyhow::{Context, Result};

/// Sheet `Constants` layout:
///
/// ```text
///   A            C              E               G                    H                  J      K
/// 1 Cash Accounts Credit Accounts Sales Tax Rate Annual Pay Periods  Pay Period Length  Today  Birth Date
/// 2 CHK          CC1            0.0625          26                  14                 ...    ...
/// 3 SAV          CC2
/// 4 BKR          CHK
/// 5              CC3
/// ```
///
/// One code can appear in both columns -- a checking account and a card at
/// the same bank -- which is why accounts are keyed by `(code, kind)`.
pub fn import(db: &Db, sheets: &mut Sheets) -> Result<()> {
    let range = sheet(sheets, "Constants")?;
    let at = |row: usize, col: usize| cell::at(&range, row, col);

    // The sheet stores codes in its own column order and nothing else, so a
    // new account is named by its code, takes its kind's default band, and
    // appends to whatever that kind already holds. All three are the owner's
    // to change on the Accounts screen, and `account` is not an imported
    // table, so a change made there survives a `--replace`.
    for (column, kind) in [(0usize, Kind::Cash), (2usize, Kind::Credit)] {
        let mut sort = account::list_by_kind(db, kind)?.len() as i64;
        for row in 1..range.height() {
            let Some(code) = as_text(&at(row, column)) else {
                continue;
            };
            if account::by_code(db, &code, kind)?.is_some() {
                continue;
            }
            account::insert(db, &code, &code, kind, sort)?;
            sort += 1;
        }
    }

    let rate_bp = as_rate_bp(&at(1, 4)).context("Constants!E2 is not a sales tax rate")?;
    setting::set(db, key::TAX_RATE, rate_bp)?;

    let periods = as_i64(&at(1, 6)).context("Constants!G2 is not an integer")?;
    setting::set(db, key::PAY_PERIODS_PER_YEAR, periods)?;

    if let Some(today) = as_date(&at(1, 9)) {
        setting::set(db, key::WORKBOOK_TODAY, today)?;
    }
    if let Some(birth) = as_date(&at(1, 10)) {
        setting::set(db, key::BIRTH_DATE, birth)?;
    }
    Ok(())
}
