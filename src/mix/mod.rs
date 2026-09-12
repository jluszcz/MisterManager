//! Fetches every held ticker's composition from SEC and writes it into
//! `db::fund_mix`.
//!
//! [`classify`] is the whole of the feature's logic and has neither a
//! network nor a database in it -- that is what keeps [`sec`], the network
//! client, thin: a call the classifier never makes is a call it can't get
//! wrong. [`refresh`] is the policy joining the two, in two phases:
//! [`fetch_and_write`] first fetches and classifies every ticker,
//! sequentially (SEC's own published limit is 10 requests/second), with no
//! transaction open at all -- only once every network call has returned does
//! it open the one transaction that writes every result through
//! [`write_outcome`]. That ordering is the point: a SQLite write lock held
//! across a whole batch of blocking HTTP requests (throttle backoff
//! included) would serialize against nothing here, only cost something. A
//! single ticker's failure -- SEC carries no series for it, the filing fetch
//! throttles, the filing fails to parse -- is collected into
//! `Refreshed::failed` rather than propagated, so one throttled ticker never
//! discards the rest and its previous mix -- if it has one -- is left
//! standing rather than cleared; a genuine write failure, by contrast, rolls
//! back every write already staged for the tickers ahead of it in the same
//! refresh, which is what makes the batch atomic.

mod classify;
pub mod sec;

pub use classify::{FUND_OF_FUNDS_MAX, RawHolding, classify};

use crate::db::fund_mix::Slice;
use crate::db::{self, Db};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use std::collections::HashMap;

/// One ticker's fetch-and-classify result, past the network -- a fund's
/// as-of date, its own name, and its slices, or why fetching it failed.
/// Named so [`fetch_and_write`] does not have to spell the nested `Result`
/// clippy flags as too complex to read inline.
type Fetched = Result<Composition>;

/// What one filing yields: everything [`db::fund_mix::set_for_ticker`] writes
/// but the ticker, which the caller already holds.
///
/// A struct rather than a third element in a tuple: three of them is where a
/// caller starts having to count positions to know which date is which.
#[derive(Debug)]
struct Composition {
    report_date: NaiveDate,
    /// The fund's own name, absent where a filing carried none -- see
    /// [`sec::Filing::series_name`] for why that is not a refusal.
    name: Option<String>,
    slices: Vec<Slice>,
}

/// What one call to [`refresh`] accomplished: the tickers whose composition
/// was written, and the tickers that were not, paired with why.
#[derive(Debug, Default)]
pub struct Refreshed {
    pub updated: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Resolves `tickers` to their SEC series once, then fetches, classifies and
/// writes each in turn. See [`fetch_and_write`] for the two-phase shape and
/// why fetching runs before any transaction opens.
///
/// Nothing asked for is answered without touching SEC or the database at
/// all. `resolve_series` downloads 1.2 MB whatever it is handed, and it is
/// the *first* hop, so an empty list would pay for the whole of it and then
/// open a transaction to write nothing -- which is what `G` on a Funds screen
/// with no holdings and `mm mixes` against a database with none both ask for.
pub fn refresh(db: &Db, contact: &str, tickers: &[String]) -> Result<Refreshed> {
    if tickers.is_empty() {
        return Ok(Refreshed::default());
    }
    let series = sec::resolve_series(contact, tickers)?;
    fetch_and_write(db, tickers, |ticker| fetch_ticker(contact, &series, ticker))
}

/// One ticker's fetch and classification, past `resolve_series`'s own map.
///
/// A ticker `resolve_series` returned nothing for is a failure of exactly
/// the same shape as a throttled or malformed filing -- SEC not knowing the
/// ticker is not a database problem -- so it is reported through the same
/// `Result` rather than a separate variant [`write_outcome`] would have to
/// handle twice.
///
/// It is also the one failure here that names a ticker, and the Funds screen
/// puts the whole sentence on its status line beside the ticker it belongs
/// to -- so it reaches the mask where it is built, the way `db::holding`'s
/// own refusals do. Masked at the screen instead, the label would be a
/// pseudonym beside the real ticker in the same line, which says more than
/// either half alone.
fn fetch_ticker(contact: &str, series: &HashMap<String, String>, ticker: &str) -> Fetched {
    let series_id = series.get(ticker).ok_or_else(|| {
        anyhow!(
            "SEC lists no series for ticker {:?}",
            crate::demo::text(ticker)
        )
    })?;
    let filing = sec::latest_filing(contact, series_id)?;
    Ok(Composition {
        report_date: filing.report_date,
        name: filing.series_name,
        slices: classify(&filing.holdings),
    })
}

/// Fetches every ticker, then writes every result -- in that order, with no
/// transaction open across the first phase.
///
/// `fetch` runs once per ticker, sequentially, *before* [`Db::transaction`]
/// is ever called: it is a parameter rather than a call to [`fetch_ticker`]
/// so that phase, and the write phase after it, can each be pinned in `mod
/// tests` with a stub and no network at all. Only once every fetch has
/// returned does the one transaction open, looping over the already-fetched
/// results and writing each through [`write_outcome`] --
/// `db::fund_mix::set_for_ticker` opens none of its own, which is what lets
/// it compose here. Shaping it this way rather than fetching and writing one
/// ticker at a time inside the transaction is what keeps a SQLite write lock
/// from spanning a whole batch of blocking HTTP requests, throttle backoff
/// included, for no reason a single-writer local database needs.
///
/// A single ticker's failure -- SEC carries no series for it, the filing
/// fetch throttles, the filing fails to parse -- is collected into
/// [`Refreshed::failed`] rather than propagated: one bad ticker must not
/// discard the writes already staged for the tickers ahead of it in this
/// same transaction. A *write* failure is the opposite case: it is not
/// something any ticker's fetch result can cause on its own, but it is
/// reachable (two slices naming the same asset class for one ticker
/// violates `fund_mix`'s own `PRIMARY KEY`), and when it happens it
/// propagates out of the closure and rolls back every write already
/// committed to this transaction -- which is the whole point of running the
/// write phase as one transaction rather than one per ticker.
fn fetch_and_write(
    db: &Db,
    tickers: &[String],
    mut fetch: impl FnMut(&str) -> Fetched,
) -> Result<Refreshed> {
    let fetched: Vec<(String, Fetched)> = tickers
        .iter()
        .map(|ticker| (ticker.clone(), fetch(ticker)))
        .collect();

    db.transaction(|db| {
        let mut refreshed = Refreshed::default();
        for (ticker, outcome) in fetched {
            match write_outcome(db, &ticker, outcome)? {
                Outcome::Updated => refreshed.updated.push(ticker),
                Outcome::Failed(message) => refreshed.failed.push((ticker, message)),
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
/// [`fetch_and_write`]'s transaction keep going: an `Err` here would abort
/// every write already staged for the tickers ahead of it in the same
/// refresh.
/// What this *does* return an `Err` for is the write itself failing, which
/// is a database problem rather than a network one and belongs on the same
/// footing every other write in this crate stands on. On a fetch failure,
/// nothing is written at all -- `ticker`'s previous mix, if it has one,
/// stands exactly as [`db::fund_mix::set_for_ticker`] last left it.
fn write_outcome(db: &Db, ticker: &str, fetched: Fetched) -> Result<Outcome> {
    match fetched {
        Ok(composition) => {
            db::fund_mix::set_for_ticker(
                db,
                ticker,
                composition.report_date,
                composition.name.as_deref(),
                &composition.slices,
            )?;
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

    /// One fund's whole composition, named out of the fixture vocabulary --
    /// what a stubbed `fetch` hands back where the real one hands back a
    /// filing.
    fn composition(ticker: &str, slice: Slice) -> Composition {
        Composition {
            report_date: day(2026, 6, 30),
            name: Some(crate::test_support::fund_name(ticker).to_string()),
            slices: vec![slice],
        }
    }

    /// `resolve_series` is the first hop and downloads 1.2 MB whatever it is
    /// handed, so a refresh with nothing to refresh has to answer before it.
    /// A network call here would fail the test rather than return, since
    /// nothing in the suite reaches SEC -- but the point stands either way:
    /// the contact is deliberately nonsense.
    #[test]
    fn a_refresh_of_no_tickers_answers_without_reaching_sec() {
        let db = crate::db::open_in_memory().unwrap();
        let refreshed = refresh(&db, "nobody@example.com", &[]).unwrap();
        assert!(refreshed.updated.is_empty());
        assert!(refreshed.failed.is_empty());
    }

    /// The Funds screen prints this sentence on its status line next to the
    /// ticker it belongs to, and that label is masked -- so an unmasked
    /// ticker in the sentence would sit beside its own pseudonym.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_masks_the_ticker_the_unresolved_series_failure_names() {
        crate::demo::install_with_salt(7);
        let error = fetch_ticker("nobody@example.com", &HashMap::new(), "USM").unwrap_err();
        let message = error.to_string();

        assert!(!message.contains("USM"), "the ticker survived: {message}");
        assert!(
            message.contains(&crate::demo::text("USM").to_string()),
            "no masked ticker found: {message}"
        );
    }

    #[test]
    fn a_ticker_that_fails_leaves_its_previous_mix_standing() {
        let db = crate::db::open_in_memory().unwrap();
        db::fund_mix::set_for_ticker(
            &db,
            "USM",
            day(2026, 3, 31),
            Some(crate::test_support::fund_name("USM")),
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
    /// *tolerated* failure among several tickers refreshed in one call must
    /// not roll back the successes staged for the others in the same
    /// transaction. `fetch_and_write` is exercised directly (with a stub
    /// `fetch`) since `refresh` itself always reaches the network.
    #[test]
    fn a_failing_ticker_does_not_roll_back_the_others_in_the_same_refresh() {
        let db = crate::db::open_in_memory().unwrap();
        let tickers: Vec<String> = ["USM", "ISM", "USB"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let refreshed = fetch_and_write(&db, &tickers, |ticker| match ticker {
            "USM" => Ok(composition(
                "USM",
                Slice {
                    class: AssetClass::UsStock,
                    weight: BasisPoints(10_000),
                },
            )),
            "ISM" => Err(anyhow!("throttled")),
            "USB" => Ok(composition(
                "USB",
                Slice {
                    class: AssetClass::UsBond,
                    weight: BasisPoints(10_000),
                },
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

    /// The genuine-failure half neither test above touches: a *write*
    /// failure -- not a tolerated fetch failure -- must roll back every
    /// write already staged for the tickers ahead of it in the same
    /// transaction. Two slices naming the same asset class for one ticker
    /// violate `fund_mix`'s own `PRIMARY KEY (ticker, asset_class)`, which is
    /// a real constraint violation reachable through `mix`'s own public
    /// types with no `rusqlite` anywhere in this module.
    ///
    /// USM is seeded with a pre-existing mix and then handed a *different*
    /// one to write, so "rolled back" is distinguishable from "never
    /// attempted": if the rollback did not happen, USM would carry its new
    /// slices (written before ISM's constraint violation was hit) rather
    /// than the old ones it was seeded with.
    #[test]
    fn a_genuine_write_failure_rolls_back_every_ticker_already_written_in_the_refresh() {
        let db = crate::db::open_in_memory().unwrap();
        db::fund_mix::set_for_ticker(
            &db,
            "USM",
            day(2026, 3, 31),
            Some(crate::test_support::fund_name("USM")),
            &[Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();

        let tickers: Vec<String> = ["USM", "ISM"].iter().map(|s| s.to_string()).collect();

        let result = fetch_and_write(&db, &tickers, |ticker| match ticker {
            "USM" => Ok(composition(
                "USM",
                Slice {
                    class: AssetClass::UsBond,
                    weight: BasisPoints(10_000),
                },
            )),
            // Two slices of one class: the duplicate `PRIMARY KEY` the write
            // is expected to refuse.
            "ISM" => Ok(Composition {
                slices: vec![
                    Slice {
                        class: AssetClass::UsStock,
                        weight: BasisPoints(5_000),
                    },
                    Slice {
                        class: AssetClass::UsStock,
                        weight: BasisPoints(5_000),
                    },
                ],
                ..composition(
                    "ISM",
                    Slice {
                        class: AssetClass::UsStock,
                        weight: BasisPoints(10_000),
                    },
                )
            }),
            other => panic!("unexpected ticker {other}"),
        });

        assert!(
            result.is_err(),
            "a duplicate asset-class slice did not surface as a write error"
        );

        let mix = db::fund_mix::for_ticker(&db, "USM")
            .unwrap()
            .expect("USM's mix vanished rather than rolling back to what it had");
        assert_eq!(
            mix.report_date,
            day(2026, 3, 31),
            "USM's new write survived even though ISM's failed afterwards"
        );
        assert_eq!(mix.slices[0].class, AssetClass::UsStock);
        assert!(
            db::fund_mix::for_ticker(&db, "ISM").unwrap().is_none(),
            "ISM's failed write left something behind"
        );
    }
}
