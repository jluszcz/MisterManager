# Fund Look-Through, Part 2: The SEC Fetcher, the Classifier and the Report Tab

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill `fund_mix` from SEC N-PORT filings, so the owner types balances and never types a
fund's internal composition.

**Architecture:** Eight tasks. The pure classifier is built and fully tested first (task 2) so the
network module above it stays thin; the network module follows (task 3), then the policy that joins
them (task 4), the command and key that press it (task 5), the screen that draws it (task 6), the
report tab (task 7), and documentation (task 8).

**Tech Stack:** Rust 2024, `jluszcz_rust_utils` (feature `query`) over `reqwest` 0.13, `quick-xml`
for streaming N-PORT, `tokio` current-thread runtime, `rusqlite`, `ratatui`.

**Spec:** `docs/superpowers/specs/2026-09-11-fund-allocation-lookthrough-design.md`

**Depends on:** `docs/superpowers/plans/2026-09-11-fund-lookthrough-model.md`, complete. This plan
assumes `db::fund_mix`, `db::holding`, `account::Kind::Investment` and the fund vocabulary in
`src/test_support.rs` all exist.

**Optional companion:** the rust-utils TLS-provider change, planned in that repository rather
than this one. Until it lands, `features = ["query"]` builds and works — it just carries `aws-lc-sys` as well as the `ring`
already present via `aws-sdk-s3`. Task 1 says what to do in either case.

## Global Constraints

- **No real data in the repository.** No ticker the owner holds, no fund family, no balance — in
  source, tests, fixtures, docs, commit messages or PR text. The XML fixtures this plan adds are
  written in the invented fund vocabulary from the model plan's task 6.
- **`rusqlite` is named only inside `src/db/`.** `ratatui`/`crossterm` only inside `src/tui/`.
  **`reqwest`, `tokio` and `jluszcz_rust_utils` are named only inside `src/mix/sec.rs`** — the
  second such seam in the crate, beside `src/backup/s3.rs`.
- **Never write a real email address into a file.** The SEC contact is read from config at runtime.
- **Never disable a failing test; fix it.**
- **Test names are full sentences.** Unit tests in `mod tests` at the bottom of the file under test.
- **Documentation describes the code as it is, not how it got there.**
- **Never commit directly to the default branch.**
- **Commit with the `jluszcz:commit` skill.**
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LPunW61FrbUCTazxaNj23Y
  ```

### Commands

```bash
cargo test
cargo test --lib
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings

# The live SEC test, task 8. Skips loudly when unset.
MM_REQUIRE_SEC=1 MM_SEC_TICKER=<ticker> cargo test sec_live
```

`MM_SEC_TICKER` names a fund the owner holds and **must not be written into any file**, for the
reason `MM_WORKBOOK` is not.

---

## Facts verified against live filings

An implementer should not re-derive these; they were measured, and the tests below encode them.

- **Ticker → series:** `https://www.sec.gov/files/company_tickers_mf.json` (~1.2 MB) has fields
  `["cik", "seriesId", "classId", "symbol"]`. Two share classes of one fund share a `seriesId`.
- **Series → latest filing:** `https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK=<seriesId>&type=NPORT-P&count=1&output=atom`.
  **The series ID goes in the `CIK` slot** — that is what yields a per-fund history rather than the
  whole trust's. The response is Atom; `<filing-href>` gives an `-index.htm` URL whose directory
  holds `primary_doc.xml`.
- **Holdings** live in `<invstOrSec>` elements. Relevant children: `name`, `title`, `cusip`,
  `pctVal`, `assetCat`, `invCountry`. `genInfo/repPdDate` is the as-of date.
- **A fund-of-funds filing is small.** Measured: 7 and 5 holdings for two target-date funds against
  3,546 / 8,878 / 17,409 for three direct index funds. The threshold is 25 and it is not a
  knife-edge.
- **`assetCat` does not classify an underlying fund.** Every fund-of-funds holding reads `EC`, and
  `invCountry` reads `US` for all of them — the fund is US-domiciled, not its holdings.
- **Direct funds need the full `assetCat` vocabulary.** `ABS-MBS` was 20.5% of one measured bond
  fund. `EP`, `DE`, `DFE`, `ABS-CBDO` also appear, at under 1%.
- **A 3.2–20.6 MB filing is normal** for a direct fund, so the XML is parsed as a stream, not into a
  DOM.

---

## Task 1: The dependency and the crypto provider

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`

**Interfaces:**
- Produces: `config::Sec { pub contact: String }` and `Config::sec: Option<Sec>`.

- [ ] **Step 1: Add the dependencies**

```toml
jluszcz_rust_utils = { git = "https://github.com/jluszcz/rust-utils", features = ["query"] }
quick-xml = "0.38"
```

**If the rust-utils plan has landed**, take `features = ["query", "tls-ring"]` instead, and nothing
else in this step changes. **If it has not**, `query` brings `aws-lc-sys` and reqwest installs its
own provider, so no install call is needed — note which case applies in the commit body so the
follow-up is not lost.

`tokio` already has `rt`, `net` and `time`; add `macros` only if a test needs it.

**Never call `jluszcz_rust_utils::set_up_logger`.** `fern` and `log` are unconditional dependencies
of that crate, so they arrive with `query`; a logger writing to stderr would corrupt the TUI
mid-frame. Nothing in this crate installs one, and nothing should.

- [ ] **Step 2: Write the failing test**

In `src/config.rs`'s `mod tests`:

```rust
#[test]
fn a_config_with_no_sec_section_parses_and_reports_no_contact() {
    let config: Config = toml::from_str("").unwrap();
    assert!(config.sec.is_none());
}

#[test]
fn the_sec_contact_is_read_from_its_own_section() {
    let config: Config =
        toml::from_str("[sec]\ncontact = \"someone@example.com\"").unwrap();
    assert_eq!(config.sec.unwrap().contact, "someone@example.com");
}
```

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo test --lib the_sec_contact_is_read`
Expected: FAIL — `no field 'sec' on type 'Config'`.

- [ ] **Step 4: Add the section**

In `src/config.rs`, beside `backup` and `report`:

```rust
/// What `mm mixes` puts in its `User-Agent`.
///
/// SEC refuses a request that declares no contact, so this is required rather
/// than defaulted -- and it is a setting rather than a constant because no
/// real address may be written into a file in this repository.
#[derive(Debug, Deserialize)]
pub struct Sec {
    pub contact: String,
}
```

with `pub sec: Option<Sec>` on `Config`.

- [ ] **Step 5: Verify the build and the resolution**

```bash
cargo build --all-features
cargo tree -i ring | head
cargo test --lib config::
```

Expected: build succeeds, tests pass.

- [ ] **Step 6: Commit**

Commit with `jluszcz:commit`. Message: `feat(config): a SEC contact for the fetcher's User-Agent`

---

## Task 2: The classifier

The whole of this feature's logic, with no network and no database in it.

**Files:**
- Create: `src/mix/mod.rs`, `src/mix/classify.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `db::fund_mix::AssetClass` from the model plan.
- Produces:
  ```rust
  pub struct RawHolding {
      pub name: String,
      pub title: String,
      pub cusip: String,
      pub pct_val: f64,
      pub asset_cat: String,
      pub inv_country: String,
  }
  pub const FUND_OF_FUNDS_MAX: usize = 25;
  pub fn classify(holdings: &[RawHolding]) -> Vec<db::fund_mix::Slice>;
  ```
  `src/mix/mod.rs` re-exports both — `pub use classify::{RawHolding, classify};` — so callers
  write `mix::classify(..)` rather than `mix::classify::classify(..)`. Task 8's live test and
  task 4's policy both depend on that spelling.
  `classify` returns slices summing to exactly `BasisPoints(10_000)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fund_mix::AssetClass;
    use crate::rate::BasisPoints;

    /// A holding of an underlying fund: `assetCat` is `EC` and `invCountry` is
    /// `US` whatever the fund holds, which is why the name is what classifies.
    fn fund(name: &str, title: &str, pct: f64) -> RawHolding {
        RawHolding {
            name: name.into(),
            title: title.into(),
            cusip: "000000000".into(),
            pct_val: pct,
            asset_cat: "EC".into(),
            inv_country: "US".into(),
        }
    }

    fn security(asset_cat: &str, country: &str, pct: f64) -> RawHolding {
        RawHolding {
            name: "A Held Company".into(),
            title: String::new(),
            cusip: "000000000".into(),
            pct_val: pct,
            asset_cat: asset_cat.into(),
            inv_country: country.into(),
        }
    }

    fn weight(slices: &[crate::db::fund_mix::Slice], class: AssetClass) -> BasisPoints {
        slices
            .iter()
            .find(|s| s.class == class)
            .map(|s| s.weight)
            .unwrap_or(BasisPoints::ZERO)
    }

    #[test]
    fn a_fund_of_funds_is_classified_by_the_names_of_the_funds_it_holds() {
        let slices = classify(&[
            fund("Total Market Index Fund", "", 50.0),
            fund("International Stock Index Fund", "", 30.0),
            fund("Total Bond Index Fund", "", 15.0),
            fund("International Bond Index Fund", "", 5.0),
        ]);

        assert_eq!(weight(&slices, AssetClass::UsStock), BasisPoints(5_000));
        assert_eq!(weight(&slices, AssetClass::IntlStock), BasisPoints(3_000));
        assert_eq!(weight(&slices, AssetClass::UsBond), BasisPoints(1_500));
        assert_eq!(weight(&slices, AssetClass::IntlBond), BasisPoints(500));
    }

    /// One family files the readable name as `name` and another as `title`, so
    /// both are read and neither alone is enough.
    #[test]
    fn a_readable_name_in_either_field_classifies_the_holding() {
        let by_title = classify(&[fund("Some Street Trust", "Total Bond Index Fund", 100.0)]);
        assert_eq!(weight(&by_title, AssetClass::UsBond), BasisPoints(10_000));

        let by_name = classify(&[fund("Total Bond Index Fund", "TB II-INV", 100.0)]);
        assert_eq!(weight(&by_name, AssetClass::UsBond), BasisPoints(10_000));
    }

    #[test]
    fn a_name_naming_no_asset_class_lands_in_unclassified() {
        let slices = classify(&[fund("Overseas Growth Fund", "", 100.0)]);
        assert_eq!(weight(&slices, AssetClass::Unclassified), BasisPoints(10_000));
    }

    /// A direct fund holds securities, and a fund share's `EC` would call the
    /// whole thing stock. Over the threshold, `assetCat` classifies instead.
    #[test]
    fn a_direct_fund_is_classified_by_asset_category_and_country() {
        let mut holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX + 1)
            .map(|_| security("EC", "US", 50.0 / (FUND_OF_FUNDS_MAX + 1) as f64))
            .collect();
        holdings.push(security("DBT", "DE", 50.0));

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::UsStock), BasisPoints(5_000));
        assert_eq!(weight(&slices, AssetClass::IntlBond), BasisPoints(5_000));
    }

    /// Mortgage-backed paper was a fifth of one measured bond fund. Dropping it
    /// would quietly lose that fifth.
    #[test]
    fn mortgage_backed_paper_counts_as_a_bond() {
        let mut holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX + 1)
            .map(|_| security("ABS-MBS", "US", 100.0 / (FUND_OF_FUNDS_MAX + 1) as f64))
            .collect();
        holdings.truncate(FUND_OF_FUNDS_MAX + 1);

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::UsBond), BasisPoints(10_000));
    }

    #[test]
    fn an_unknown_asset_category_lands_in_unclassified_rather_than_a_neighbour() {
        let mut holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX)
            .map(|_| security("EC", "US", 90.0 / FUND_OF_FUNDS_MAX as f64))
            .collect();
        holdings.push(security("WAT", "US", 10.0));

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::Unclassified), BasisPoints(1_000));
    }

    #[test]
    fn the_slices_always_foot_to_one_hundred_percent() {
        // Three thirds do not divide 10,000 evenly; the remainder goes to the
        // largest bucket rather than leaving the row short.
        let slices = classify(&[
            fund("Total Market Index Fund", "", 33.333),
            fund("International Stock Index Fund", "", 33.333),
            fund("Total Bond Index Fund", "", 33.334),
        ]);

        let total: i64 = slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, 10_000, "the slices did not foot to 100%");
    }

    #[test]
    fn a_filing_with_no_holdings_foots_to_unclassified_rather_than_panicking() {
        let slices = classify(&[]);
        assert_eq!(weight(&slices, AssetClass::Unclassified), BasisPoints(10_000));
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib mix::classify`
Expected: FAIL — `file not found for module mix`.

- [ ] **Step 3: Write the classifier**

`src/mix/classify.rs`. The keyword lists are generic vocabulary and **name no institution** — that
is what lets them live in a tracked file at all. Order matters: cash is tested first, then bond
against international, because "International Bond Index Fund" matches both and the bond reading is
the right one.

```rust
const INTL: [&str; 7] = [
    "international", "intl", "ex u.s.", "ex-u.s.", "global ex",
    "developed markets", "emerging markets",
];
const BOND: [&str; 3] = ["bond", "treasury", "fixed income"];
const CASH: [&str; 4] = ["liquidity", "money market", "short-term reserve", "cash"];
const STOCK: [&str; 3] = ["index", "stock", "market"];
```

`FUND_OF_FUNDS_MAX` carries its own derivation as a doc comment: the measured separation, and that
what it distinguishes is a fund holding a handful of other funds from a diversified index fund.

Normalization: accumulate each class in hundredths of a basis point to keep the rounding honest,
convert once at the end, then add `10_000 - total` to the largest slice. Say in a comment why the
largest: a remainder spread across slices moves several figures to fix one.

- [ ] **Step 4: Run them to make sure they pass**

```bash
cargo test --lib mix::classify
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

Commit with `jluszcz:commit`. Message: `feat(mix): classify a fund's holdings into asset classes`

---

## Task 3: The SEC client

**Files:**
- Create: `src/mix/sec.rs`
- Create: `tests/fixtures/nport_fund_of_funds.xml`, `tests/fixtures/nport_direct.xml`

**Interfaces:**
- Consumes: `mix::classify::RawHolding` from task 2.
- Produces:
  ```rust
  pub struct Filing { pub report_date: NaiveDate, pub holdings: Vec<RawHolding> }
  pub fn resolve_series(contact: &str, tickers: &[String]) -> Result<HashMap<String, String>>;
  pub fn latest_filing(contact: &str, series_id: &str) -> Result<Filing>;
  pub fn parse_filing(xml: &[u8]) -> Result<Filing>;   // pure, and where the tests live
  ```

**This is the only file in the crate that may name `reqwest`, `tokio` or `jluszcz_rust_utils`.**
Open a current-thread runtime and `block_on`, exactly as `src/backup/s3.rs:26` does; read that file
before writing this one.

- [ ] **Step 1: Write the fixtures**

Two small N-PORT documents in the **invented fund vocabulary** from the model plan — `TDF45` holding
`USM`/`ISM`/`USB`/`ISB`, and a direct fund holding 30 securities. Real element names and nesting,
invented contents. Copy the element structure from the "Facts verified" section above; do not
download a real filing into the repository.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn a_filings_report_date_is_read_from_its_general_information() {
        let filing = parse_filing(include_bytes!(
            "../../tests/fixtures/nport_fund_of_funds.xml"
        ))
        .unwrap();
        assert_eq!(filing.report_date, day(2026, 6, 30));
    }

    #[test]
    fn every_holding_in_a_filing_is_read_with_its_percentage() {
        let filing = parse_filing(include_bytes!(
            "../../tests/fixtures/nport_fund_of_funds.xml"
        ))
        .unwrap();

        assert_eq!(filing.holdings.len(), 4);
        let total: f64 = filing.holdings.iter().map(|h| h.pct_val).sum();
        assert!((total - 100.0).abs() < 0.01, "the fixture's holdings do not foot");
    }

    #[test]
    fn a_direct_funds_securities_are_read_with_their_categories_and_countries() {
        let filing =
            parse_filing(include_bytes!("../../tests/fixtures/nport_direct.xml")).unwrap();

        assert_eq!(filing.holdings.len(), 30);
        assert!(filing.holdings.iter().any(|h| h.inv_country != "US"));
    }

    #[test]
    fn a_document_that_is_not_a_filing_is_an_error_naming_what_was_missing() {
        let err = parse_filing(b"<nonsense/>").unwrap_err();
        assert!(
            err.to_string().contains("repPdDate"),
            "the error does not name the missing element: {err}"
        );
    }
}
```

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo test --lib mix::sec`
Expected: FAIL — `file not found for module sec`.

- [ ] **Step 4: Write the parser**

`parse_filing` uses `quick_xml::Reader` as a stream — a 20 MB filing must not become a DOM. It is
pure and takes bytes, which is what makes the four tests above possible with no network.

- [ ] **Step 5: Write the two network functions**

`resolve_series` fetches `company_tickers_mf.json`, keeps only the rows whose `symbol` matches a
requested ticker, and drops the rest — caching thirty thousand funds to look up ten is the wrong
trade. Say that in a comment.

`latest_filing` fetches the Atom feed, extracts `<filing-href>`, takes its directory, appends
`primary_doc.xml`, fetches, and calls `parse_filing`.

Both build a `User-Agent` from `contact`. Requests go through `jluszcz_rust_utils::query::send`.
Wrap it so a 403 carrying SEC's throttle text is retried and a plain 403 is not — `query::send`
handles 5xx and 429 but not this.

Requests run **sequentially**. SEC's limit is 10/second and there is nothing to gain by approaching
it.

- [ ] **Step 6: Run the tests**

```bash
cargo test --lib mix::sec
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

Commit with `jluszcz:commit`. Message: `feat(mix): read a fund's latest N-PORT filing from SEC`

---

## Task 4: The refresh policy

**Files:**
- Modify: `src/mix/mod.rs`

**Interfaces:**
- Consumes: tasks 2 and 3.
- Produces:
  ```rust
  pub struct Refreshed { pub updated: Vec<String>, pub failed: Vec<(String, String)> }
  pub fn refresh(db: &Db, contact: &str, tickers: &[String]) -> Result<Refreshed>;
  pub fn tickers_held(db: &Db) -> Result<Vec<String>>;
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_ticker_that_fails_leaves_its_previous_mix_standing() {
    let db = crate::db::open_in_memory().unwrap();
    db::fund_mix::set_for_ticker(
        &db, "USM", day(2026, 3, 31),
        &[Slice { class: AssetClass::UsStock, weight: BasisPoints(10_000) }],
    )
    .unwrap();

    // `write_outcome` is what `refresh` calls per ticker once the network part
    // is done, which is what lets this be tested with no network at all.
    write_outcome(&db, "USM", Err(anyhow!("throttled"))).unwrap();

    let mix = db::fund_mix::for_ticker(&db, "USM").unwrap().expect("the mix survived");
    assert_eq!(mix.report_date, day(2026, 3, 31));
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test --lib a_ticker_that_fails_leaves`
Expected: FAIL — no function `write_outcome`.

- [ ] **Step 3: Write the policy**

`refresh` resolves every ticker once, then loops: fetch, classify, write. Each ticker's write goes
through `db::fund_mix::set_for_ticker` inside **one caller-owned `Db::transaction`**, so a refresh
of ten tickers is atomic. `Db::transaction` is **not reentrant** — `set_for_ticker` opens none of
its own, which is why this composes.

A failure is collected into `Refreshed::failed`, never propagated: one throttled ticker must not
discard nine good results.

`tickers_held` reads `DISTINCT ticker FROM holding` through `db::holding`.

- [ ] **Step 4: Run the tests**

```bash
cargo test --lib mix::
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

Commit with `jluszcz:commit`. Message: `feat(mix): refresh every held ticker in one transaction`

---

## Task 5: `mm mixes`, and `g`/`G` on the screen

**Files:**
- Modify: `src/bin/mm.rs`, `src/tui/app/funds.rs`, `src/tui/help.rs`

**Interfaces:**
- Consumes: task 4.
- Produces: subcommand `mm mixes [--ticker <T>]`; keys `g` and `G` on screen 6.

`g`/`G` is the established pairing from `src/tui/CLAUDE.md`: "`G` regenerates every recurring
transaction where `g` regenerates the selected one." A mix refresh is the same verb — idempotent,
derives rows from an external rule, safe to repeat — so no letter is invented.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn refreshing_with_no_sec_contact_configured_says_what_to_set() {
    let mut app = test_support::app_with_holdings();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    app.key(KeyCode::Char('G'));

    let status = app.status_line();
    assert!(
        status.contains("contact"),
        "the refusal does not name the setting to fix: {status}"
    );
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test --lib refreshing_with_no_sec_contact`
Expected: FAIL — `G` is unbound, so the status line is empty.

- [ ] **Step 3: Bind the keys**

In `src/tui/app/funds.rs`, `g` refreshes the selected row's ticker and `G` every held ticker, both
through `crate::mix::refresh`. An unset contact is a refusal naming the setting, printed on the
status line — never a request sent anonymously.

- [ ] **Step 4: Add the subcommand**

In `src/bin/mm.rs`, add `mixes` beside `report` and `backup`. Unlike `import` it is not behind a
feature: it is part of an ordinary build.

- [ ] **Step 5: Restore the help entries**

In `src/tui/help.rs`, add `'g'` and `'G'` to screen 6's `Topic`. Keep within
`no_panel_entry_runs_longer_than_a_glance`.

- [ ] **Step 6: Run the tests and commit**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Commit with `jluszcz:commit`. Message: `feat(tui): refresh fund mixes from the Funds screen`

---

## Task 6: The screen draws the allocation

**Files:**
- Modify: `src/tui/fund.rs`

**Interfaces:**
- Consumes: tasks 4 and 5, and `crate::fund::targets_from_db` from the model plan's task 2. Extends
  `tui::fund::Row` from the model plan's task 8, whose `stock_percent` and `as_of` are already
  `Option`.
- Produces: `tui::fund::TargetClass { Bonds, UsStock, IntlStock }` and
  `SummaryRow { class: TargetClass, target: Option<BasisPoints>, actual: BasisPoints,
  delta: Option<BasisPoints> }`, reachable as `Funds::summary_row(TargetClass) -> Option<SummaryRow>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_summary_totals_every_holding_weighted_by_its_mix() {
    let mut app = test_support::app_with_mixes();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    let summary = app.funds().summary();
    let total: i64 = summary.iter().map(|s| s.weight.0).sum();
    assert_eq!(total, 10_000, "the summary does not foot to 100%");
}

#[test]
fn the_unclassified_row_is_absent_when_nothing_is_unclassified() {
    let mut app = test_support::app_with_mixes();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    assert!(
        !app.funds().summary().iter().any(|s| s.class == AssetClass::Unclassified),
        "an empty Unclassified row was drawn"
    );
}

#[test]
fn the_account_filter_recomputes_the_summary_for_that_account_alone() {
    let mut app = test_support::app_with_mixes();
    app.screen = Screen::Funds;
    app.reload().unwrap();
    let all = app.funds().summary();

    app.key(KeyCode::Tab);

    assert_ne!(app.funds().summary(), all, "Tab did not recompute the summary");
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib the_summary_totals_every_holding`
Expected: FAIL — no method `app_with_mixes`.

- [ ] **Step 3: Extend the fixture**

`app_with_mixes()` is `app_with_holdings()` plus `fund_mix` rows for every ticker but one — the one
without is what keeps the "never fetched" test from the model plan honest once mixes exist. It also
sets `key::BIRTH_DATE`, since two of the tests below are about the target column; set it from
`test_support::day` relative to the fixture's own `today()` rather than writing a year, so no real
birth year enters a tracked file.

- [ ] **Step 4: Draw the summary, the targets and the bars**

The summary is each holding's balance apportioned by its ticker's mix, summed by class, then
expressed as basis points of the total. A holding with no mix contributes to the total and to no
class — and the screen says so on its row rather than in the summary.

Beside each actual sits its target, from `crate::fund::targets_from_db`. **The target-bearing rows
are Bonds, U.S. stock and Intl stock** — Bonds being `us_bond + intl_bond` combined, because the age
rule produces one number and splitting it would invent a precision the rule does not have. `cash`
and `unclassified` carry no target and draw an em dash in that column.

Δ is `target − actual`, signed, drawn wherever a target exists. A `None` bond target — no birth date
on record — draws an em dash in both the Target and Δ columns, never a zero.

`Unclassified` is drawn only when non-zero, as the two transfer footers on Planning are.

Add these tests alongside the three in step 1:

```rust
#[test]
fn the_bond_target_is_drawn_against_the_combined_bond_share() {
    let mut app = test_support::app_with_mixes();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    let bonds = app.funds().summary_row(TargetClass::Bonds).expect("a Bonds row");
    let mix = app.funds().mix();
    assert_eq!(
        bonds.actual,
        weight(&mix, AssetClass::UsBond) + weight(&mix, AssetClass::IntlBond)
    );
}

#[test]
fn with_no_birth_date_on_record_the_bond_target_is_blank_rather_than_zero() {
    let mut app = test_support::app_with_mixes();
    setting::clear(&app.db, key::BIRTH_DATE).unwrap();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    let bonds = app.funds().summary_row(TargetClass::Bonds).unwrap();
    assert_eq!(bonds.target, None, "a missing birth date drew a zero bond target");
    assert_eq!(bonds.delta, None);
}
```

- [ ] **Step 5: Add the width test and run**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 6: Commit**

Commit with `jluszcz:commit`. Message: `feat(tui): the Funds screen totals the look-through`

---

## Task 7: The report tab

**Files:**
- Modify: `src/report/html/funds.rs`, `src/report/mod.rs`, `src/palette.rs`

**Interfaces:**
- Consumes: task 6.
- Produces: `palette::ASSET_CLASSES: [(u8, u8, u8); 6]`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_allocation_tab_carries_no_script() {
    let html = render(&fixture::snapshot());
    assert!(!html.contains("<script"), "the report grew a script");
}

#[test]
fn every_asset_class_bar_has_a_width_and_a_percentage_beside_it() {
    let html = render(&fixture::snapshot());
    for class in AssetClass::ALL {
        if class == AssetClass::Unclassified { continue; }
        assert!(html.contains(class.label()), "{} is not labelled", class.label());
    }
}

#[test]
fn the_allocation_tab_draws_the_same_targets_the_screen_does() {
    let snapshot = fixture::snapshot();
    let html = render(&snapshot);
    for row in &snapshot.allocation.summary {
        let Some(target) = row.target else { continue };
        assert!(
            html.contains(&target.to_string()),
            "{:?}'s target is missing from the page",
            row.class
        );
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib report::html::funds`
Expected: FAIL — the tab renders an empty section from the model plan's task 2.

- [ ] **Step 3: Add the palette entries**

Four asset-class colors plus cash and unclassified in `src/palette.rs`, beside the funding ramp, so
the terminal and the page cannot disagree about what bonds look like.

- [ ] **Step 4: Render the tab**

Bars are CSS widths on a `<div>` with the percentage as text beside them — **the page carries no
script and is read offline on a phone**. The per-account breakdown renders as stacked sections
rather than the screen's `Tab` cycle, which the page has no way to offer.

The summary table carries the same Target / Actual / Δ columns the screen draws, from the same
`crate::fund::targets_from_db` — the tab is a spelling of the screen, not a second reading. A
`None` bond target draws an em dash in both columns here too, for the reason it does there.

- [ ] **Step 5: Run the tests and commit**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Commit with `jluszcz:commit`. Message: `feat(report): an allocation tab over the held funds`

---

## Task 8: The live test, and documentation

**Files:**
- Create: `tests/sec_live.rs`, `src/mix/CLAUDE.md`
- Modify: `CLAUDE.md` (root), `src/tui/CLAUDE.md`, `src/report/CLAUDE.md`, `README.md`

- [ ] **Step 1: Write the live test**

`tests/sec_live.rs` mirrors `tests/common/mod.rs`'s skip-or-fail shape: `MM_SEC_TICKER` names the
fund, `MM_REQUIRE_SEC=1` turns a skip into a failure. Unset, it skips loudly and a clean checkout
passes.

It asserts **structurally** — never a figure, so nothing rots when the quarter turns:

```rust
#[test]
fn the_three_hops_resolve_and_the_weights_foot() {
    let Some(ticker) = require_ticker() else { return };
    let contact = require_contact();

    let series = mix::sec::resolve_series(&contact, &[ticker.clone()]).unwrap();
    let series_id = series.get(&ticker).expect("the ticker resolved to a series");

    let filing = mix::sec::latest_filing(&contact, series_id).unwrap();
    assert!(!filing.holdings.is_empty(), "the filing carried no holdings");

    let slices = mix::classify(&filing.holdings);
    let total: i64 = slices.iter().map(|s| s.weight.0).sum();
    assert_eq!(total, 10_000);
    assert!(
        slices.iter().any(|s| s.weight > BasisPoints::ZERO
            && s.class != crate::db::fund_mix::AssetClass::Unclassified),
        "every slice landed in Unclassified, so the classifier matched nothing"
    );
}
```

**This is the only thing that catches EDGAR changing a URL shape or a field name.** Without it the
feature fails silently next quarter. Say so at the top of the file.

- [ ] **Step 2: Run it**

```bash
cargo test sec_live                                       # skips loudly
MM_REQUIRE_SEC=1 MM_SEC_TICKER=<ticker> cargo test sec_live  # actually runs
```

Expected: skip, then PASS.

- [ ] **Step 3: Write `src/mix/CLAUDE.md`**

The three hops and why the series ID goes in the `CIK` slot; SEC etiquette and where the contact
comes from; the two classification paths and the measured separation behind
`FUND_OF_FUNDS_MAX`; why `Unclassified` is stored rather than folded away.

- [ ] **Step 4: Update the root `CLAUDE.md`**

Add to the architecture table:

```markdown
| `src/mix/` | A fund's composition, from SEC N-PORT filings. `sec` is the only place `reqwest`, `tokio` and `jluszcz_rust_utils` are named — the second such seam beside `src/backup/s3.rs`; `classify` is pure; `mod` is the policy that writes `db::fund_mix`. |
```

And the invariants:

```markdown
- **`Unclassified` is a class, not a gap.** The classifier is two heuristics — a name over a
  fund-of-funds holding, an `assetCat` over a security — and a miss is stored and drawn as its own
  row rather than folded into a neighbour. That is what makes the heuristics safe to use: money in
  the wrong bucket is invisible, money in a labelled bucket is a question the owner can answer. Same
  stance `transfer::resolve` takes toward a dangling key.
- **A fund with no `fund_mix` row has never been fetched, which is not the same as holding nothing.**
  Both `Row::stock_percent` and `Row::as_of` are `Option`, and the screen draws "never fetched"
  rather than 0%. The screen that says so is the screen `G` is pressed from.
- **A refresh is atomic across every ticker but tolerant of any one of them.**
  `db::fund_mix::set_for_ticker` opens no transaction of its own — `mix::refresh` owns one for the
  whole run, the way `txn::write_transfer` composes into one payday — while a ticker that fails is
  collected rather than propagated, so one throttled fund cannot discard nine good results.
```

- [ ] **Step 5: Update `README.md`**

Document `mm mixes` and the `[sec] contact` setting. **No real address in the example** — use
`user@example.com`.

- [ ] **Step 6: Final gate**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
MM_REQUIRE_SEC=1 MM_SEC_TICKER=<ticker> cargo test sec_live
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<path> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test --features import
```

Expected: all pass.

- [ ] **Step 7: Commit**

Commit with `jluszcz:commit`. Message: `docs(mix): state how a fund's composition is resolved`
