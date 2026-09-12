//! The one test that catches SEC changing under the fetcher.
//!
//! Every other test of `src/mix/` runs against a fixture: a recorded filing,
//! a hand-written ticker file, a stubbed fetch. None of them can notice the
//! day EDGAR moves a URL, renames a field, or answers a hop in a different
//! shape -- the feature would go on passing and start returning nothing, one
//! quarter after the last time anyone looked. This binary is the only thing
//! that makes that failure loud, and it earns that by actually reaching the
//! network.
//!
//! **It asserts structurally and never a figure.** A fund's composition
//! changes every quarter and its holdings change every day, so a golden
//! weight here would rot on a schedule rather than report a defect: what is
//! pinned is that the three hops resolve to each other, that a filing comes
//! back with holdings in it, that the classified weights foot to
//! `BasisPoints::ONE`, and that something landed in a class the age rule can
//! read.
//!
//! `MM_SEC_TICKER` names the fund, and there is deliberately no default:
//! which funds the owner holds is the same kind of fact as an account code,
//! and a ticker written down here would name one in a public repository. The
//! contact comes from the ordinary config file, for the same reason `mm
//! mixes` takes it from there -- SEC refuses a request declaring none, and a
//! real address may not be a literal in any tracked file.
//!
//! Unset, this skips loudly, so a clean checkout passes;
//! `MM_REQUIRE_SEC=1` turns the skip into a failure, which is what a run
//! meant to exercise the fetcher passes:
//!
//! ```sh
//! MM_REQUIRE_SEC=1 MM_SEC_TICKER=<ticker> cargo test --test sec_live
//! ```
//!
//! `--test sec_live` rather than a bare filter, since the test below is named
//! for what it asserts rather than for the binary it sits in: `cargo test
//! sec_live` matches no test name and exits green having run nothing.
//!
//! Not behind `--features import`, unlike every workbook-oracle binary beside
//! it: what it exercises is in an ordinary build, and gating it would leave
//! this binary compiling to nothing.
//!
//! **If this fails with a 403 rather than a parse error, read
//! `src/mix/CLAUDE.md`'s note on the throttle marker first.** The text
//! `src/mix/sec.rs` tells SEC's own throttle apart by is the one thing in
//! the module no fixture can confirm, and a marker that never matches turns
//! a backoff into an immediate failure.

use mistermanager::db::fund_mix::AssetClass;
use mistermanager::rate::BasisPoints;
use mistermanager::{config, mix};

/// Report a fixture the run does not have: a hard failure under
/// `MM_REQUIRE_SEC=1`, and otherwise a line on stderr so a skipped run is
/// never a silent one. `tests/common/mod.rs`'s shape, spelled again rather
/// than shared: that module reaches the importer, so a binary outside
/// `--features import` cannot compile it.
fn missing(what: &str) {
    if std::env::var("MM_REQUIRE_SEC").as_deref() == Ok("1") {
        panic!("MM_REQUIRE_SEC=1 but {what}");
    }
    eprintln!("skipping: {what} (set MM_REQUIRE_SEC=1 to fail instead of skip)");
}

/// The fund to ask SEC about, from `MM_SEC_TICKER`.
fn ticker() -> Option<String> {
    match std::env::var("MM_SEC_TICKER") {
        Ok(ticker) if !ticker.trim().is_empty() => Some(ticker.trim().to_string()),
        _ => {
            missing("MM_SEC_TICKER is not set (a ticker whose fund files an N-PORT)");
            None
        }
    }
}

/// The contact every request declares itself with, read from the config file
/// `mm mixes` reads it from -- an absent file parses as a configuration with
/// no `[sec]` section, which is the state a clean checkout is in.
fn contact() -> Option<String> {
    let path = config::default_path().expect("a config path");
    let config = match config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            missing(&format!("{} does not parse: {error}", path.display()));
            return None;
        }
    };
    match config.sec {
        Some(sec) => Some(sec.contact),
        None => {
            missing(&format!(
                "{} carries no [sec] contact, which SEC requires of every request",
                path.display()
            ));
            None
        }
    }
}

#[test]
fn the_three_hops_resolve_and_the_weights_foot() {
    let (Some(ticker), Some(contact)) = (ticker(), contact()) else {
        return;
    };

    let series = mix::sec::resolve_series(&contact, std::slice::from_ref(&ticker)).unwrap();
    let series_id = series
        .get(&ticker)
        .unwrap_or_else(|| panic!("SEC lists no series for {ticker:?}"));

    let filing = mix::sec::latest_filing(&contact, series_id).unwrap();
    assert!(
        !filing.holdings.is_empty(),
        "the filing carried no holdings"
    );
    // `latest_filing` is what parses `repPdDate`, so reaching here is the
    // parse. What this adds is that the date read is a *period end*: a
    // filing reports as of a date that has already happened, so a future one
    // means some other element's text was read into the field.
    assert!(
        filing.report_date <= chrono::Local::now().date_naive(),
        "the filing reports as of {}, which has not happened",
        filing.report_date
    );

    let slices = mix::classify(&filing.holdings);
    let total: i64 = slices.iter().map(|s| s.weight.0).sum();
    assert_eq!(
        total,
        BasisPoints::ONE.0,
        "the classified weights do not foot"
    );
    assert!(
        slices
            .iter()
            .any(|s| s.weight > BasisPoints::ZERO && s.class != AssetClass::Unclassified),
        "every slice landed in Unclassified, so the classifier matched nothing in {ticker}'s filing"
    );
}
