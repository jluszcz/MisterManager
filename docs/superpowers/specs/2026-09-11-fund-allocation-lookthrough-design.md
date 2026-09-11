# Fund Allocation Look-Through — Design

**Status:** approved in conversation, 2026-09-11
**Branch:** `remove-funds-screen`

## Goal

Replace the Funds screen's age-based asset-allocation block with a **look-through view**: the
owner types a balance per fund per account, and the app derives a bonds / domestic-stock /
international-stock breakdown across the whole portfolio by fetching each fund's published
composition from SEC N-PORT filings.

The figure the owner should never have to type is the one inside a fund. Balances are typed;
compositions are fetched.

## Scope

### What goes

| File | Lines | Disposition |
|---|---|---|
| `src/fund.rs` | 218 | Deleted. |
| `src/calc/fund.rs` | 374 | Deleted. |
| `src/db/fund.rs` | 394 | Deleted. |
| `src/import/fund.rs` | 395 | Deleted. |
| `tests/fund_from_workbook.rs` | 108 | Deleted. |
| `src/tui/fund.rs` | 795 | **Rewritten.** |
| `src/tui/app/funds.rs` | 369 | **Rewritten.** |
| `src/report/html/funds.rs` | 109 | **Rewritten.** |

Also removed: `db::FundId`, `key::BIRTH_DATE`, the `Planning!I1:M5` block in the importer, and
`Constants!K2` (the birth date's only producer). The `fund` table is dropped by a migration arm;
`schema.sql` is a frozen baseline and is not edited.

The birth date is read on the fund-import path (`src/import/mod.rs:265`), in `src/fund.rs`, and on
the Funds screen. Nothing else reads it, so it leaves with them.

Screen 6 keeps its slot and its three files are rewritten rather than deleted, so the exhaustive
`match self.screen` blocks in `src/tui/app/mod.rs` never pass through a state with a hole in them.

### What survives, and is easy to delete by mistake

- **`BasisPoints`** (`src/rate.rs`). It loses the age rule and gains `fund_mix.weight_bp`, so it
  keeps three callers throughout.
- **The paired `CHECK` construction** `db::fund::Target` introduced — a column whose presence is
  tied to another column's value. It moves to `account.tax_treatment`.
- **The palette's funding ramp** (`src/palette.rs`). That is how funded a *goal* is, unrelated.
- **Every `Emergency Fund`** — `gate::Gate::EmergencyFund`, `plan_line::Line::EmergencyFund`,
  `calc::planning::Lines::emergency_fund`. A Planning gate and the goal behind it. Exclude it from
  every `fund` grep.
- **`calc::pro_rata`**, `calc::tax`, and the rest of `src/calc/`.

## Data model

### `account` gains a third kind

SQLite cannot alter a `CHECK`, so this is a create/copy/drop/rename arm — the first table rebuild
in `MIGRATIONS`. Two facts make it plain rather than delicate: there is **no `PRAGMA foreign_keys =
ON`** anywhere in the crate, so the three `REFERENCES account(id)` declarations are documentation
and not enforcement; and `db::migration::apply` runs the whole chain in one transaction.

The arm names the table as it stands *at that version* — carrying `color` (arm 2) and
`interest_policy` — and recreates the `account_code_kind` index (arm 5) after the rename.

```sql
kind TEXT NOT NULL CHECK (kind IN ('cash', 'credit', 'investment')),
grp  TEXT NOT NULL CHECK (grp  IN ('checking', 'savings', 'credit', 'investment')),
tax_treatment TEXT CHECK (tax_treatment IN ('taxable', 'tax_deferred', 'tax_free')),
CHECK ((kind = 'investment') = (tax_treatment IS NOT NULL))
```

`Kind::ALL` and `Group::ALL` are fixed-length arrays and `Group::bands` is a table, so the compiler
flags each site a third variant reaches. `overview.rs:137` matches `account.kind` exhaustively and
is flagged likewise.

**Investment accounts do not appear on the Overview and do not count toward Net.** `kind =
'investment'` means "an account the Overview skips". Net keeps meaning spendable net worth derived
from the dated ledger, which is what makes its three columns a widening horizon; a holding balance
is not dated and would read the same in all three. What reusing `account` buys is naming, colors,
ordering and the Accounts screen — deliberately not the balance model.

The 35 `account::list_by_kind` call sites already filter by kind and stay correct. The ~26 bare
`account::list` call sites — pickers, transfer destinations, ledger and worksheet account
selection — are the audit list.

### Three new tables

```sql
CREATE TABLE holding (
  id            INTEGER PRIMARY KEY,
  account_id    INTEGER NOT NULL REFERENCES account(id),
  ticker        TEXT    NOT NULL,
  balance_cents INTEGER NOT NULL,
  sort          INTEGER NOT NULL DEFAULT 0,
  UNIQUE (account_id, ticker)
);
```

`UNIQUE (account_id, ticker)`: two rows of one ticker in one account is a typo, while one ticker
held in two accounts is the ordinary case and stays legal — the same shape as `UNIQUE (code, kind)`
on `account`.

```sql
CREATE TABLE fund_mix (
  ticker      TEXT    NOT NULL,
  asset_class TEXT    NOT NULL CHECK (asset_class IN
                ('us_stock', 'intl_stock', 'us_bond', 'intl_bond', 'cash', 'unclassified')),
  weight_bp   INTEGER NOT NULL,
  report_date TEXT    NOT NULL,
  PRIMARY KEY (ticker, asset_class)
);
```

`report_date` is per ticker rather than per fetch: fund families file on their own schedules, and
the screen has to be able to quote an as-of date per row rather than once for the page. One row per
(ticker, asset_class) makes a refresh idempotent, and a fetch that fails for one ticker leaves that
ticker's previous quarter standing rather than blanking it.

No alias table. A collective investment trust — a 401(k) fund with no ticker and no SEC filing — is
recorded under the ticker of the retail fund it tracks. The account column distinguishes it from a
genuine holding of that fund.

**Both new tables go in `PRESERVED_TABLES`**, and the reason is uniform: the workbook carries none
of this, so a `--replace` has nothing to say about it.
`every_table_the_schema_creates_is_either_cleared_or_deliberately_kept` is what will demand it.

## Fetching

### Where the network lives

`src/mix/`, three files, arranged as `src/backup/` is:

| File | Responsibility |
|---|---|
| `src/mix/sec.rs` | The only place `reqwest`, `tokio` and `jluszcz_rust_utils` are named. Resolves tickers, returns parsed holdings. |
| `src/mix/classify.rs` | Pure. Holdings to asset-class weights. No network, no database. |
| `src/mix/mod.rs` | Policy: which tickers need refreshing, and writing results to `db::fund_mix`. |

`sec.rs` opens a current-thread runtime and blocks on it, as `src/backup/s3.rs:26` does. That file's
doc comment currently claims to be the only place `tokio` is named; it becomes one of two and the
sentence is amended.

### The three hops

1. `https://www.sec.gov/files/company_tickers_mf.json` (1.2 MB) — ticker to CIK and series ID.
   Parsed for the matching rows and dropped; caching thirty thousand funds to look up ten is the
   wrong trade.
2. `browse-edgar?action=getcompany&CIK=<seriesId>&type=NPORT-P&count=1&output=atom` — **the series
   ID goes in the CIK slot.** This is what yields a clean per-fund filing history instead of the
   whole trust's.
3. That filing's `primary_doc.xml`.

Requests run sequentially. SEC's published limit is 10/second and there is nothing to gain by
approaching it.

### SEC etiquette, and where the contact comes from

A request without a `User-Agent` declaring a contact is refused with 403. `~/.claude/CLAUDE.md`
forbids writing a real email address into any file, so the contact is read at runtime from
`src/config.rs` — a `[sec]` section beside the existing `[report]` and `[backup]` ones. An unset
contact is a refusal naming the setting, never a request sent anonymously.

`query::send` retries 5xx and 429. SEC also throttles with 403, so `mix/sec.rs` treats a 403
carrying SEC's throttle body as transient and a plain 403 as permanent.

### Dependency: `jluszcz_rust_utils`

Taken as `{ git = "https://github.com/jluszcz/rust-utils", features = ["query"] }`, the spelling
five sibling projects already use. `query::send` is the reason: retry with jittered backoff, no
retry on non-idempotent methods, and an error that keeps the response body `error_for_status`
discards.

**This requires an upstream change first.** `features = ["query"]` currently pulls `aws-lc-sys`,
because reqwest 0.13 resolves `default` to `default-tls` to `rustls` to `__rustls-aws-lc-rs`.
`aws-lc-sys` compiles C, which the `aarch64-unknown-linux-musl` target both CI jobs build for has
no toolchain for — the constraint `Cargo.toml` documents at the `aws-config` dependency. The fix is
in rust-utils:

```toml
reqwest = { version = "0.13", default-features = false,
           features = ["gzip", "json", "query", "http2", "charset", "rustls-no-provider"] }
```

Verified to resolve to `ring` with no `aws-lc-sys` and no `openssl-sys`. Two consequences: a
process-wide rustls provider must be installed before the first request (rust-utils' existing
`tls::install_default_provider` is aws-lc-rs and needs a ring variant or a feature switch), and
`system-proxy` leaves reqwest's defaults and is re-added explicitly.

**`rust_utils::cache` is not used.** `dated_cache_path` writes to `$TMPDIR` and expires daily;
N-PORT changes quarterly and lags roughly sixty days, so daily expiry would re-fetch to learn
nothing and a temp file would not survive. `fund_mix` is the cache, invalidated by `report_date`.

**`fern` and `log` are unconditional dependencies of rust-utils.** Harmless provided
`set_up_logger` is never called — a logger writing to stderr would corrupt the TUI mid-frame.

## Classification

A holding's asset class is never stated by the filing. `assetCat` reads `EC` for every underlying
fund (a fund *share* is equity whatever it holds) and `invCountry` reads `US` for all of them (the
fund is US-domiciled, not its holdings). So the class is derived, by one of two paths.

**A filing of 25 holdings or fewer is a fund of funds.** Measured separation on real filings: 7 and
5 for two target-date funds, against 3,546 / 8,878 / 17,409 for three direct index funds. This is a
chasm, not a threshold.

**Fund-of-funds path** — each holding is classified by keyword over its `name` and `title`
*concatenated*, because fund families are legible in opposite fields and which one is anybody's
guess: one measured family files an unreadable abbreviation as `title` and full prose as `name`,
another files the legal trust as `name` and the fund as `title`. Reading both is what makes the
classifier indifferent to which. Keywords are generic vocabulary and name no institution:

- international: `international`, `intl`, `ex u.s.`, `ex-u.s.`, `global ex`, `developed markets`,
  `emerging markets`
- bond: `bond`, `treasury`, `fixed income`
- cash: `liquidity`, `money market`, `short-term reserve`, `cash`

**Direct-fund path** — holdings are securities, classified by `assetCat` with `invCountry`
selecting the domestic or international variant:

| `assetCat` | Class |
|---|---|
| `EC`, `EP` | stock |
| `DBT`, `ABS-MBS`, `ABS-CBDO`, `ABS-O`, `LON` | bond |
| `STIV` | cash |
| anything else | `unclassified` |

`ABS-MBS` matters: it is 20.5% of one measured bond fund, and omitting it would quietly lose a
fifth of that fund. The unmapped remainder on real filings is derivatives at about 0.05%.

**`Unclassified` is a real class, stored and drawn**, never folded into a neighbour. It is what
makes the heuristics safe: a miss surfaces as a labelled row rather than money silently in the
wrong bucket — the stance `transfer::resolve` already takes toward a dangling key.

Weights normalize to 10,000 bp with the remainder going to the largest bucket, so a row always
foots.

There is no override table. The classifier is correct on every holding of the portfolio this was
designed against, `Unclassified` is visible, and the first genuine miss can add a keyword. A schema
surface serving a case that has not happened is not earned yet.

## Screens

### Funds, screen 6

Keys, all drawn from the existing vocabulary in `src/tui/CLAUDE.md`:

| Key | Does |
|---|---|
| `a` / `e` / `d` | add, edit, delete a holding — account, ticker, balance |
| `g` / `G` | refresh the selected holding's ticker / every ticker |
| `Tab` / `BackTab` | cycle the account filter: All, then one per investment account |
| `/` | filter by typing, over ticker and account |
| `Enter` | the selected fund's full look-through — its underlying holdings and weights |

`g`/`G` is the established pairing: "`G` regenerates every recurring transaction where `g`
regenerates the selected one". A mix refresh is the same verb — idempotent, derives rows from an
external rule, safe to repeat.

`Tab` is load-bearing. Filtering to one account recomputes the summary above the list for that
account alone, which answers "where do the bonds live" with the mechanism the screen already needed
rather than a second panel competing for height.

Layout follows the width rules: `Account` takes the single `Constraint::Min` and absorbs the slack;
ticker, balance, stock percentage and the as-of date are `Length`-sized to their true content. The
mix bar is fixed-width and glyph-based, so it truncates from the right like text rather than losing
leading characters the way a right-aligned figure would.

**A ticker with no `fund_mix` rows reads "never fetched", not 0%.** Zero and unknown mean opposite
things — a fund holding no stock, versus a fund nobody has asked SEC about — and the screen saying
so is the screen `G` is pressed from.

**The `Unclassified` summary row is drawn only when non-zero**, as the two transfer footers on
Planning are.

### Accounts, screen 9

`a` already creates an account the workbook does not name. Its kind selector grows `investment`,
and selecting it reveals a tax-treatment field. That form is where the paired `CHECK` is enforced in
Rust: it cannot produce the invalid combination.

### Report

`src/report/html/funds.rs` is rewritten to the same shape. The page carries no script and is read
offline on a phone, so bars are CSS widths on a `<div>` with the percentage as text beside them, and
the per-account breakdown renders as stacked sections rather than a `Tab` cycle the page cannot
offer — the same split `plan_rows` makes, where the Destinations block is the screen's alone.

Colors come from `src/palette.rs`, which gains four asset-class entries beside the funding ramp, so
the terminal and the page cannot disagree about what bonds look like.

## Testing

### Fixtures name no institution

A saved N-PORT fixture is real XML naming real fund families and the owner's actual holdings —
banned from every tracked file. `src/test_support` grows a fund vocabulary as it already carries
`cash` and `credit`, and the XML fixtures are written in it:

| Ticker | Name | Exercises |
|---|---|---|
| `TDF45`, `TDF35` | `Target 2045 Fund`, `Target 2035 Fund` | no class in the name, so the look-through path |
| `USM` | `Total Market Index Fund` | `us_stock` |
| `ISM` | `International Stock Index Fund` | `intl_stock` |
| `USB` | `Total Bond Index Fund` | `us_bond` |
| `ISB` | `International Bond Index Fund` | both keywords, international wins |
| `UNC` | `Overseas Growth Fund` | matches nothing, so `Unclassified` |

`UNC` earns its row: "Overseas" is a near-miss for the `international` keyword, so it pins the
behaviour that an unrecognised name lands in a visible bucket rather than a plausible one. A code
outside the table panics, as `cash` does.

### Where the coverage sits

`mix/classify.rs` carries it, because it is pure: the two paths, the 25-holding fork, the `assetCat`
mapping including `ABS-MBS`, normalization to 10,000 bp, and `Unclassified` are ordinary `mod tests`
against invented input. `mix/sec.rs` stays thin on purpose — URL construction and XML parsing is
close to all that can go wrong in it.

### One live test, opted into like the workbook oracle

`MM_SEC_TICKER` names a fund; `MM_REQUIRE_SEC=1` turns a skip into a failure. Unset, it skips
loudly and a clean checkout passes. The ticker lives in the environment for the reason `MM_WORKBOOK`
does: naming one in a tracked file says which funds the owner holds.

It asserts **structurally** — three hops resolve, weights foot to 10,000 bp, `report_date` parses,
at least one class is non-zero. No figure is written down, so nothing rots when the quarter turns.

This is the only thing that catches EDGAR changing a URL shape or a field name; without it the
feature fails silently next quarter. It is the bet `MM_REQUIRE_WORKBOOK` already makes.

### Migration

The chain replays from version 1 on every `cargo test`, so the `account` rebuild is exercised
constantly. What that does not cover is data survival: one test seeds an account with a color and a
non-default `interest_policy`, runs the arm, and asserts both survive **with the row's id intact** —
the id being what three tables reference.

### What has no oracle

The workbook carries none of this. `tests/fund_from_workbook.rs` is deleted and nothing replaces it;
`tests/` gains no new binary. Stated here because "the workbook is the test oracle" is the first
thing `CLAUDE.md` says, and a reader will look for the missing file.

## Documentation

`CLAUDE.md` (root) loses the four fund bullets and the `src/fund.rs`, `src/calc/fund.rs`,
`src/db/fund.rs`, `src/import/fund.rs` rows, and gains: `src/mix/` in the architecture table, the
investment-kind and Overview-exclusion invariant, the `PRESERVED_TABLES` reasoning, and the
`Unclassified` stance.

`src/tui/CLAUDE.md` gains screen 6's new identity and `g`/`G` on it. `src/import/CLAUDE.md` loses
the `Planning!I1:M5` and `Constants!K2` mappings. `src/calc/CLAUDE.md` loses the fund derivation.
`src/report/CLAUDE.md` gains the allocation tab. `src/mix/CLAUDE.md` is new, and carries what this
spec says about the three hops, SEC etiquette, and the two classification paths.

## Open items

- The rust-utils change lands upstream before this crate can take the dependency.
- The `remove-funds-screen` branch holds a single commit planning an abandoned approach — deleting
  the Funds feature outright rather than replacing it. It should be deleted rather than merged, so
  no agent following `superpowers:executing-plans` picks that plan up.
