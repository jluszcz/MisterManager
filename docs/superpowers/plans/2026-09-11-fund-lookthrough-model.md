# Fund Look-Through, Part 1: Model and Holdings Screen

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hand-typed asset-allocation table with typed holdings — investment accounts,
the funds held in them, and a place to store each fund's composition — while **keeping the
age-based target rule**, which the look-through will be measured against.

**Architecture:** Nine tasks, leaf-consumer first so every intermediate state compiles and the whole
suite stays green. The screen and report tab are emptied before the derivation they read
(tasks 1–2), the derivation before the table (tasks 3–4), and the new model is built on the ground
that clears (tasks 5–7) before the screen is refilled (task 8).

**Tech Stack:** Rust 2024, `rusqlite` (bundled SQLite), `ratatui`/`crossterm`, `calamine` behind the
non-default `import` feature, `anyhow`, `chrono`.

**Spec:** `docs/superpowers/specs/2026-09-11-fund-allocation-lookthrough-design.md`

**Follow-on:** `docs/superpowers/plans/2026-09-11-fund-lookthrough-fetch.md` adds the SEC fetcher,
the classifier and the report tab. This plan deliberately stops with every mix reading
"never fetched", which is a real state the screen must handle anyway.

## Global Constraints

Copied from `CLAUDE.md` (root) and `~/.claude/CLAUDE.md`. Every task's requirements implicitly
include this section.

- **No real data in the repository.** No balances, institution names, account codes, or goal names
  traceable to a real person — in source, tests, fixtures, docs, commit messages or PR text. The
  invented vocabulary is `src/test_support::{cash, credit}`, which this plan extends.
- **`rusqlite` is named only inside `src/db/`.** `calamine` only inside `src/import/`.
  `ratatui`/`crossterm` only inside `src/tui/`.
- **`schema.sql` is a frozen baseline and is never edited to describe a change.** Every schema
  change is an arm appended to `db::migration::MIGRATIONS`. An arm must survive replaying against an
  empty database. **The head version is currently 10** (nine arms), so the arms this plan adds are
  11, 12 and 13.
- **Never disable a failing test; fix it.** A deleted feature's tests go with it; a test covering
  something else that merely mentions funds is edited, never `#[ignore]`d.
- **Anything the schema constrains has a Rust type that says the same thing**, so the `CHECK` is a
  backstop rather than the only guard.
- **A `get` by id errors when the row is missing; it does not return `None`.**
- **Every query module states its columns in a `select_*!` macro** beside it and builds every
  `SELECT` of that row from it.
- **Test names are full sentences.** Unit tests live in `mod tests` at the bottom of the file under
  test, against `db::open_in_memory`.
- **A test that walks a fixture uses `test_support::walk_until!`, never a bare `while`.**
- **Documentation describes the code as it is, not how it got there.** No "removed", "previously".
- **Never commit directly to the default branch.** This work goes on a feature branch off `main`.
- **Commit with the `jluszcz:commit` skill.** Each task's final step is one commit.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LPunW61FrbUCTazxaNj23Y
  ```

### Commands

```bash
cargo test
cargo test --lib                                          # no workbook needed
cargo fmt                                                 # pre-commit hook runs --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings  # CI runs both

# The workbook oracle. Task 3 touches the importer, so it runs there and at the final gate.
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<path> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test --features import
```

Ask the owner for `MM_WORKBOOK` and `MM_ACCOUNTS`, or read them from the shell environment. **Neither
may be written into any file.** Without `MM_REQUIRE_WORKBOOK=1` those tests skip silently, and
without `--features import` they compile to nothing.

### Grep discipline

`fund` matches far more prose than code. Every sweep in this plan excludes the Planning gate and the
goal behind it, which are unrelated and are the single most likely thing to delete by mistake:

```bash
grep -rn 'fund' --include='*.rs' src/ tests/ | grep -iv 'emergency' | grep -v 'refund'
```

`gate::Gate::EmergencyFund`, `plan_line::Line::EmergencyFund` and
`calc::planning::Lines::emergency_fund` all stay.

---

## File Structure

| File | Change |
|---|---|
| `src/tui/fund.rs` | Emptied in task 1, refilled in task 8 as the holdings screen. |
| `src/tui/app/funds.rs` | Same. |
| `src/report/html/funds.rs` | Emptied in task 2; refilled by the follow-on plan. |
| `src/fund.rs` | Rewritten (task 2) — reads settings instead of a table. |
| `src/calc/fund.rs` | Shrunk (task 2) to the target rule alone. |
| `src/import/fund.rs` | Deleted (task 3). |
| `src/db/fund.rs` | Deleted (task 4). |
| `src/db/id.rs` | `FundId` removed (task 4). |
| `src/db/migration.rs` | Arms 11 (drop `fund`), 12 (rebuild `account`), 13 (new tables). |
| `src/db/account.rs` | `Kind::Investment`, `Group::Investment`, `TaxTreatment` (task 5). |
| `src/db/holding.rs` | New — the `holding` table (task 6). |
| `src/db/fund_mix.rs` | New — the `fund_mix` table (task 6). |
| `src/db/mod.rs` | Module wiring; `PRESERVED_TABLES` gains two entries (tasks 4, 6). |
| `src/overview.rs` | Investment accounts excluded from every band (task 5). |
| `src/tui/accounts.rs`, `src/tui/app/accounts.rs` | Kind selector and tax-treatment field (task 7). |
| `src/test_support.rs` | `investment()` builder and the fund vocabulary (task 6). |

---

## Task 1: Empty the Funds screen

Leaf first: nothing may read `crate::fund` by the time task 2 deletes it.

**Files:**
- Modify: `src/tui/fund.rs`
- Modify: `src/tui/app/funds.rs`
- Modify: `src/tui/modal.rs` (the birth-date prompt variant)
- Modify: `src/tui/help.rs` (screen 6's `Topic`)

**Interfaces:**
- Consumes: nothing.
- Produces: `tui::fund::Funds` — a view-state struct with `pub fn new() -> Funds` and
  `pub fn render(&self, frame: &mut Frame, area: Rect)`. Task 8 replaces its internals; tasks 2–7
  only need it to compile and draw.

- [ ] **Step 1: Replace `src/tui/fund.rs` wholesale**

```rust
//! Screen 6: the funds held across the owner's investment accounts.
//!
//! A placeholder until the holdings model exists. It draws one line and owns
//! no state, so nothing above it has to know the screen is unfinished.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

#[derive(Default)]
pub struct Funds;

impl Funds {
    pub fn new() -> Funds {
        Funds
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Paragraph::new("No holdings yet."), area);
    }
}
```

- [ ] **Step 2: Replace `src/tui/app/funds.rs` wholesale**

```rust
//! Screen 6's key handling.
//!
//! The screen holds no rows yet, so every key falls through to `App::dispatch`
//! above it.

use super::App;
use crossterm::event::KeyEvent;

impl App {
    pub(super) fn funds_key(&mut self, _key: KeyEvent) -> anyhow::Result<()> {
        Ok(())
    }

    pub(super) fn reload_funds(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 3: Remove the birth-date prompt from `src/tui/modal.rs`**

Delete the `Modal` variant the birth-date prompt uses and its arm in `modal_key` in
`src/tui/app/mod.rs`. Run `cargo build` and let the exhaustive matches name every site.

- [ ] **Step 4: Trim screen 6's help topic**

In `src/tui/help.rs`, reduce screen 6's `Topic` to no entries. Its footer becomes empty, which is
what a screen with no keys should show.

- [ ] **Step 5: Delete the tests that covered the old screen**

Delete the `mod tests` blocks in both files. They test a feature that no longer exists; they are not
disabled, they are removed along with what they covered.

- [ ] **Step 6: Build and test**

```bash
cargo test --lib
cargo clippy --all-targets -- -D warnings
```

Expected: PASS. Any remaining reference to `crate::fund` from `src/tui/` is a miss — fix it here.

- [ ] **Step 7: Commit**

```bash
git add src/tui/fund.rs src/tui/app/funds.rs src/tui/modal.rs src/tui/help.rs src/tui/app/mod.rs
```

Commit with `jluszcz:commit`. Message: `refactor(tui): empty the Funds screen ahead of its new model`

---

## Task 2: Shrink the derivation to the target rule

The table goes; the rule does not. `calc::fund` keeps `whole_years` and `BONDS_START_AGE` and loses
everything shaped around a table of rows.

**Files:**
- Modify: `src/calc/fund.rs`, `src/fund.rs`
- Modify: `src/report/html/funds.rs`, `src/report/mod.rs`, `src/report/html/mod.rs`,
  `src/report/html/fixture.rs`

**Interfaces:**
- Consumes: task 1 (nothing in `src/tui/` reads `crate::fund`).
- Produces:
  ```rust
  // src/calc/fund.rs
  pub const BONDS_START_AGE: i64 = 30;                          // unchanged
  pub fn whole_years(birth: NaiveDate, on: NaiveDate) -> i64;   // unchanged

  /// The three target shares, derived on every read.
  pub struct Targets {
      /// `None` only when no birth date is on record — a question to ask,
      /// not a zero to assume. The two equity targets then divide the whole
      /// 100% rather than being told a bond target that is really a question.
      pub bonds: Option<BasisPoints>,
      pub us_stock: BasisPoints,
      pub intl_stock: BasisPoints,
  }

  pub fn targets(age: Option<i64>, intl_equity_share: BasisPoints) -> Targets;

  // src/fund.rs
  pub fn targets_from_db(db: &Db, today: NaiveDate) -> Result<calc::fund::Targets>;
  ```

- [ ] **Step 0: Write the failing tests for the surviving rule**

In `src/calc/fund.rs`'s `mod tests`, keeping whichever existing `whole_years` tests still apply:

```rust
#[test]
fn the_bond_target_is_one_point_a_year_over_thirty() {
    let t = targets(Some(48), BasisPoints(4_000));
    assert_eq!(t.bonds, Some(BasisPoints(1_800)));
}

#[test]
fn the_equity_remainder_splits_by_the_configured_share() {
    let t = targets(Some(48), BasisPoints(4_000));
    // 82% equity, 40% of it international.
    assert_eq!(t.intl_stock, BasisPoints(3_280));
    assert_eq!(t.us_stock, BasisPoints(4_920));
}

#[test]
fn the_three_targets_foot_to_one_hundred_percent() {
    let t = targets(Some(48), BasisPoints(4_000));
    let total = t.bonds.unwrap().0 + t.us_stock.0 + t.intl_stock.0;
    assert_eq!(total, 10_000);
}

#[test]
fn with_no_birth_date_the_equity_targets_divide_the_whole_hundred_percent() {
    let t = targets(None, BasisPoints(4_000));
    assert_eq!(t.bonds, None, "a missing birth date became a zero bond target");
    assert_eq!(t.intl_stock, BasisPoints(4_000));
    assert_eq!(t.us_stock, BasisPoints(6_000));
}

#[test]
fn an_age_at_or_under_thirty_targets_no_bonds_rather_than_a_negative_share() {
    assert_eq!(targets(Some(30), BasisPoints(4_000)).bonds, Some(BasisPoints::ZERO));
    assert_eq!(targets(Some(22), BasisPoints(4_000)).bonds, Some(BasisPoints::ZERO));
}

#[test]
fn an_age_past_a_hundred_and_thirty_targets_all_bonds_rather_than_overflowing() {
    assert_eq!(targets(Some(200), BasisPoints(4_000)).bonds, Some(BasisPoints(10_000)));
}
```

Run: `cargo test --lib calc::fund` — expected FAIL, `cannot find function targets`.

- [ ] **Step 0b: Shrink the module**

Delete `Rule`, `Row`, `ComputedRow`, `Computed` and the per-row `compute`. Keep `whole_years` and
`BONDS_START_AGE`, and add `Targets`/`targets` clamping at both ends as the tests above pin. Restate
the module doc to describe the rule as it now stands: three shares over a whole portfolio, with no
mention of the `Planning!J2:L4` block or the table.

- [ ] **Step 0c: Rewrite `src/fund.rs` to read settings**

It loses the `fund` table read and keeps the birth-date read, gaining the equity-split key:

```rust
pub fn targets_from_db(db: &Db, today: NaiveDate) -> Result<calc::fund::Targets> {
    let age = setting::get(db, key::BIRTH_DATE)?
        .map(|birth| calc::fund::whole_years(birth, today));
    // Unset is a real state: a database nobody has imported into yet. The
    // sheet's own split is what an import writes, and 40% is what it carries.
    let intl = setting::get(db, key::INTL_EQUITY_SHARE)?
        .unwrap_or(DEFAULT_INTL_EQUITY_SHARE);
    Ok(calc::fund::targets(age, intl))
}
```

- [ ] **Step 1: Empty the report's Funds tab**

Replace `src/report/html/funds.rs` with a function of the same name and signature the tab list
already calls, rendering an empty section. Read the neighbouring `src/report/html/savings.rs` for
the exact signature this crate uses and match it — do not invent one.

- [ ] **Step 2: Remove the fund fields from the report snapshot**

In `src/report/mod.rs`, delete the `Snapshot` field the Funds tab read and its population. In
`src/report/html/fixture.rs`, delete the fund rows and the `FundId` uses.

- [ ] **Step 3: Add the setting key**

In `src/db/setting.rs`, beside `BIRTH_DATE`:

```rust
/// The international share *of the equity remainder*, in basis points.
///
/// One key rather than one per side: two keys for one fact can disagree, which
/// is the reason only `Constants!G2` is imported for the pay cadence. Domestic
/// is what is left.
pub const INTL_EQUITY_SHARE: Key<BasisPoints> = Key::new("allocation.intl_equity_share");
```

Add it to the key-name test beside `BIRTH_DATE`'s line.

- [ ] **Step 4: Build and follow the errors**

```bash
cargo build --all-features
```

Every remaining reference is a compile error naming its site. Fix each by deletion, not by
re-implementation.

- [ ] **Step 5: Test**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS. `tests/fund_from_workbook.rs` still compiles at this point because it reads
`db::fund`, which survives until task 4.

- [ ] **Step 6: Commit**

Commit with `jluszcz:commit`. Message: `refactor(fund): target the portfolio rather than a table row`

---

## Task 3: Delete the importer's fund block; import the equity split

**Files:**
- Delete: `src/import/fund.rs`, `tests/fund_from_workbook.rs`
- Modify: `src/import/mod.rs:255-270`, `src/import/planning.rs`, `src/import/CLAUDE.md`
- Modify: `tests/planning_from_workbook.rs`

**Interfaces:**
- Consumes: task 2 (`key::INTL_EQUITY_SHARE` exists).
- Produces: `import::import_all` returns without a fund count. Check its current return type in
  `src/import/mod.rs` and drop the fund half of the tuple, updating `src/bin/mm.rs`'s reporting to
  match.

**`key::BIRTH_DATE` and `Constants!K2` stay put.** The bond target is `(age - 30)` points and
nothing else produces the age. `src/import/constants.rs` is **not** edited by this task.

**What replaces the fund block's other job.** `Planning!J3` and `J4` are the equity targets as
shares of the whole portfolio. The importer stores their ratio under one key rather than both cells
under two, for the reason only `Constants!G2` is imported for the pay cadence: two stored values for
one fact can disagree.

- [ ] **Step 1: Delete the fund import**

```bash
git rm src/import/fund.rs tests/fund_from_workbook.rs
```

Remove `mod fund;` from `src/import/mod.rs`.

- [ ] **Step 2: Remove the fund half of `import_all`**

In `src/import/mod.rs`, delete the block at lines 262–268 — the `quoted_at`/`age` derivation and
the `fund::import` call — and the `fund::targets_frozen` half of the return.

- [ ] **Step 3: Import the equity split**

Wherever the `Planning` sheet is read (`src/import/planning.rs`, or the module that already reads
`Planning!C6:E12` — find it rather than assuming), read `J3` and `J4` through
`import::cell::as_rate_bp` and store their ratio:

```rust
// The sheet carries both equity targets as shares of the whole portfolio.
// What is stored is the split between them, because the bond target moves
// with a birthday and these two would go stale beside it.
let intl = as_rate_bp(&at(2, 9)).context("Planning!J3 is not a rate")?;
let us = as_rate_bp(&at(3, 9)).context("Planning!J4 is not a rate")?;
let equity = intl.0 + us.0;
ensure!(equity > 0, "Planning!J3:J4 leave no equity to split");
setting::set(db, key::INTL_EQUITY_SHARE, BasisPoints(intl.0 * 10_000 / equity))?;
```

Check the row/column indices against `src/import/CLAUDE.md`'s sheet map before writing them — the
`at(row, col)` convention there is zero-based and `J` is column 9.

- [ ] **Step 4: Assert the derivation against the workbook**

In `tests/planning_from_workbook.rs`:

```rust
/// The stored split is the ratio of the two cells, not either cell.
///
/// Asserted against both, which is what would catch a sheet where the pair has
/// come apart — the same guard `import_constants` keeps over `G2` and `H2`.
#[test]
fn the_stored_equity_split_is_the_ratio_of_the_sheets_two_equity_targets() {
    let Some(ctx) = common::workbook() else { return };
    let range = ctx.planning_range();

    let intl = sheet_bp(&range, 2, 9);
    let us = sheet_bp(&range, 3, 9);
    let stored = setting::get(&ctx.db, key::INTL_EQUITY_SHARE).unwrap().unwrap();

    assert_eq!(stored, BasisPoints(intl.0 * 10_000 / (intl.0 + us.0)));
}
```

Read `tests/planning_from_workbook.rs`'s existing helpers and match them — `sheet_bp` and the
context accessor above are the shapes `tests/fund_from_workbook.rs` used, and may be named
differently here.

- [ ] **Step 5: Update `src/import/CLAUDE.md`**

Remove the `Planning!I1:M5` block mapping. **Keep the `Constants!K2` row.** Add `Planning!J3:J4`,
saying that the ratio rather than either cell is what is stored.

- [ ] **Step 6: Run the workbook oracle**

```bash
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<path> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test --features import
```

Expected: PASS. This is the gate that matters for this task — the importer changed, so a silent skip
here would be exactly the failure `MM_REQUIRE_WORKBOOK` exists to prevent.

- [ ] **Step 7: Commit**

Commit with `jluszcz:commit`. Message: `refactor(import): drop the fund block, keep the equity split`

---

## Task 4: Drop the `fund` table

**Files:**
- Delete: `src/db/fund.rs`
- Modify: `src/db/mod.rs`, `src/db/id.rs`, `src/db/migration.rs`

**Interfaces:**
- Consumes: task 3.
- Produces: schema head version **11**.

- [ ] **Step 1: Write the failing test**

In `src/db/migration.rs`'s `mod tests`:

```rust
#[test]
fn the_fund_table_is_gone_after_the_chain() {
    let db = crate::db::open_in_memory().unwrap();
    let count: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'fund'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "the fund table survived the migration chain");
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test --lib the_fund_table_is_gone`
Expected: FAIL — `assertion failed: left == right, left: 1, right: 0`.

- [ ] **Step 3: Append arm 11**

At the end of `MIGRATIONS` in `src/db/migration.rs`:

```rust
    Migration {
        version: 11,
        // The asset-allocation block is derived from each fund's published
        // composition now, not from a target the sheet carried per row, so
        // the table holds answers to a question nobody puts. Dropped here
        // rather than cleared by `--replace`, for the reason the retired
        // `pay.period_days` key was: an owner who never replaces would keep
        // the rows indefinitely.
        sql: "DROP TABLE IF EXISTS fund",
        // Nothing to move. A fresh install replaying the chain has no such
        // table, which `IF EXISTS` is what makes safe.
        data: None,
    },
```

- [ ] **Step 4: Delete the module and its id type**

```bash
git rm src/db/fund.rs
```

Remove `pub mod fund;` from `src/db/mod.rs`, remove `"fund"` from `IMPORTED_TABLES`
(`src/db/mod.rs:148`), and remove `FundId` from `src/db/id.rs` and its re-export in
`src/db/mod.rs`.

- [ ] **Step 5: Run it to make sure it passes**

```bash
cargo test --lib the_fund_table_is_gone
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS. `every_table_the_schema_creates_is_either_cleared_or_deliberately_kept` must still
pass — `fund` is now in neither list and no longer in the schema, which is consistent.

- [ ] **Step 6: Commit**

Commit with `jluszcz:commit`. Message: `feat(db): drop the fund table`

---

## Task 5: `account` gains the investment kind

**Files:**
- Modify: `src/db/migration.rs` (arm 12), `src/db/account.rs`, `src/overview.rs`

**Interfaces:**
- Consumes: task 4 (head version 11).
- Produces:
  - `account::Kind::Investment`, and `Kind::ALL: [Kind; 3]`
  - `account::Group::Investment`, and `Group::ALL: [Group; 4]`
  - `pub enum TaxTreatment { Taxable, TaxDeferred, TaxFree }` with
    `as_str(self) -> &'static str`, `label(self) -> &'static str`,
    `ALL: [TaxTreatment; 3]`, and `FromStr`
  - `account::Account` gains `pub tax_treatment: Option<TaxTreatment>`
  - `account::set_tax_treatment(db: &Db, id: AccountId, t: TaxTreatment) -> Result<()>`
  - `account::insert` gains a `tax_treatment: Option<TaxTreatment>` parameter
  - Schema head version **12**

- [ ] **Step 1: Write the failing tests**

In `src/db/account.rs`'s `mod tests`:

```rust
#[test]
fn an_investment_account_carries_a_tax_treatment() {
    let db = crate::db::open_in_memory().unwrap();
    let id = insert(
        &db,
        "RET",
        "Retirement",
        Kind::Investment,
        0,
        Some(TaxTreatment::TaxDeferred),
    )
    .unwrap();

    assert_eq!(get(&db, id).unwrap().tax_treatment, Some(TaxTreatment::TaxDeferred));
    assert_eq!(get(&db, id).unwrap().group, Group::Investment);
}

#[test]
fn a_cash_account_may_not_carry_a_tax_treatment() {
    let db = crate::db::open_in_memory().unwrap();
    assert!(
        insert(&db, "CHK", "Everyday", Kind::Cash, 0, Some(TaxTreatment::Taxable)).is_err(),
        "the schema's paired CHECK did not refuse a taxed cash account"
    );
}

#[test]
fn an_investment_account_must_carry_a_tax_treatment() {
    let db = crate::db::open_in_memory().unwrap();
    assert!(
        insert(&db, "RET", "Retirement", Kind::Investment, 0, None).is_err(),
        "the schema's paired CHECK did not refuse an untaxed investment account"
    );
}
```

In `src/db/migration.rs`'s `mod tests` — the data-survival test the rebuild risks:

```rust
#[test]
fn the_account_rebuild_keeps_every_column_and_the_row_id() {
    use crate::db::account::{self, AccountColor, InterestPolicy, Kind};

    let db = crate::db::open_in_memory().unwrap();
    let id = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 3, None).unwrap();
    account::set_color(&db, id, Some(AccountColor::ALL[2])).unwrap();
    account::set_interest_policy(&db, id, InterestPolicy::ProRata).unwrap();

    // Replay the whole chain against the database that already holds the row.
    crate::db::migration::run(&db.conn).unwrap();

    let account = account::get(&db, id).unwrap();
    assert_eq!(account.id, id, "the row id moved, and three tables reference it");
    assert_eq!(account.code, "SAV");
    assert_eq!(account.name, "Rainy Day");
    assert_eq!(account.sort, 3);
    assert_eq!(account.color, Some(AccountColor::ALL[2]));
    assert_eq!(
        account::interest_policy(&db, id).unwrap(),
        InterestPolicy::ProRata
    );
}
```

In `src/overview.rs`'s `mod tests`:

```rust
#[test]
fn an_investment_account_is_not_banded_and_does_not_reach_net() {
    let db = crate::db::open_in_memory().unwrap();
    let everyday = placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
    add(&db, everyday, day(2026, 9, 1), 10_000);
    account::insert(
        &db,
        "RET",
        "Retirement",
        Kind::Investment,
        0,
        Some(account::TaxTreatment::TaxDeferred),
    )
    .unwrap();

    let overview = Overview::load(&db, dates()).unwrap();

    assert!(
        overview.cash.bands.iter().all(|b| b.group != Group::Investment),
        "an investment account was banded on the Overview"
    );
    assert_eq!(overview.net.to_date, Cents(10_000));
}
```

- [ ] **Step 2: Run them to make sure they fail**

```bash
cargo test --lib an_investment_account_carries_a_tax_treatment
```

Expected: FAIL — `no variant named Investment found for enum Kind`.

- [ ] **Step 3: Append arm 12**

The first table rebuild in the chain. SQLite cannot alter a `CHECK`, so the table is recreated.
There is **no `PRAGMA foreign_keys = ON`** in this crate, so the three `REFERENCES account(id)`
declarations elsewhere are not enforced and no pragma dance is needed; `apply` already runs the
whole chain in one transaction.

The arm spells the table as it stands at version 11 — carrying `color` from arm 2 and
`interest_policy` from the baseline — and recreates arm 5's index after the rename.

```rust
    Migration {
        version: 12,
        // Investment accounts join the two kinds already here. A `CHECK` list
        // cannot be altered in SQLite, so the table is rebuilt; `grp` widens
        // with `kind` because a group subdivides exactly one kind and the new
        // kind does not split.
        //
        // `tax_treatment` is paired to the kind rather than left free: it is
        // meaningless on a cash row and required on an investment one, and a
        // paired CHECK is what says so in the one place both columns are
        // visible at once.
        sql: "CREATE TABLE account_new (
                id   INTEGER PRIMARY KEY,
                code TEXT    NOT NULL,
                name TEXT    NOT NULL,
                kind TEXT    NOT NULL CHECK (kind IN ('cash', 'credit', 'investment')),
                grp  TEXT    NOT NULL CHECK (grp IN ('checking', 'savings', 'credit', 'investment')),
                sort INTEGER NOT NULL DEFAULT 0,
                interest_policy TEXT CHECK (interest_policy IN ('pro_rata', 'manual')),
                color TEXT CHECK (color IN
                  ('red','orange','yellow','green','cyan','blue','magenta','white')),
                tax_treatment TEXT CHECK (tax_treatment IN ('taxable','tax_deferred','tax_free')),
                CHECK ((kind = 'investment') = (tax_treatment IS NOT NULL)),
                UNIQUE (code, kind)
              );
              INSERT INTO account_new
                (id, code, name, kind, grp, sort, interest_policy, color, tax_treatment)
                SELECT id, code, name, kind, grp, sort, interest_policy, color, NULL FROM account;
              DROP TABLE account;
              ALTER TABLE account_new RENAME TO account;
              CREATE UNIQUE INDEX account_code_kind ON account (lower(code), kind);",
        data: None,
    },
```

**Before writing this arm, read the `color` `CHECK` list and the `account_code_kind` index
definition out of arms 2 and 5 and copy them verbatim.** The values above are what those arms
are expected to say; if they differ, the arms win — a rebuild that narrows a `CHECK` silently
drops rows.

- [ ] **Step 4: Add the Rust types**

In `src/db/account.rs`:

```rust
pub enum Kind {
    Cash,
    Credit,
    Investment,
}
```

with `ALL: [Kind; 3] = [Kind::Cash, Kind::Credit, Kind::Investment]`, `as_str` gaining
`Kind::Investment => "investment"`, `label` gaining `Kind::Investment => "Investment"`, and
`FromStr` gaining `"investment" => Ok(Kind::Investment)`.

```rust
pub enum Group {
    Checking,
    Savings,
    Credit,
    Investment,
}
```

with `ALL: [Group; 4]`, `as_str`/`label`/`FromStr` extended the same way, `kind()` gaining
`Group::Investment => Kind::Investment`, and `bands(Kind::Investment)` returning
`&[Group::Investment]`.

```rust
/// How the money in an investment account is taxed.
///
/// Carried by every investment account and by no other kind, which the
/// schema's paired `CHECK` is the backstop for. It decides nothing the app
/// computes — it is what the screen groups by when the owner asks where the
/// bonds live, a question about tax drag that no balance answers.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TaxTreatment {
    Taxable,
    TaxDeferred,
    TaxFree,
}

impl TaxTreatment {
    pub const ALL: [TaxTreatment; 3] = [
        TaxTreatment::Taxable,
        TaxTreatment::TaxDeferred,
        TaxTreatment::TaxFree,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaxTreatment::Taxable => "taxable",
            TaxTreatment::TaxDeferred => "tax_deferred",
            TaxTreatment::TaxFree => "tax_free",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TaxTreatment::Taxable => "Taxable",
            TaxTreatment::TaxDeferred => "Tax-deferred",
            TaxTreatment::TaxFree => "Tax-free",
        }
    }
}

impl FromStr for TaxTreatment {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "taxable" => Ok(TaxTreatment::Taxable),
            "tax_deferred" => Ok(TaxTreatment::TaxDeferred),
            "tax_free" => Ok(TaxTreatment::TaxFree),
            other => bail!("unknown tax treatment {other:?}"),
        }
    }
}
```

Add `tax_treatment` to `select_account!`, to `Account`, to `from_row`, and a
`tax_treatment: Option<TaxTreatment>` parameter to `insert`, which continues to write the kind's
`default_group` — now `Group::Investment` for the new kind.

Add the writer:

```rust
/// The one writer of `account.tax_treatment`.
///
/// Separate from `set_group` for the reason `set_interest_policy` is separate
/// from both: the column is the owner's, set on the Accounts screen, and an
/// edit that wrote the whole row would let one field clear another.
pub fn set_tax_treatment(db: &Db, id: AccountId, treatment: TaxTreatment) -> Result<()> {
    let account = get(db, id)?;
    ensure!(
        account.kind == Kind::Investment,
        "{} is not an investment account, so it carries no tax treatment",
        account.name
    );
    db.conn.execute(
        "UPDATE account SET tax_treatment = ?1 WHERE id = ?2",
        params![treatment.as_str(), id],
    )?;
    Ok(())
}
```

- [ ] **Step 5: Exclude investment accounts from the Overview**

`cargo build` now fails at `src/overview.rs:137`, where `match account.kind` is exhaustive. That is
the compiler doing this task's most important work. Filter the accounts before banding rather than
adding an arm that returns zeroes — a zeroed row would still be drawn:

```rust
        let accounts: Vec<_> = account::list(db)?
            .into_iter()
            // Investment accounts are not banded and do not reach Net. Their
            // balance is the sum of the holdings in them, which is not dated,
            // so it would read the same in all three columns of a screen whose
            // whole shape is one widening horizon.
            .filter(|a| a.kind != Kind::Investment)
            .collect();
```

`Group::ALL` now yields `Group::Investment`, whose `lines` will be empty, and the existing
`if lines.is_empty() { continue; }` drops the band. Keep that path rather than special-casing.

- [ ] **Step 6: Audit the bare `account::list` callers**

```bash
grep -rn 'account::list(' --include='*.rs' src/ | grep -v 'list_by_kind'
```

For each site, decide whether it should now see investment accounts. The 35 `list_by_kind` callers
already filter and need no change. Sites that must exclude them: transfer destination pickers, the
ledger's account selector, the worksheet's container list, `account_label::Account::named`'s
callers that build a selectable list. Sites that should include them: the Accounts screen itself.
Record the decision for each in the commit body.

- [ ] **Step 7: Run the tests**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS, including the three new tests and the rebuild's data-survival test.

- [ ] **Step 8: Commit**

Commit with `jluszcz:commit`. Message: `feat(account): investment accounts, banded off the Overview`

---

## Task 6: The `holding` and `fund_mix` tables

**Files:**
- Modify: `src/db/migration.rs` (arm 13), `src/db/mod.rs`, `src/test_support.rs`
- Create: `src/db/holding.rs`, `src/db/fund_mix.rs`

**Interfaces:**
- Consumes: task 5 (head version 12, `Kind::Investment`).
- Produces:
  - `db::HoldingId` in `src/db/id.rs`
  - `db::holding::{Holding, insert, get, list, list_for_account, tickers, update, delete, reorder}`
    where `tickers(db: &Db) -> Result<Vec<String>>` is `SELECT DISTINCT ticker`, which the
    fetcher's refresh reads to learn what to ask SEC about
    where `Holding { id: HoldingId, account_id: AccountId, ticker: String, balance: Cents, sort: i64 }`
  - `db::fund_mix::{Mix, Slice, set_for_ticker, for_ticker}` where
    `Slice { class: AssetClass, weight: BasisPoints }` and
    `Mix { ticker: String, report_date: NaiveDate, slices: Vec<Slice> }`
  - `db::fund_mix::AssetClass` with variants `UsStock, IntlStock, UsBond, IntlBond, Cash,
    Unclassified`, plus `ALL: [AssetClass; 6]`, `as_str`, `label`, `FromStr`
  - `test_support::investment(id: i64, code: &str) -> Account` and the fund vocabulary
  - Schema head version **13**

- [ ] **Step 1: Write the failing tests**

In `src/db/holding.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Kind, TaxTreatment};

    fn account(db: &Db) -> crate::db::AccountId {
        account::insert(db, "RET", "Retirement", Kind::Investment, 0, Some(TaxTreatment::TaxFree))
            .unwrap()
    }

    #[test]
    fn one_ticker_may_be_held_in_two_accounts() {
        let db = crate::db::open_in_memory().unwrap();
        let first = account(&db);
        let second = account::insert(
            &db, "BRK", "Brokerage", Kind::Investment, 1, Some(TaxTreatment::Taxable),
        )
        .unwrap();

        insert(&db, first, "USM", Cents(100_000)).unwrap();
        assert!(
            insert(&db, second, "USM", Cents(250_000)).is_ok(),
            "a ticker held in a second account was refused"
        );
    }

    #[test]
    fn one_ticker_twice_in_one_account_is_refused() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account(&db);

        insert(&db, id, "USM", Cents(100_000)).unwrap();
        assert!(
            insert(&db, id, "USM", Cents(250_000)).is_err(),
            "the same ticker was inserted twice into one account"
        );
    }

    #[test]
    fn a_holding_in_a_cash_account_is_refused() {
        let db = crate::db::open_in_memory().unwrap();
        let cash = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0, None).unwrap();

        assert!(
            insert(&db, cash, "USM", Cents(100_000)).is_err(),
            "a holding was written against a cash account"
        );
    }
}
```

In `src/db/fund_mix.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::day;

    #[test]
    fn a_mix_read_back_is_the_mix_that_was_written() {
        let db = crate::db::open_in_memory().unwrap();
        let slices = vec![
            Slice { class: AssetClass::UsStock, weight: BasisPoints(6_000) },
            Slice { class: AssetClass::IntlStock, weight: BasisPoints(4_000) },
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
            &db, "USM", day(2026, 3, 31),
            &[Slice { class: AssetClass::UsStock, weight: BasisPoints(10_000) }],
        )
        .unwrap();
        set_for_ticker(
            &db, "USM", day(2026, 6, 30),
            &[Slice { class: AssetClass::UsBond, weight: BasisPoints(10_000) }],
        )
        .unwrap();

        let mix = for_ticker(&db, "USM").unwrap().unwrap();
        assert_eq!(mix.slices.len(), 1, "the previous quarter's slices survived");
        assert_eq!(mix.slices[0].class, AssetClass::UsBond);
    }

    #[test]
    fn a_ticker_never_fetched_reads_as_none_rather_than_an_empty_mix() {
        let db = crate::db::open_in_memory().unwrap();
        assert!(for_ticker(&db, "USM").unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib holding::`
Expected: FAIL — `file not found for module holding`.

- [ ] **Step 3: Append arm 13**

```rust
    Migration {
        version: 13,
        // The funds held in each investment account, and the composition of
        // each fund.
        //
        // `UNIQUE (account_id, ticker)`: one ticker twice in one account is a
        // typo, while one ticker held in two accounts is ordinary — the same
        // shape as `UNIQUE (code, kind)` on `account`.
        //
        // `fund_mix` keys on the ticker rather than on a holding, because a
        // fund's composition is a property of the fund and not of who holds
        // it; `report_date` is per ticker because fund families file on their
        // own schedules and a screen has to quote an as-of date per row.
        sql: "CREATE TABLE holding (
                id            INTEGER PRIMARY KEY,
                account_id    INTEGER NOT NULL REFERENCES account(id),
                ticker        TEXT    NOT NULL,
                balance_cents INTEGER NOT NULL,
                sort          INTEGER NOT NULL DEFAULT 0,
                UNIQUE (account_id, ticker)
              );
              CREATE TABLE fund_mix (
                ticker      TEXT    NOT NULL,
                asset_class TEXT    NOT NULL CHECK (asset_class IN
                              ('us_stock','intl_stock','us_bond','intl_bond','cash','unclassified')),
                weight_bp   INTEGER NOT NULL,
                report_date TEXT    NOT NULL,
                PRIMARY KEY (ticker, asset_class)
              );",
        data: None,
    },
```

- [ ] **Step 4: Write the two query modules**

`src/db/holding.rs` follows `src/db/recurring_txn.rs`'s shape: a `select_holding!` macro stating the
columns, `from_row` reading them in that order, and every `SELECT` built from the macro with
`concat!`. `insert` checks the account's kind and refuses a non-investment one with a message naming
the account — the `UNIQUE` constraint is the backstop for the duplicate case, and the kind check is
the Rust guard the schema cannot express here.

`src/db/fund_mix.rs` follows the same shape. `set_for_ticker` deletes the ticker's rows and inserts
the new ones — **not** through `Db::transaction`, so a caller refreshing many tickers composes them
into one atomic refresh under a single caller-owned transaction, exactly as `txn::write_transfer`
does for a payday's two legs. Say that in its doc comment.

- [ ] **Step 5: Wire the modules and the preserved list**

In `src/db/mod.rs`, add `pub mod holding;` and `pub mod fund_mix;`, add `HoldingId` to
`src/db/id.rs` and re-export it, and extend `PRESERVED_TABLES`:

```rust
#[cfg(test)]
const PRESERVED_TABLES: &[&str] = &["account", "recurring_txn", "holding", "fund_mix"];
```

Extend `IMPORTED_TABLES`' doc comment to say why the two new tables are exempt: the workbook carries
neither, so a replace has nothing to say about them.

- [ ] **Step 6: Extend the fixture vocabulary**

In `src/test_support.rs`, beside `cash` and `credit`:

```rust
/// An investment account: `BRK`, `RET`, `ROTH` or `HSA`.
///
/// Named from the same table `cash` and `credit` read, so a fixture takes a
/// code and gets a name rather than restating the pairing. No name here is
/// `Taxable`, `Investment` or any other word `TaxTreatment::label` or
/// `Group::label` already prints.
pub fn investment(id: i64, code: &str) -> Account { /* mirror `cash` */ }
```

with the vocabulary table gaining: `BRK` → `Holdings`, `RET` → `Long Haul`, `ROTH` → `Untaxed Pot`,
`HSA` → `Health Pot`. Add the fund vocabulary as a second table, which the follow-on plan's
classifier tests read:

| Ticker | Fund name |
|---|---|
| `TDF45` | `Target 2045 Fund` |
| `TDF35` | `Target 2035 Fund` |
| `USM` | `Total Market Index Fund` |
| `ISM` | `International Stock Index Fund` |
| `USB` | `Total Bond Index Fund` |
| `ISB` | `International Bond Index Fund` |
| `UNC` | `Overseas Growth Fund` |

- [ ] **Step 7: Run the tests**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS, including
`every_table_the_schema_creates_is_either_cleared_or_deliberately_kept`.

- [ ] **Step 8: Commit**

Commit with `jluszcz:commit`. Message: `feat(db): holdings and the fund mixes they resolve through`

---

## Task 7: The Accounts screen creates investment accounts

**Files:**
- Modify: `src/tui/accounts.rs`, `src/tui/app/accounts.rs`, `src/tui/CLAUDE.md`

**Interfaces:**
- Consumes: tasks 5 and 6.
- Produces: no new public API. `a` on screen 9 writes an investment account with a tax treatment.

- [ ] **Step 1: Write the failing test**

In `src/tui/app/accounts.rs`'s `mod tests`:

```rust
#[test]
fn creating_an_investment_account_asks_for_a_tax_treatment() {
    let mut app = test_support::app();
    app.screen = Screen::Accounts;
    app.key(KeyCode::Char('a'));

    let form = app.account_form().expect("the add form is open");
    walk_until!(form.kind == Kind::Investment, form.next_kind());

    assert!(
        app.account_form().unwrap().shows_tax_treatment(),
        "the tax-treatment field is hidden on an investment account"
    );
}

#[test]
fn the_tax_treatment_field_is_hidden_on_a_cash_account() {
    let mut app = test_support::app();
    app.screen = Screen::Accounts;
    app.key(KeyCode::Char('a'));

    let form = app.account_form().expect("the add form is open");
    assert_eq!(form.kind, Kind::Cash, "the selector does not open on Cash");
    assert!(!form.shows_tax_treatment());
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib creating_an_investment_account_asks`
Expected: FAIL — no method `shows_tax_treatment`.

- [ ] **Step 3: Add the field to the form**

In `src/tui/accounts.rs`, add a `tax_treatment: TaxTreatment` field to the add form's state and a
`shows_tax_treatment(&self) -> bool` returning `self.kind == Kind::Investment`. The field
participates in `next_field`/`prev_field` only when shown — a hidden field the caret can land on is
a field the owner cannot see they are editing.

Cycle it with `←`/`→` like every other selector, per the key vocabulary.

- [ ] **Step 4: Pass it through the commit**

In `src/tui/app/accounts.rs`, pass `Some(form.tax_treatment)` to `account::insert` when the kind is
`Investment` and `None` otherwise. The paired `CHECK` is the backstop; this form is the guard.

- [ ] **Step 5: Document the divergence**

In `src/tui/CLAUDE.md`'s screen-9 paragraph, state that `a` asks the kind and, for an investment
account, the tax treatment — and why the field is conditional.

- [ ] **Step 6: Run the tests**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 7: Commit**

Commit with `jluszcz:commit`. Message: `feat(tui): create investment accounts on the Accounts screen`

---

## Task 8: The Funds screen shows holdings

**Files:**
- Modify: `src/tui/fund.rs`, `src/tui/app/funds.rs`, `src/tui/help.rs`, `src/tui/CLAUDE.md`

**Interfaces:**
- Consumes: tasks 6 and 7.
- Produces: `tui::fund::Funds` with `set_rows(&mut self, rows: Vec<Row>)` and the key handler below.
  The follow-on plan adds `g`/`G` to the same handler.

**Keys**, all from the existing vocabulary in `src/tui/CLAUDE.md`:

| Key | Does |
|---|---|
| `a` / `e` / `d` | add, edit, delete a holding — account, ticker, balance |
| `Tab` / `BackTab` | cycle the account filter: All, then one per investment account |
| `/` | filter by typing, over ticker and account |

`g`/`G` and `Enter` arrive with the fetcher and are **not** bound here — a key that does nothing is
worse than a key that is absent, because the owner cannot tell it from one that failed.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_holding_with_no_mix_on_record_reads_as_never_fetched() {
    let mut app = test_support::app_with_holdings();
    app.screen = Screen::Funds;
    app.reload().unwrap();

    let row = app.funds().rows().first().expect("a holding is listed");
    assert_eq!(row.as_of, None);
    assert_eq!(row.stock_percent, None, "a fund with no mix reported a stock share");
}

#[test]
fn the_account_filter_narrows_the_list_to_one_account() {
    let mut app = test_support::app_with_holdings();
    app.screen = Screen::Funds;
    app.reload().unwrap();
    let all = app.funds().rows().len();

    app.key(KeyCode::Tab);

    let filtered = app.funds().rows().len();
    assert!(filtered < all, "Tab did not narrow the list");
    assert!(
        app.funds().rows().iter().all(|r| r.account_id == app.funds().filter_account().unwrap()),
        "a row from another account survived the filter"
    );
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --lib a_holding_with_no_mix_on_record`
Expected: FAIL — no method `app_with_holdings`.

- [ ] **Step 3: Add the fixture**

In `src/tui/app/test_support.rs`, add `app_with_holdings()` building on the existing fixture app:
two investment accounts from `test_support::investment`, three holdings across them from the fund
vocabulary, and no `fund_mix` rows at all. The absence is the point — it is what the first test
asserts against.

- [ ] **Step 4: Write the view state**

`Row` carries `account_id: AccountId`, `account: account_label::Account`, `ticker: String`,
`balance: Cents`, `stock_percent: Option<BasisPoints>`, `as_of: Option<NaiveDate>`. Both `Option`s
are `None` exactly when no `fund_mix` row exists for the ticker — **zero and unknown are different
states and neither may be spelled as the other.**

- [ ] **Step 5: Lay out the table**

Per the width rules in `src/tui/CLAUDE.md`: `Account` takes the single `Constraint::Min`; ticker,
balance, stock percentage and the as-of date are `Constraint::Length` sized to their true content.
`tui::GUTTER` comes out of the `Min` column.

```rust
let widths = [
    Constraint::Min(20),     // Account — absorbs all slack, incl. GUTTER
    Constraint::Length(8),   // Ticker
    Constraint::Length(14),  // Balance
    Constraint::Length(14),  // Mix bar
    Constraint::Length(7),   // Stock%
    Constraint::Length(11),  // As of
];
```

- [ ] **Step 6: Add the width test**

Follow the existing per-screen width tests: assert the layout holds at `tui::MIN_WIDTH`, writing
`MIN_WIDTH` and never `120`, and derive any pinned column offset from it.

- [ ] **Step 7: Restore the help topic**

In `src/tui/help.rs`, give screen 6 entries for `a`/`e`/`d`, `Tab` and `/`. Quote single-character
keys as `'a'`. Keep each entry within
`help::tests::no_panel_entry_runs_longer_than_a_glance`'s eight wrapped lines.

- [ ] **Step 8: Run the tests**

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 9: Commit**

Commit with `jluszcz:commit`. Message: `feat(tui): the Funds screen lists holdings by account`

---

## Task 9: Documentation

**Files:**
- Modify: `CLAUDE.md` (root), `src/tui/CLAUDE.md`, `src/calc/CLAUDE.md`, `src/import/CLAUDE.md`

**Interfaces:**
- Consumes: tasks 1–8.

- [ ] **Step 1: Update the root architecture table**

Remove the `src/db/fund.rs` and `src/import/fund.rs` rows. Restate the two that survive:

```markdown
| `src/calc/fund.rs` | The target rule: bonds track age one point a year over thirty, and the equity remainder splits by one configured share. No database. |
| `src/fund.rs` | Reads the birth date and the equity split out of `db`, feeds `calc::fund`. The one place a stored birth date becomes the bond target every screen measures against. |
```

Add:

```markdown
| `src/db/holding.rs` | The `holding` table — a fund held in an investment account, and the balance the owner typed. |
| `src/db/fund_mix.rs` | The `fund_mix` table — one fund's composition by asset class, as of the filing it was read from. |
```

- [ ] **Step 2: Restate the fund invariants**

Delete the bullet on the `fund` table being imported and `--replace` overwriting hand-typed values —
the table is gone. **Keep the bullet on a fund's target percentage being derived rather than
stored**, and restate it for the new shape:

```markdown
- **The allocation target is derived, never stored.** The bond target is `(age - 30)` points and a
  birthday moves it with no write, so storing it would go stale in the night. What is stored is the
  birth date and one split — `key::INTL_EQUITY_SHARE`, the international share of the equity
  remainder — and `calc::fund::targets` turns the pair into three percentages on every read. **An
  age row with no birth date on record claims nothing**, so the two equity targets divide the whole
  100% rather than being told a bond target that is really a question.
- **One key holds the equity split, not two.** Domestic is the remainder. The importer reads
  `Planning!J3` and `J4` and stores their ratio, because two stored values for one fact can
  disagree — the same reason only `Constants!G2` is imported for the pay cadence while `H2` is
  merely asserted against it.
```

Add:

```markdown
- **An investment account is banded off the Overview, and its balance is not a `SUM(cents)`.**
  `kind = 'investment'` means an account the Overview skips: `overview::load` filters them before
  banding, so Net keeps meaning spendable net worth derived from the dated ledger. Their balance is
  the sum of the `holding` rows beneath them, which carries no date — pinned into the Overview it
  would read the same in all three columns of a screen whose whole shape is one widening horizon.
  What reusing `account` buys is naming, colors, ordering and the Accounts screen, deliberately not
  the balance model.
- **`account.tax_treatment` is present exactly when the kind is `investment`**, which the schema's
  paired `CHECK` is the backstop for and the Accounts screen's conditional field is the guard.
  `account::set_tax_treatment` is its one writer, for the reason `set_interest_policy` is its
  column's.
- **`holding` and `fund_mix` are in `PRESERVED_TABLES`**, and the reason is uniform: the workbook
  carries neither, so a `--replace` has nothing to say about them.
```

- [ ] **Step 3: State that this feature has no workbook oracle**

In the root `CLAUDE.md`, under "The workbook is the test oracle", add a sentence: the holdings
model is not in the workbook, so `tests/` carries no binary for it and the coverage is unit tests
against invented fixtures. Stated because a reader arriving at that section will look for the file
task 3 deleted.

- [ ] **Step 4: Update the module docs**

`src/calc/CLAUDE.md` loses the fund derivation. `src/import/CLAUDE.md` loses `Planning!I1:M5` and
`Constants!K2` (done in task 3 — verify it stuck). `src/tui/CLAUDE.md`'s "Which module is which
screen" paragraph restates screen 6.

- [ ] **Step 5: Verify no stale references survive**

```bash
# These must be GONE.
grep -rn 'db::fund\b\|FundId\|fund::Target\|fund::Rule\|Planning!I1' \
  --include='*.rs' --include='*.md' src/ tests/ CLAUDE.md

# These must still be PRESENT — the rule survived the table.
grep -rn 'BIRTH_DATE' --include='*.rs' src/ | grep -v emergency
grep -rn 'BONDS_START_AGE\|INTL_EQUITY_SHARE' --include='*.rs' src/
```

Expected: no output from the first; matches from both of the others. A clean first grep with an
empty second one means the rule was deleted along with its table, which is the mistake this step
exists to catch.

- [ ] **Step 6: Final gate**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<path> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test --features import
```

Expected: all pass.

- [ ] **Step 7: Commit**

Commit with `jluszcz:commit`. Message: `docs: state the holdings model and what it replaced`
