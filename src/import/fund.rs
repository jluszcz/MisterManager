//! `Planning!I2:M<n>` -> the `fund` table.
//!
//! ```text
//!    I               J         K         L      M
//! 1                  Target %  Actual %  Delta  Actual Value
//! 2  Bonds           0.10      0.10      0      10000
//! 3  International   0.36      0.40      0      40000
//! 4  Domestic        0.54      0.50      0.04   50000
//! 5                                             100000
//! ```
//!
//! `K` and `L` are never read: they are derived, and recomputing them is what
//! the screen is for. `M5` is the total, not a fund -- the real workbook
//! leaves `I5` *and* `J5` blank beside it, and that pair is what ends the
//! scan. Pairing the end-of-block check with `M` (the way a bill row pairs
//! label and amount) would misread `M5` as a fund with a dropped name and
//! refuse the whole import; pairing it with name *alone* would instead let a
//! fund that lost its name -- but kept its `J` formula -- vanish silently
//! and shrink the portfolio. `J` is what tells the two apart: a genuine fund
//! row always carries a target percentage, the sheet's own total never does.

use super::SheetRange;
use super::cell::{self, as_cents, as_rate_bp, as_text};
use crate::calc::fund::BONDS_START_AGE;
use crate::db::Db;
use crate::db::fund::{self, NewFund, Target};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, bail, ensure};

/// How far a cached `J` may sit from the age rule and still be read as it.
///
/// One basis point: the sheet stores `(age - 30) / 100` as a float, so the
/// value that comes back is the rule's own figure give or take float dust.
const AGE_MATCH_TOLERANCE_BP: i64 = 1;

/// Whether importing this fund block left every row's target frozen as a
/// fixed share, with no row tracking age.
///
/// True only when the block actually held rows (`count > 0`) *and* `age` was
/// `None`: without an age, `import` cannot tell an age-tracking bond row from
/// an ordinary one, so `first_is_age` is unconditionally false and every row
/// -- including what would otherwise be `AgeOver30` -- is stored as a
/// `RemainderShare` with its cached percentage frozen permanently. That
/// storage decision is silent on its own: once every row has a resolved
/// target, `Funds::needs_birth_date` can never be true for that data, so the
/// screen's birth-date prompt can never open. Callers report this so the
/// owner can set the birth date and re-import with `--replace`, or fix the
/// row's kind by hand on the Funds screen with `E`.
pub fn targets_frozen(count: usize, age: Option<i64>) -> bool {
    count > 0 && age.is_none()
}

/// Read the block into the table, and say how many rows it held.
///
/// `age` is the owner's age *as the sheet computed it* — from the birth date
/// and the workbook's own `Today`, not the app's — because the `J` values
/// being classified are the ones Excel cached against that date.
pub fn import(db: &Db, range: &SheetRange, age: Option<i64>) -> Result<usize> {
    let at = |row: usize, col: usize| cell::at(range, row, col);

    // Scanned first and inserted second: the shares are recovered against the
    // age row's own cached percentage, which is not known until the first row
    // has been read and classified.
    let mut scanned: Vec<(String, BasisPoints, Cents)> = Vec::new();
    for row in 1..range.height() {
        let name = as_text(&at(row, 8));
        let target = as_rate_bp(&at(row, 9));
        let name = match (name, target) {
            // Blank in both is the sheet's own totals row: `M5` carries a
            // sum, but nothing here is a fund's own target, so the block
            // ends honestly rather than swallowing it.
            (None, None) => break,
            // A target with no name is a fund that lost its label -- the `J`
            // formula still fired, so this was a real row, not a footer, and
            // reading it as the end of the block would silently shrink the
            // portfolio.
            (None, Some(_)) => bail!(
                "Planning row {} is half a fund: has a target percentage but no name",
                row + 1
            ),
            (Some(name), _) => name,
        };
        // A dropped value shrinks the portfolio and moves every percentage
        // beside it, so half a row is an error rather than a skip -- the way
        // half a bill is.
        let value = as_cents(&at(row, 12)).with_context(|| {
            format!(
                "Planning row {} is half a fund: {name:?} has no value",
                row + 1
            )
        })?;
        let target =
            target.with_context(|| format!("Planning!J{} is not a target percentage", row + 1))?;
        scanned.push((name, target, value));
    }

    // The kind comes from the value, not the formula: `calamine` hands back
    // cached values, and parsing
    // `(DATEDIF(Dates[Birth Date],Dates[Today],"y")-30)/100` would be brittle.
    //
    // Only the first scanned row is ever classified as the age row -- the
    // workbook has bonds first, and this does not generalize to a sheet with
    // the age row in another position. `calc::fund::compute` is the general
    // one: it sums the claim of every `AgeOver30` row wherever it sits.
    let first_is_age = match (scanned.first(), age) {
        (Some((_, target, _)), Some(age)) => {
            (target.0 - (age - BONDS_START_AGE) * 100).abs() <= AGE_MATCH_TOLERANCE_BP
        }
        _ => false,
    };
    let remainder = match first_is_age {
        true => BasisPoints::ONE.0 - scanned[0].1.0,
        false => BasisPoints::ONE.0,
    };

    let count = scanned.len();
    for (ord, (name, target, value)) in scanned.into_iter().enumerate() {
        let stored = match ord == 0 && first_is_age {
            true => Target::AgeOver30,
            false => {
                ensure!(
                    remainder > 0,
                    "Planning!J{} is a share of a zero remainder, which is undefined",
                    ord + 2
                );
                // Nearest basis point rather than truncated: a share stored
                // as a fraction of the remainder has to divide back out to
                // the figure it was entered as.
                Target::RemainderShare(BasisPoints(
                    ((i128::from(target.0) * i128::from(BasisPoints::ONE.0)
                        + i128::from(remainder) / 2)
                        / i128::from(remainder)) as i64,
                ))
            }
        };
        fund::insert(
            db,
            &NewFund {
                name,
                ord: ord as i64,
                target: stored,
                actual: value,
            },
        )?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use calamine::{Cell, Data, Range};

    fn cell(row: u32, col: u32, value: Data) -> Cell<Data> {
        Cell::new((row, col), value)
    }

    /// One block row: `I` name, `J` cached target fraction, `M` value.
    fn block_row(row: u32, name: &str, target: f64, value: f64) -> Vec<Cell<Data>> {
        vec![
            cell(row, 8, Data::String(name.into())),
            cell(row, 9, Data::Float(target)),
            cell(row, 12, Data::Float(value)),
        ]
    }

    /// The workbook's own block, anchored at A1 the way calamine reads a
    /// worksheet whose used range starts there. Row 4 reproduces the sheet's
    /// own trailing total (`M5` in the live workbook) -- blank `I` and `J`
    /// beside a real `M` value -- so the fixture every test below shares
    /// already covers the shape that end-of-block detection has to get
    /// right.
    fn workbook_block() -> SheetRange {
        let mut cells = vec![cell(0, 0, Data::Empty)];
        cells.extend(block_row(1, "Bonds", 0.10, 30_000.0));
        cells.extend(block_row(2, "International", 0.36, 60_000.0));
        cells.extend(block_row(3, "Domestic", 0.54, 90_000.0));
        cells.push(cell(4, 12, Data::Float(180_000.0)));
        Range::from_sparse(cells)
    }

    /// The kind comes from the value, not the formula: `calamine` hands back
    /// cached values, and `(DATEDIF(...)-30)/100` would be brittle to parse.
    #[test]
    fn the_first_row_is_the_age_row_when_its_percentage_matches_the_age() {
        let db = db::open_in_memory().unwrap();

        assert_eq!(import(&db, &workbook_block(), Some(40)).unwrap(), 3);

        let stored = fund::list(&db).unwrap();
        assert_eq!(stored[0].name, "Bonds");
        assert_eq!(stored[0].target, fund::Target::AgeOver30);
        assert_eq!(stored[0].actual, Cents::from_dollars(30_000));
    }

    /// `0.36 / 0.90` and `0.54 / 0.90` come back as exactly 40% and 60%.
    #[test]
    fn the_shares_are_recovered_against_the_age_rows_remainder() {
        let db = db::open_in_memory().unwrap();

        import(&db, &workbook_block(), Some(40)).unwrap();

        let stored = fund::list(&db).unwrap();
        assert_eq!(
            stored[1].target,
            fund::Target::RemainderShare(BasisPoints(4_000))
        );
        assert_eq!(
            stored[2].target,
            fund::Target::RemainderShare(BasisPoints(6_000))
        );
        assert_eq!(
            stored.iter().map(|f| f.ord).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "sheet row order is display order"
        );
    }

    /// A first row that is not the age rule is an ordinary share row, taking
    /// its share of the whole.
    #[test]
    fn a_first_row_that_does_not_match_the_age_is_a_share_of_the_whole() {
        let db = db::open_in_memory().unwrap();
        let mut cells = vec![cell(0, 0, Data::Empty)];
        cells.extend(block_row(1, "International", 0.4, 100.0));
        cells.extend(block_row(2, "Domestic", 0.6, 100.0));

        import(&db, &Range::from_sparse(cells), Some(40)).unwrap();

        let stored = fund::list(&db).unwrap();
        assert_eq!(
            stored[0].target,
            fund::Target::RemainderShare(BasisPoints(4_000))
        );
        assert_eq!(
            stored[1].target,
            fund::Target::RemainderShare(BasisPoints(6_000))
        );
    }

    /// With no birth date there is nothing to match the first row against.
    #[test]
    fn an_unknown_age_makes_every_row_a_share_row() {
        let db = db::open_in_memory().unwrap();

        import(&db, &workbook_block(), None).unwrap();

        assert!(
            fund::list(&db)
                .unwrap()
                .iter()
                .all(|f| matches!(f.target, fund::Target::RemainderShare(_)))
        );
    }

    /// A dropped fund silently shrinks the portfolio and moves every
    /// percentage beside it, so half a row is an error rather than a skip --
    /// the way half a bill is.
    #[test]
    fn a_row_with_a_name_and_no_value_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let range = Range::from_sparse(vec![
            cell(0, 0, Data::Empty),
            cell(1, 8, Data::String("Bonds".into())),
            cell(1, 9, Data::Float(0.10)),
        ]);

        let err = import(&db, &range, Some(40)).unwrap_err();
        assert!(err.to_string().contains("half a fund"), "{err}");
    }

    /// The real workbook follows its last fund with a totals row -- `M5` in
    /// the live sheet -- that leaves both `I` and `J` blank beside a real
    /// `M`. That row is the sheet's own total, not a fund: the block ends
    /// there cleanly, the funds before it are still imported, and the total
    /// itself is simply never read.
    #[test]
    fn the_sheets_own_total_is_not_a_fund() {
        let db = db::open_in_memory().unwrap();
        let mut cells = vec![cell(0, 0, Data::Empty)];
        cells.extend(block_row(1, "Bonds", 0.10, 30_000.0));
        cells.push(cell(4, 12, Data::Float(30_000.0)));

        assert_eq!(
            import(&db, &Range::from_sparse(cells), Some(40)).unwrap(),
            1
        );
    }

    /// A blank name beside a *present* target percentage is not the sheet's
    /// total -- the totals row never carries a `J` formula -- so it is a
    /// fund that lost its label, and reading it as the end of the block
    /// would silently shrink the portfolio rather than saying so.
    #[test]
    fn a_row_with_a_target_percentage_and_no_name_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let range = Range::from_sparse(vec![
            cell(0, 0, Data::Empty),
            cell(1, 9, Data::Float(0.10)),
            cell(1, 12, Data::Float(30_000.0)),
        ]);

        let err = import(&db, &range, Some(40)).unwrap_err();
        assert!(err.to_string().contains("half a fund"), "{err}");
    }

    /// A row with no cached percentage cannot be classified, and a zero share
    /// would quietly starve that fund forever.
    #[test]
    fn a_row_with_no_target_percentage_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let range = Range::from_sparse(vec![
            cell(0, 0, Data::Empty),
            cell(1, 8, Data::String("Bonds".into())),
            cell(1, 12, Data::Float(30_000.0)),
        ]);

        let err = import(&db, &range, Some(40)).unwrap_err();
        assert!(err.to_string().contains("J2"), "{err}");
    }

    /// At 130 the age row claims everything, and a share of nothing is
    /// undefined rather than zero.
    #[test]
    fn a_share_of_a_zero_remainder_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let mut cells = vec![cell(0, 0, Data::Empty)];
        cells.extend(block_row(1, "Bonds", 1.0, 1.0));
        cells.extend(block_row(2, "Domestic", 0.0, 1.0));

        let err = import(&db, &Range::from_sparse(cells), Some(130)).unwrap_err();
        assert!(err.to_string().contains("remainder"), "{err}");
    }

    /// A workbook without the block still imports; the screen simply starts
    /// empty.
    #[test]
    fn a_missing_block_imports_no_funds() {
        let db = db::open_in_memory().unwrap();
        let range = Range::from_sparse(vec![cell(0, 0, Data::Empty)]);

        assert_eq!(import(&db, &range, Some(40)).unwrap(), 0);
        assert_eq!(fund::count(&db).unwrap(), 0);
    }

    /// Storage does not change when the age is unknown -- every row is still
    /// stored, just as a frozen share -- but the fact is now reportable.
    #[test]
    fn an_unknown_age_with_funds_present_reports_frozen_targets() {
        let db = db::open_in_memory().unwrap();

        let count = import(&db, &workbook_block(), None).unwrap();

        assert!(targets_frozen(count, None));
    }

    /// A known age lets the bond row track age, so nothing is frozen.
    #[test]
    fn a_known_age_does_not_report_frozen_targets() {
        let db = db::open_in_memory().unwrap();

        let count = import(&db, &workbook_block(), Some(40)).unwrap();

        assert!(!targets_frozen(count, Some(40)));
    }

    /// A workbook with no fund block and no birth date has nothing worth
    /// warning about -- there is no bond row to have frozen.
    #[test]
    fn no_funds_and_no_birth_date_does_not_report_frozen_targets() {
        let db = db::open_in_memory().unwrap();
        let range = Range::from_sparse(vec![cell(0, 0, Data::Empty)]);

        let count = import(&db, &range, None).unwrap();

        assert!(!targets_frozen(count, None));
    }

    /// The scan stops at the first row blank in both columns: stray text
    /// further down column I must never import as a phantom fund.
    #[test]
    fn the_scan_stops_at_the_end_of_the_block() {
        let db = db::open_in_memory().unwrap();
        let mut cells = vec![cell(0, 0, Data::Empty)];
        cells.extend(block_row(1, "Bonds", 0.10, 30_000.0));
        cells.extend(block_row(6, "stray note", 9.0, 999.0));

        assert_eq!(
            import(&db, &Range::from_sparse(cells), Some(40)).unwrap(),
            1
        );
    }
}
