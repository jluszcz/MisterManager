//! Fetches every held ticker's composition from SEC and writes it into
//! `db::fund_mix`.
//!
//! [`classify`] is the whole of the feature's logic and has neither a
//! network nor a database in it -- that is what keeps [`sec`], the network
//! client, thin: a call the classifier never makes is a call it can't get
//! wrong. [`refresh`] is the policy joining the two -- it resolves every
//! ticker to its SEC series once, then fetches, classifies and writes each in
//! turn, sequentially, since SEC's own published limit is 10 requests/second.
//! Every write goes through [`write_outcome`] inside one caller-owned
//! transaction, so a refresh of many tickers is atomic; a single ticker's
//! failure is collected into `Refreshed::failed` rather than propagated, so
//! one throttled ticker never discards the rest, and its previous mix -- if
//! it has one -- is left standing rather than cleared.

mod classify;
pub mod sec;

pub use classify::{FUND_OF_FUNDS_MAX, RawHolding, classify};

use crate::db::fund_mix::Slice;
use crate::db::{self, Db};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use std::collections::HashMap;

/// What one call to [`refresh`] accomplished: the tickers whose composition
/// was written, and the tickers that were not, paired with why.
#[derive(Debug, Default)]
pub struct Refreshed {
    pub updated: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Resolves `tickers` to their SEC series once, then fetches, classifies and
/// writes each in turn.
///
/// The fetch-classify-write loop runs inside one [`Db::transaction`] (opened
/// by [`write_all`]), so a refresh of ten tickers is atomic --
/// `db::fund_mix::set_for_ticker` opens none of its own, which is what lets
/// it compose here. A single ticker's failure -- SEC carries no series for
/// it, the filing fetch throttles, the filing fails to parse -- is collected
/// into [`Refreshed::failed`] by [`write_outcome`] rather than propagated:
/// one bad ticker must not discard the results already staged for the
/// tickers around it in this same transaction.
pub fn refresh(db: &Db, contact: &str, tickers: &[String]) -> Result<Refreshed> {
    let series = sec::resolve_series(contact, tickers)?;
    write_all(db, tickers, |ticker| fetch_ticker(contact, &series, ticker))
}

/// One ticker's fetch and classification, past `resolve_series`'s own map.
///
/// A ticker `resolve_series` returned nothing for is a failure of exactly
/// the same shape as a throttled or malformed filing -- SEC not knowing the
/// ticker is not a database problem -- so it is reported through the same
/// `Result` rather than a separate variant [`write_outcome`] would have to
/// handle twice.
fn fetch_ticker(
    contact: &str,
    series: &HashMap<String, String>,
    ticker: &str,
) -> Result<(NaiveDate, Vec<Slice>)> {
    let series_id = series
        .get(ticker)
        .ok_or_else(|| anyhow!("SEC lists no series for ticker {ticker:?}"))?;
    let filing = sec::latest_filing(contact, series_id)?;
    Ok((filing.report_date, classify(&filing.holdings)))
}

/// The loop [`refresh`] runs, inside the one transaction that makes it
/// atomic -- taking `fetch` as a parameter rather than calling
/// [`fetch_ticker`] directly, which is what lets the transaction-composition
/// and per-ticker-tolerance rules be pinned in `mod tests` with no network
/// at all. In production `fetch` is [`fetch_ticker`] bound to `contact` and
/// the resolved series map; a test hands it a stub.
fn write_all(
    db: &Db,
    tickers: &[String],
    mut fetch: impl FnMut(&str) -> Result<(NaiveDate, Vec<Slice>)>,
) -> Result<Refreshed> {
    db.transaction(|db| {
        let mut refreshed = Refreshed::default();
        for ticker in tickers {
            match write_outcome(db, ticker, fetch(ticker))? {
                Outcome::Updated => refreshed.updated.push(ticker.clone()),
                Outcome::Failed(message) => refreshed.failed.push((ticker.clone(), message)),
            }
        }
        Ok(refreshed)
    })
}

/// One ticker's outcome, past whether its fetch succeeded.
enum Outcome {
    Updated,
    Failed(String),
}

/// Writes `fetched`'s composition for `ticker`, or reports why it could not.
///
/// Never turns a fetch failure into an `Err` of its own -- that is what lets
/// [`write_all`]'s transaction keep going: an `Err` here would abort every
/// write already staged for the tickers ahead of it in the same refresh.
/// What this *does* return an `Err` for is the write itself failing, which
/// is a database problem rather than a network one and belongs on the same
/// footing every other write in this crate stands on. On a fetch failure,
/// nothing is written at all -- `ticker`'s previous mix, if it has one,
/// stands exactly as [`db::fund_mix::set_for_ticker`] last left it.
fn write_outcome(
    db: &Db,
    ticker: &str,
    fetched: Result<(NaiveDate, Vec<Slice>)>,
) -> Result<Outcome> {
    match fetched {
        Ok((report_date, slices)) => {
            db::fund_mix::set_for_ticker(db, ticker, report_date, &slices)?;
            Ok(Outcome::Updated)
        }
        Err(error) => Ok(Outcome::Failed(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fund_mix::AssetClass;
    use crate::rate::BasisPoints;
    use crate::test_support::day;
    use anyhow::anyhow;

    #[test]
    fn a_ticker_that_fails_leaves_its_previous_mix_standing() {
        let db = crate::db::open_in_memory().unwrap();
        db::fund_mix::set_for_ticker(
            &db,
            "USM",
            day(2026, 3, 31),
            &[Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();

        // `write_outcome` is what `refresh` calls per ticker once the network part
        // is done, which is what lets this be tested with no network at all.
        write_outcome(&db, "USM", Err(anyhow!("throttled"))).unwrap();

        let mix = db::fund_mix::for_ticker(&db, "USM")
            .unwrap()
            .expect("the mix survived");
        assert_eq!(mix.report_date, day(2026, 3, 31));
    }

    /// The other half of the rule the mandated test above does not touch: a
    /// failure among *several* tickers refreshed in one call must not roll
    /// back the successes staged for the others in the same transaction.
    /// `write_all` is exercised directly (with a stub `fetch`) since
    /// `refresh` itself always reaches the network.
    #[test]
    fn a_failing_ticker_does_not_roll_back_the_others_in_the_same_refresh() {
        let db = crate::db::open_in_memory().unwrap();
        let tickers: Vec<String> = ["USM", "ISM", "USB"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let refreshed = write_all(&db, &tickers, |ticker| match ticker {
            "USM" => Ok((
                day(2026, 6, 30),
                vec![Slice {
                    class: AssetClass::UsStock,
                    weight: BasisPoints(10_000),
                }],
            )),
            "ISM" => Err(anyhow!("throttled")),
            "USB" => Ok((
                day(2026, 6, 30),
                vec![Slice {
                    class: AssetClass::UsBond,
                    weight: BasisPoints(10_000),
                }],
            )),
            other => panic!("unexpected ticker {other}"),
        })
        .unwrap();

        assert_eq!(
            refreshed.updated,
            vec!["USM".to_string(), "USB".to_string()]
        );
        assert_eq!(refreshed.failed.len(), 1);
        assert_eq!(refreshed.failed[0].0, "ISM");
        assert_eq!(refreshed.failed[0].1, "throttled");

        assert!(
            db::fund_mix::for_ticker(&db, "USM").unwrap().is_some(),
            "a sibling failure rolled back USM's write"
        );
        assert!(
            db::fund_mix::for_ticker(&db, "USB").unwrap().is_some(),
            "a sibling failure rolled back USB's write"
        );
        assert!(db::fund_mix::for_ticker(&db, "ISM").unwrap().is_none());
    }
}
