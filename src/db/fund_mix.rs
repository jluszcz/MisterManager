//! The `fund_mix` table: a fund's published composition, as of the report it
//! was last read out of.
//!
//! Keyed on the ticker rather than on a [`super::holding::Holding`], because
//! a fund's composition is a property of the fund and not of who holds it --
//! one fetch of `USM`'s mix prices every account holding it. `report_date` is
//! per ticker because fund families file on their own schedules, and a
//! screen quoting a mix has to say how current it is.

use super::Db;
use super::date::{self, iso};
use crate::rate::BasisPoints;
use anyhow::{Result, bail};
use chrono::NaiveDate;
use rusqlite::{Row, params};
use std::str::FromStr;

/// What a slice of a fund's composition is invested in.
///
/// The variants are exactly the schema's `CHECK (asset_class IN (...))` list:
/// keep the two in step, or an insert that type-checks will fail against the
/// constraint. `Cash` is a fund genuinely holding cash, and `Unclassified` is
/// the classifier's own residual -- what a fund's filing lists that fits
/// none of the other five, carried rather than dropped so a mix's weights
/// still foot to `BasisPoints::ONE`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AssetClass {
    UsStock,
    IntlStock,
    UsBond,
    IntlBond,
    Cash,
    Unclassified,
}

impl AssetClass {
    /// Every class, in the order the Funds screen lists a mix's slices.
    ///
    /// Beside the enum rather than on the screen, for the reason
    /// `account::InterestPolicy::ALL` is: a screen offering a subset would
    /// leave a variant unreachable with nothing to say so.
    pub const ALL: [AssetClass; 6] = [
        AssetClass::UsStock,
        AssetClass::IntlStock,
        AssetClass::UsBond,
        AssetClass::IntlBond,
        AssetClass::Cash,
        AssetClass::Unclassified,
    ];

    /// This class's own place in [`AssetClass::ALL`].
    ///
    /// Both places that accumulate a figure per class key an array by it --
    /// `mix::classify` summing a filing's holdings, `allocation::apportion`
    /// summing a portfolio's balances -- and a second implementation of this
    /// mapping is a reordering of `ALL` away from mislabelling every slice
    /// one of them produces. Derived from `ALL` rather than matched out by
    /// hand so the two cannot come apart: there is one order, and this is it.
    pub fn index(self) -> usize {
        AssetClass::ALL
            .iter()
            .position(|class| *class == self)
            .expect("AssetClass::ALL names every variant")
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AssetClass::UsStock => "us_stock",
            AssetClass::IntlStock => "intl_stock",
            AssetClass::UsBond => "us_bond",
            AssetClass::IntlBond => "intl_bond",
            AssetClass::Cash => "cash",
            AssetClass::Unclassified => "unclassified",
        }
    }

    /// What the Funds screen calls this class. Prose rather than the string
    /// it is stored as, the way `account::TaxTreatment::label` is.
    pub fn label(self) -> &'static str {
        match self {
            AssetClass::UsStock => "U.S. Stock",
            AssetClass::IntlStock => "International Stock",
            AssetClass::UsBond => "U.S. Bond",
            AssetClass::IntlBond => "International Bond",
            AssetClass::Cash => "Cash",
            AssetClass::Unclassified => "Unclassified",
        }
    }
}

impl FromStr for AssetClass {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "us_stock" => Ok(AssetClass::UsStock),
            "intl_stock" => Ok(AssetClass::IntlStock),
            "us_bond" => Ok(AssetClass::UsBond),
            "intl_bond" => Ok(AssetClass::IntlBond),
            "cash" => Ok(AssetClass::Cash),
            "unclassified" => Ok(AssetClass::Unclassified),
            other => bail!("unknown asset class {other:?}"),
        }
    }
}

/// One class's share of a fund, as of its [`Mix::report_date`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Slice {
    pub class: AssetClass,
    pub weight: BasisPoints,
}

/// A fund's published composition, as of the filing it was read out of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mix {
    pub ticker: String,
    pub report_date: NaiveDate,
    pub slices: Vec<Slice>,
}

// Column order is fixed by `select_fund_mix!` below -- keep the two in sync.
// `ticker` is not read back: every caller already names the ticker it asked
// for, since every query here is scoped to one.
fn from_row(row: &Row<'_>) -> rusqlite::Result<(NaiveDate, Slice)> {
    let class: String = row.get(0)?;
    let weight: i64 = row.get(1)?;
    let report_date: String = row.get(2)?;
    Ok((
        date::parse(&report_date, 2)?,
        Slice {
            class: class
                .parse()
                .expect("schema CHECK guarantees a valid asset class"),
            weight: BasisPoints(weight),
        },
    ))
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_fund_mix {
    ($tail:literal) => {
        concat!(
            "SELECT asset_class, weight_bp, report_date FROM fund_mix ",
            $tail
        )
    };
}

/// Replace `ticker`'s composition with `slices`, dated `report_date`.
///
/// Deletes the ticker's existing rows and inserts the new ones. **Not**
/// wrapped in its own [`Db::transaction`] -- the same contract
/// `txn::write_transfer` carries for a payday's two legs, so a refresh
/// writing many tickers composes them into one atomic refresh under a single
/// caller-owned transaction. `Db::transaction` is not reentrant, so a caller
/// reachable from inside another one must not open a second.
pub fn set_for_ticker(
    db: &Db,
    ticker: &str,
    report_date: NaiveDate,
    slices: &[Slice],
) -> Result<()> {
    db.conn
        .execute("DELETE FROM fund_mix WHERE ticker = ?1", params![ticker])?;
    for slice in slices {
        db.conn.execute(
            "INSERT INTO fund_mix (ticker, asset_class, weight_bp, report_date) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                ticker,
                slice.class.as_str(),
                slice.weight.0,
                iso(report_date)
            ],
        )?;
    }
    Ok(())
}

/// `ticker`'s composition, or `None` for a ticker nobody has fetched a
/// composition for -- a real state rather than an empty mix, since a mix
/// with no slices and no fetch look identical from the row count alone.
/// `ORDER BY rowid` returns the slices in the order [`set_for_ticker`] wrote
/// them, which is the order the caller supplied.
pub fn for_ticker(db: &Db, ticker: &str) -> Result<Option<Mix>> {
    let mut stmt = db
        .conn
        .prepare(select_fund_mix!("WHERE ticker = ?1 ORDER BY rowid"))?;
    let rows = stmt.query_map(params![ticker], from_row)?;
    let pairs: Vec<(NaiveDate, Slice)> = super::collect_rows(rows)?;
    let Some((report_date, _)) = pairs.first().copied() else {
        return Ok(None);
    };
    Ok(Some(Mix {
        ticker: ticker.to_string(),
        report_date,
        slices: pairs.into_iter().map(|(_, slice)| slice).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn a_mix_read_back_is_the_mix_that_was_written() {
        let db = crate::db::open_in_memory().unwrap();
        let slices = vec![
            Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(6_000),
            },
            Slice {
                class: AssetClass::IntlStock,
                weight: BasisPoints(4_000),
            },
        ];
        set_for_ticker(&db, "USM", day(2026, 6, 30), &slices).unwrap();

        let mix = for_ticker(&db, "USM").unwrap().expect("a mix was written");
        assert_eq!(mix.report_date, day(2026, 6, 30));
        assert_eq!(mix.slices, slices);
    }

    #[test]
    fn writing_a_mix_replaces_the_previous_one_rather_than_adding_to_it() {
        let db = crate::db::open_in_memory().unwrap();
        set_for_ticker(
            &db,
            "USM",
            day(2026, 3, 31),
            &[Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();
        set_for_ticker(
            &db,
            "USM",
            day(2026, 6, 30),
            &[Slice {
                class: AssetClass::UsBond,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();

        let mix = for_ticker(&db, "USM").unwrap().unwrap();
        assert_eq!(
            mix.slices.len(),
            1,
            "the previous quarter's slices survived"
        );
        assert_eq!(mix.slices[0].class, AssetClass::UsBond);
    }

    #[test]
    fn a_ticker_never_fetched_reads_as_none_rather_than_an_empty_mix() {
        let db = crate::db::open_in_memory().unwrap();
        assert!(for_ticker(&db, "USM").unwrap().is_none());
    }

    /// A mix carries no ticker or balance for `set_for_ticker` to clash
    /// over, so two tickers are independent -- writing one must not touch
    /// the other's rows.
    #[test]
    fn a_mix_is_scoped_to_its_own_ticker() {
        let db = crate::db::open_in_memory().unwrap();
        set_for_ticker(
            &db,
            "USM",
            day(2026, 6, 30),
            &[Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();
        set_for_ticker(
            &db,
            "USB",
            day(2026, 6, 30),
            &[Slice {
                class: AssetClass::UsBond,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();

        assert_eq!(
            for_ticker(&db, "USM").unwrap().unwrap().slices[0].class,
            AssetClass::UsStock
        );
        assert_eq!(
            for_ticker(&db, "USB").unwrap().unwrap().slices[0].class,
            AssetClass::UsBond
        );
    }

    #[test]
    fn asset_class_as_str_and_from_str_round_trip() {
        for class in AssetClass::ALL {
            assert_eq!(class.as_str().parse::<AssetClass>().unwrap(), class);
        }
        assert!("stock".parse::<AssetClass>().is_err());
    }

    /// `ALL` is written out by hand, so a variant added to the enum without
    /// being added here would drop out of both this test and the one below,
    /// which is what a fifth-class fund would need to have exercised.
    #[test]
    fn all_covers_every_variant() {
        // The match is exhaustive, so a seventh variant stops this compiling
        // until it is added to `ALL` and counted here too.
        for class in AssetClass::ALL {
            match class {
                AssetClass::UsStock
                | AssetClass::IntlStock
                | AssetClass::UsBond
                | AssetClass::IntlBond
                | AssetClass::Cash
                | AssetClass::Unclassified => {}
            }
        }
        assert_eq!(AssetClass::ALL.len(), 6);
    }

    /// Two modules key a per-class array by [`AssetClass::index`], so a class
    /// whose index did not find it back in `ALL` would put a filing's stock
    /// weight in the bond row with nothing to say so.
    #[test]
    fn every_class_indexes_to_its_own_place_in_all() {
        for (position, class) in AssetClass::ALL.iter().enumerate() {
            assert_eq!(class.index(), position);
        }
    }

    /// The enum and the schema's `CHECK (asset_class IN (...))` are two
    /// independent lists of the same six strings. A variant missing from the
    /// constraint type-checks and then fails at runtime.
    #[test]
    fn every_asset_class_satisfies_the_schema_constraint() {
        let db = crate::db::open_in_memory().unwrap();
        for class in AssetClass::ALL {
            set_for_ticker(
                &db,
                "USM",
                day(2026, 6, 30),
                &[Slice {
                    class,
                    weight: BasisPoints(10_000),
                }],
            )
            .unwrap_or_else(|e| panic!("{class:?} is not in the schema's CHECK list: {e}"));
            assert_eq!(
                for_ticker(&db, "USM").unwrap().unwrap().slices[0].class,
                class
            );
        }
    }
}
