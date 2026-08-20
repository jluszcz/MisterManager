use super::cell::{self, as_cents, as_date, as_text};
use super::{CellData, Sheets, sheet};
use crate::db::account::{self, Kind};
use crate::db::txn::{self, NewTxn};
use crate::db::{AccountId, Db};

use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Imported {
    pub cash_rows: usize,
    pub credit_rows: usize,
    /// Rows that could not be read, described well enough to find in the sheet.
    pub skipped: Vec<String>,
}

pub fn import(db: &Db, sheets: &mut Sheets) -> Result<Imported> {
    let mut skipped = Vec::new();
    let cash_rows = import_sheet(db, sheets, "Cash", Kind::Cash, &mut skipped)?;
    let credit_rows = import_sheet(db, sheets, "Credit", Kind::Credit, &mut skipped)?;
    Ok(Imported {
        cash_rows,
        credit_rows,
        skipped,
    })
}

/// `A=Date, B=Amount, C=Acct, D=Description`, header in row 1.
fn import_sheet(
    db: &Db,
    sheets: &mut Sheets,
    name: &str,
    kind: Kind,
    skipped: &mut Vec<String>,
) -> Result<usize> {
    let range = sheet(sheets, name)?;
    let mut ids: HashMap<String, AccountId> = HashMap::new();
    let mut count = 0;

    for row in 1..range.height() {
        let get = |col: usize| cell::at(&range, row, col);
        // Excel line number, for messages the reader can act on.
        let line = row + 1;

        let Some(date) = as_date(&get(0)) else {
            // Trailing blank rows inside the table range are normal, but a
            // row carrying an amount, an account, or a description is a
            // real row with a bad date and must be reported rather than
            // dropped.
            if has_data_despite_bad_date(&get(1), &get(2), &get(3)) {
                skipped.push(format!("{name}!A{line}: unreadable date"));
            }
            continue;
        };
        let Some(cents) = as_cents(&get(1)) else {
            skipped.push(format!("{name}!B{line}: unreadable amount"));
            continue;
        };
        let Some(code) = as_text(&get(2)) else {
            skipped.push(format!("{name}!C{line}: missing account"));
            continue;
        };
        let description = as_text(&get(3)).unwrap_or_default();

        let account_id = match ids.get(&code) {
            Some(id) => *id,
            None => {
                let account = account::by_code(db, &code, kind)?.with_context(|| {
                    format!("{name}!C{line}: no {kind:?} account with code {code:?}")
                })?;
                ids.insert(code.clone(), account.id);
                account.id
            }
        };

        txn::insert(
            db,
            &NewTxn {
                date,
                cents,
                account_id,
                description,
                recurring_txn_id: None,
            },
        )?;
        count += 1;
    }
    Ok(count)
}

/// Whether a row whose date failed to parse is nonetheless real: it carries
/// an amount, an account code, or a description. A genuinely blank trailing
/// row inside the table range carries none of the three, and is not
/// reported.
fn has_data_despite_bad_date(amount: &CellData, code: &CellData, description: &CellData) -> bool {
    as_cents(amount).is_some() || as_text(code).is_some() || as_text(description).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_date_row_with_only_an_account_code_is_reported() {
        assert!(has_data_despite_bad_date(
            &CellData::Empty,
            &CellData::String("CHK".to_string()),
            &CellData::Empty,
        ));
    }

    #[test]
    fn a_genuinely_empty_row_is_not_reported() {
        assert!(!has_data_despite_bad_date(
            &CellData::Empty,
            &CellData::Empty,
            &CellData::Empty,
        ));
    }
}
