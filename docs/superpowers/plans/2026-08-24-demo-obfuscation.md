# Demo Obfuscation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `mm --demo`'s fixed block mask with a per-run obfuscation — every digit of an absolute figure becomes another digit, every owner-entered word becomes a same-length pronounceable pseudoword — and compile the whole of it only under a new non-default `demo` Cargo feature.

**Architecture:** One per-run salt lives in the thread-local `src/demo/` already uses. `src/demo/mask.rs` (feature-gated) holds two pure rules: a digit scramble keyed on `(salt, the Cents value, the digit's position relative to the decimal point)`, and a pseudoword keyed on `(salt, the lowercased word)`. `src/demo/mod.rs` keeps today's public API with two bodies, so no call site changes when the feature is off. Strings reach the mask through two existing seams (`account_label::Account::render_with`, `description::render`) plus a `demo::text` at each remaining **render** site — never where a screen builds its view-state rows, because forms prefill from those.

**Tech Stack:** Rust 2024, ratatui, rusqlite, `std::hash::{DefaultHasher, RandomState}` (no new dependency).

**Spec:** `docs/superpowers/specs/2026-08-24-demo-obfuscation-design.md`

## Global Constraints

- **No real data in any tracked file.** Every money literal, account code, institution and goal name in this repository is invented. Fixtures use the vocabulary in `src/test_support::{cash, credit}`.
- **No floats.** `Cents(i64)` is the only money type.
- **`rusqlite` is named only inside `src/db/`; `ratatui`/`crossterm` only inside `src/tui/`.** `crate::demo` sits at the crate root and may name neither.
- **Test names are full sentences describing the scenario**, e.g. `a_demo_scrambles_every_digit_of_a_figure`. Unit tests live in `mod tests` at the bottom of the file under test.
- **Never disable or delete a failing test to make a suite pass.** Rewrite the assertion to the new truth or fix the code.
- **Documentation describes the code as it is**, never how it got there. No "changed to…", "previously…".
- **`cargo fmt` runs as a pre-commit hook; CI treats clippy warnings as errors.** Every task ends green under `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings`.
- **Both feature states must pass.** `cargo test` (feature off) and `cargo test --features demo` (feature on) are two separate gates, and every task runs both.
- **Commit on the current feature branch** (`DemoTweak`), never on `main`. No `Co-Authored-By` trailers, no reference to AI authorship.
- The workbook-oracle tests are untouched by this work — nothing here reaches `src/import/` or `src/calc/` — so `MM_REQUIRE_WORKBOOK=1` runs are not required.

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Declares `[features] demo = []`. |
| `src/demo/mod.rs` | The public API — `install`, `figure`, `whole_figure`, `typed`, `text` — with a real body and a pass-through body. Holds the salt. Was `src/demo.rs`. |
| `src/demo/mask.rs` | Feature-gated. The two pure rules: `scramble` (digits) and `text` (pseudowords), plus the hashing under both. Knows nothing about screens. |
| `src/account_label.rs` | `render_with` gains the mask, covering every account name and code in the app. |
| `src/description.rs` | `render` gains the mask and returns `Cow<'_, str>`. |
| `src/transfer.rs` | `diagnose` and `unclaimed_by_container` mask the goal and account names they write into prose. |
| `src/tui/*.rs`, `src/tui/app/*.rs` | Render sites and status lines that name a goal, a bill, a fund or a recurring row. |
| `src/bin/mm.rs` | The `--demo` flag, feature-gated, behind `Cli::demo()`. |
| `.github/workflows/ci.yml` | A second job with `all-features: true`. |
| `README.md`, `CLAUDE.md`, `src/tui/CLAUDE.md`, `src/report/CLAUDE.md` | The behaviour and the invariants that change. |

---

## Task 1: The `demo` feature gate

Behaviour is unchanged by this task — the block mask still blocks. What changes is that it compiles only when asked for.

**Files:**
- Modify: `Cargo.toml`
- Move: `src/demo.rs` → `src/demo/mod.rs`
- Modify: `src/bin/mm.rs:26` (the flag), `:96-97`, `:119`
- Modify: every file holding a `crate::demo::install(true)` test — `src/transfer.rs`, `src/tui/{form,fund,goal_form,overview,planning,recurring_goal,recurring_txn,savings,search,worksheet}.rs`, `src/tui/app/{mod,planning}.rs`
- Modify: `.github/workflows/ci.yml`, `README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the `demo` cargo feature; `Cli::demo(&self) -> bool` in `src/bin/mm.rs`; `crate::demo`'s public API unchanged (`install(bool)`, `figure(Cents) -> String`, `whole_figure(Cents) -> String`, `typed(&str) -> String`).

- [ ] **Step 1: Declare the feature**

In `Cargo.toml`, after the `[dependencies]` block:

```toml
[features]
# Demo mode: `mm --demo`, and the obfuscation behind it. Off by default so a
# shipped binary carries neither the flag nor the code it installs.
demo = []
```

- [ ] **Step 2: Move the module**

```bash
mkdir -p src/demo && git mv src/demo.rs src/demo/mod.rs
```

- [ ] **Step 3: Split every body in `src/demo/mod.rs`**

Replace the `DEMO` thread-local, `install`, `enabled`, `figure`, `whole_figure`, `typed` and `mask_or` with the following. `MASK` and the doc comments above each function stay as they are for now.

```rust
#[cfg(feature = "demo")]
thread_local! {
    /// Whether this run is a demo.
    ///
    /// A thread-local rather than an `AtomicBool` because the app draws on
    /// one thread and the test suite does not: `cargo test` runs each test on
    /// its own thread, so a test that turns the mask on cannot be seen by the
    /// hundreds that assert real figures.
    static DEMO: Cell<bool> = const { Cell::new(false) };
}

/// Make this run a demo. Called once, before anything draws.
///
/// A no-op without the `demo` feature, which is what lets `tui::run` keep one
/// signature and every caller below it stay unaware the feature exists.
pub fn install(on: bool) {
    #[cfg(feature = "demo")]
    DEMO.set(on);
    #[cfg(not(feature = "demo"))]
    let _ = on;
}

/// Whether figures are being blocked out.
#[cfg(feature = "demo")]
pub(crate) fn enabled() -> bool {
    DEMO.get()
}

/// A figure with its cents: `1,234.56`, or the mask.
pub(crate) fn figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if enabled() {
        return masked(cents);
    }
    cents.to_string()
}

/// The same figure with the cents dropped -- see [`Cents::to_whole_dollars`].
pub(crate) fn whole_figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if enabled() {
        return masked(cents);
    }
    cents.to_whole_dollars()
}

/// What a form shows in a field holding an amount.
pub(crate) fn typed(raw: &str) -> String {
    #[cfg(feature = "demo")]
    if enabled() && !raw.is_empty() {
        return MASK.to_string();
    }
    raw.to_string()
}

/// The mask, signed as the figure is.
///
/// The sign survives because [`crate::tui::style::amount_color`] still sees
/// the real figure and still paints a negative red: dropping the `-` would
/// hide the glyph and leave the fact.
#[cfg(feature = "demo")]
fn masked(cents: Cents) -> String {
    let sign = if cents < Cents::ZERO { "-" } else { "" };
    format!("{sign}{MASK}")
}
```

Put `#[cfg(feature = "demo")]` on the `MASK` constant and on `use std::cell::Cell;` as well, since neither is reachable without it.

- [ ] **Step 4: Gate the module's own tests**

Change the test module header in `src/demo/mod.rs` to:

```rust
#[cfg(all(test, feature = "demo"))]
mod tests {
```

and leave every test in it as it stands.

- [ ] **Step 5: Gate every other demo test in the crate**

```bash
grep -rn "crate::demo::install(true)" --include='*.rs' src
```

Put `#[cfg(feature = "demo")]` directly above the `#[test]` of every function that call appears in — 31 of them across `src/transfer.rs`, `src/tui/`, and `src/tui/app/`. Two helpers used only by those tests need the same attribute: `DEMO_FIXTURE_FIGURES` in `src/tui/app/mod.rs:1293`, and any `use` that becomes unused (the compiler will name it).

- [ ] **Step 6: Gate the flag**

In `src/bin/mm.rs`, put `#[cfg(feature = "demo")]` above the `#[arg(long)]` on the `demo` field, and add beside the `Cli` impl:

```rust
impl Cli {
    /// Whether this run is a demo. Always `false` without the `demo`
    /// feature, where there is no flag to read: a build that cannot install
    /// the mask must not have a way to ask for it.
    #[cfg(feature = "demo")]
    fn demo(&self) -> bool {
        self.demo
    }

    #[cfg(not(feature = "demo"))]
    fn demo(&self) -> bool {
        false
    }
}
```

Two whole definitions rather than one body with a `cfg`'d `return` in it: a
`return` followed by a tail expression is unreachable code under one of the two
builds, and CI treats that warning as an error.

Change the three readers — `tui::run(db, today, cli.demo)`, `write_report(&db, &cfg, today, cli.demo)` and `if cli.demo` — to `cli.demo()`. The `mm report` refusal stays exactly as it is: without the feature it is unreachable rather than absent, which keeps one code path for the message.

- [ ] **Step 7: Cover both builds in CI**

In `.github/workflows/ci.yml`, add a second job beside `ci`:

```yaml
  ci-all-features:
    uses: jluszcz/github-utils/.github/workflows/rust-ci.yml@v2
    with:
      runs-on: ubuntu-24.04-arm
      target: aarch64-unknown-linux-musl
      all-features: true
```

The shared workflow's concurrency group already includes its inputs, so the two calls do not cancel each other.

- [ ] **Step 8: Say so in the README**

In `README.md`'s Demo mode section, immediately under the `mm --demo` fence:

```markdown
Demo mode is a build-time feature, off by default:

```bash
cargo install --path . --features demo
```

A default build has no `--demo` flag, because it carries none of the code
behind it.
```

- [ ] **Step 9: Verify both builds**

```bash
cargo fmt
cargo test
cargo test --features demo
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: all five pass. `cargo test` runs a smaller suite than `cargo test --features demo` — that difference is the feature working.

- [ ] **Step 10: Verify the flag is gone from a default build**

```bash
cargo run --quiet --bin mm -- --demo --help 2>&1 | head -3
cargo run --quiet --features demo --bin mm -- --help 2>&1 | grep -- --demo
```

Expected: the first prints clap's `unexpected argument '--demo'`; the second prints the flag's help line.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat(demo): put demo mode behind a non-default feature" -m "The flag and everything it installs now compile only under --features demo, so a shipped binary carries neither. CI covers both builds."
```

---

## Task 2: The salt and the digit rule

Pure functions with unit tests. Nothing is wired to them yet, so the app still draws blocks at the end of this task.

**Files:**
- Create: `src/demo/mask.rs`
- Modify: `src/demo/mod.rs` (the salt, and `mod mask;`)

**Interfaces:**
- Consumes: `crate::money::Cents` (`Cents(i64)`, `Cents::to_whole_dollars() -> String`, `Display`, `FromStr`).
- Produces:
  - `crate::demo::mask::scramble(salt: u64, key: i64, text: &str) -> String`
  - `crate::demo::mask::figure(salt: u64, cents: Cents) -> String`
  - `crate::demo::mask::whole_figure(salt: u64, cents: Cents) -> String`
  - `crate::demo::mask::typed(salt: u64, raw: &str) -> String`
  - `crate::demo::mask::hashed(salt: u64, key: i64, position: i32) -> u64`
  - `crate::demo::salt() -> Option<u64>` (feature-gated, module-private)
  - `crate::demo::install_with_salt(salt: u64)` (`#[cfg(all(test, feature = "demo"))]`, `pub(crate)`)

- [ ] **Step 1: Write the failing tests**

Create `src/demo/mask.rs` with only this test module in it:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Ten thousand and a cent must not read differently in kind -- what
    /// changes is every digit, and nothing else in the string.
    #[test]
    fn a_scrambled_figure_keeps_its_punctuation_and_loses_its_digits() {
        let drawn = figure(7, Cents(123_456));
        assert_eq!(drawn.len(), "1,234.56".len());
        assert_eq!(drawn.matches(',').count(), 1);
        assert_eq!(drawn.matches('.').count(), 1);
        assert_ne!(drawn, "1,234.56");
        assert!(drawn.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.'));
    }

    /// The colour beside the figure already says it is negative, so the sign
    /// stays: dropping it would hide the glyph and leave the fact.
    #[test]
    fn a_scrambled_figure_keeps_the_sign_a_colour_would_have_given_away() {
        assert!(figure(7, Cents(-123_456)).starts_with('-'));
        assert!(whole_figure(7, Cents(-123_456)).starts_with('-'));
    }

    /// The Planning screen draws whole dollars and the ledger draws cents.
    /// One balance quoted twice must be one balance.
    #[test]
    fn whole_dollars_are_the_same_figure_with_its_cents_dropped() {
        let with_cents = figure(7, Cents(123_456));
        let whole = whole_figure(7, Cents(123_456));
        assert_eq!(with_cents.split('.').next().unwrap(), whole);
    }

    /// Every screen quoting one amount quotes one scrambled amount.
    #[test]
    fn one_amount_scrambles_the_same_way_every_time() {
        assert_eq!(figure(7, Cents(123_456)), figure(7, Cents(123_456)));
    }

    /// Not a digit-for-digit substitution: one figure learned tells nothing
    /// about the next.
    #[test]
    fn two_amounts_sharing_a_digit_do_not_share_its_replacement() {
        let a = figure(7, Cents(111_111));
        let b = figure(7, Cents(211_111));
        assert_ne!(a[2..], b[2..], "{a} and {b} scrambled in lockstep");
    }

    /// The salt is what a screenshot does not carry.
    #[test]
    fn two_salts_scramble_one_amount_differently() {
        assert_ne!(figure(7, Cents(123_456)), figure(8, Cents(123_456)));
    }

    /// `0,834` reads as a rendering fault rather than as money.
    #[test]
    fn a_multi_digit_figure_never_scrambles_to_a_leading_zero() {
        for cents in 0..500i64 {
            let drawn = figure(cents as u64, Cents(123_456));
            assert!(!drawn.starts_with('0'), "{drawn}");
        }
    }

    /// Nothing about a real figure survives, including that it was nothing.
    #[test]
    fn a_zero_scrambles_like_any_other_figure() {
        assert_ne!(figure(7, Cents::ZERO), "0.00");
    }

    /// A form prefilled from a row shows that row's own scrambled figure:
    /// a form disagreeing with the row it opened on reads as a bug.
    #[test]
    fn a_typed_figure_agrees_with_the_row_it_was_prefilled_from() {
        assert_eq!(typed(7, "1,234.56"), figure(7, Cents(123_456)));
    }

    /// Half-typed text is not a figure and has no value to key on, but it is
    /// still digits on a screen.
    #[test]
    fn text_that_is_not_yet_a_figure_still_loses_its_digits() {
        let drawn = typed(7, "12.");
        assert_eq!(drawn.len(), 3);
        assert!(drawn.ends_with('.'));
        assert_ne!(drawn, "12.");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features demo --lib demo::mask`
Expected: FAIL — `cannot find function figure in this scope`, and `src/demo/mask.rs` is not yet a module.

- [ ] **Step 3: Write the implementation**

Above the test module in `src/demo/mask.rs`:

```rust
//! What a demo draws in place of a figure.
//!
//! Two rules, both pure and both keyed on the run's salt. This module is
//! compiled only under the `demo` feature; [`super`] is the API that stands
//! in for it when it is not.
//!
//! **A digit is keyed on the value and its position relative to the decimal
//! point**, which is what makes a demo coherent rather than noisy: one
//! amount draws the same on every screen, `figure` and `whole_figure` agree
//! about the dollars they share, and two amounts an order of magnitude apart
//! are unrelated rather than one substitution away from each other.
//!
//! It is obfuscation for a demonstration, not a security control. The salt
//! never leaves the process and a screenshot does not carry it; nothing more
//! than that is claimed, and nothing should be built on it.

use crate::money::Cents;
use std::hash::{DefaultHasher, Hash, Hasher};

/// A figure with its cents, every digit replaced.
pub(super) fn figure(salt: u64, cents: Cents) -> String {
    scramble(salt, cents.0, &cents.to_string())
}

/// The same figure with the cents dropped.
pub(super) fn whole_figure(salt: u64, cents: Cents) -> String {
    scramble(salt, cents.0, &cents.to_whole_dollars())
}

/// What a form shows in a field holding an amount.
///
/// Text that parses is keyed on the value it parses to, so a field prefilled
/// from a row draws that row's own scrambled figure. Text that does not parse
/// is keyed on itself: it is not a figure yet, and there is no value to agree
/// with.
pub(super) fn typed(salt: u64, raw: &str) -> String {
    let key = match raw.parse::<Cents>() {
        Ok(cents) => cents.0,
        Err(_) => hashed_text(salt, raw) as i64,
    };
    scramble(salt, key, raw)
}

/// Every ASCII digit in `text` replaced, everything else left where it is.
///
/// Positions are counted from the decimal point -- the ones digit is 0 and
/// the first cents digit is -1 -- so two renderings of one value agree about
/// every digit they both draw. The leading digit of a multi-digit integer
/// part is drawn from `1..=9`, because `0,834` reads as a rendering fault
/// rather than as money.
pub(super) fn scramble(salt: u64, key: i64, text: &str) -> String {
    let digits = text.chars().filter(char::is_ascii_digit).count();
    let after_point = match text.rfind('.') {
        Some(point) => text[point..].chars().filter(char::is_ascii_digit).count(),
        None => 0,
    };
    let whole = (digits - after_point) as i32;

    let mut out = String::with_capacity(text.len());
    let mut seen = 0i32;
    for c in text.chars() {
        if !c.is_ascii_digit() {
            out.push(c);
            continue;
        }
        let h = hashed(salt, key, whole - 1 - seen);
        let d = if seen == 0 && whole > 1 {
            1 + h % 9
        } else {
            h % 10
        };
        out.push(char::from(b'0' + d as u8));
        seen += 1;
    }
    out
}

/// The run's randomness, mixed with what is being drawn.
pub(super) fn hashed(salt: u64, key: i64, position: i32) -> u64 {
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    key.hash(&mut h);
    position.hash(&mut h);
    h.finish()
}

/// The same, for text that has no value behind it.
pub(super) fn hashed_text(salt: u64, text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}
```

Note `char::is_ascii_digit` takes `&char`, which is what `filter` hands it.

- [ ] **Step 4: Hold the salt in `src/demo/mod.rs`**

Replace the `DEMO` thread-local and `install`/`enabled` from Task 1 with:

```rust
#[cfg(feature = "demo")]
mod mask;

#[cfg(feature = "demo")]
thread_local! {
    /// This run's salt, and whether it is a demo at all.
    ///
    /// `None` *is* "not a demo", so one cell carries both facts and there is
    /// no state where the mask is on with nothing behind it.
    ///
    /// A thread-local rather than an `AtomicU64` because the app draws on one
    /// thread and the test suite does not: `cargo test` runs each test on its
    /// own thread, so a test that turns the mask on cannot be seen by the
    /// hundreds that assert real figures.
    static SALT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Make this run a demo. Called once, before anything draws.
pub fn install(on: bool) {
    #[cfg(feature = "demo")]
    SALT.set(on.then(draw_salt));
    #[cfg(not(feature = "demo"))]
    let _ = on;
}

/// This run's salt, or `None` when this is not a demo.
#[cfg(feature = "demo")]
fn salt() -> Option<u64> {
    SALT.get()
}

/// A salt for this run, from the standard library's own randomly seeded
/// hasher.
///
/// `rand` would be a whole dependency for one `u64` drawn once, and this is
/// obfuscation rather than key material.
#[cfg(feature = "demo")]
fn draw_salt() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one("mm --demo")
}

/// A demo whose salt is known, so a test can assert an exact output.
///
/// Test-only for the reason the salt is drawn per run: a fixed salt is a
/// mapping that survives from one run to the next, which is the one property
/// the drawn salt exists to deny.
#[cfg(all(test, feature = "demo"))]
pub(crate) fn install_with_salt(salt: u64) {
    SALT.set(Some(salt));
}
```

Everything that read `enabled()` now reads `salt()`; keep `figure`, `whole_figure` and `typed` drawing `MASK` for one more task by matching on `salt()`:

```rust
pub(crate) fn figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if salt().is_some() {
        return masked(cents);
    }
    cents.to_string()
}
```

with the same shape for `whole_figure` and `typed`, and delete `enabled`.

- [ ] **Step 5: Fix the module's own tests**

`an_ordinary_run_is_not_a_demo` in `src/demo/mod.rs` calls `enabled()`. Rewrite it against the salt:

```rust
/// Whether the app is demonstrating itself is a fact about the run, and
/// every formatting function reads the same answer.
#[test]
fn an_ordinary_run_has_no_salt_and_a_demo_has_one() {
    assert!(salt().is_none());
    install(true);
    assert!(salt().is_some());
}

/// The salt is what a screenshot does not carry, so it cannot be a constant.
#[test]
fn two_runs_draw_two_salts() {
    assert_ne!(draw_salt(), draw_salt());
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test --features demo --lib demo
```

Expected: PASS, including all eleven new `mask` tests.

- [ ] **Step 7: Verify both builds**

```bash
cargo fmt
cargo test
cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: all pass. `mask` is dead code from the app's point of view, but it is `pub(super)` and exercised by its own tests, so clippy is quiet.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(demo): the salt and the digit rule" -m "A digit is keyed on the run's salt, the value, and the digit's position relative to the decimal point, so one amount draws the same everywhere and whole dollars agree with the figure they came from. Not yet wired to a screen."
```

---

## Task 3: Figures scramble

The swap. `MASK` goes, and the ~30 assertions that name it become assertions about the real figure being absent.

**Files:**
- Modify: `src/demo/mod.rs`
- Modify: `src/transfer.rs:1461-1467`, `src/tui/{form,fund,goal_form,overview,planning,recurring_goal,recurring_txn,savings,worksheet}.rs`, `src/tui/app/{mod,planning}.rs` — the test assertions listed by the grep in step 4

**Interfaces:**
- Consumes: `mask::{figure, whole_figure, typed}` from Task 2.
- Produces: `crate::demo::{figure, whole_figure, typed}` scrambling rather than blocking. No signature changes.

- [ ] **Step 1: Write the failing tests**

Rewrite the four mask tests in `src/demo/mod.rs`'s test module to the new truth, and add one for the sign:

```rust
#[test]
fn an_ordinary_run_prints_the_figure_itself() {
    assert_eq!(figure(Cents(123_456)), "1,234.56");
    assert_eq!(whole_figure(Cents(123_456)), "1,234");
    assert_eq!(typed("1,234.56"), "1,234.56");
}

#[test]
fn a_demo_scrambles_every_digit_of_a_figure() {
    install_with_salt(7);
    assert_ne!(figure(Cents(123_456)), "1,234.56");
    assert_eq!(figure(Cents(123_456)), mask::figure(7, Cents(123_456)));
    assert_eq!(whole_figure(Cents(123_456)), mask::whole_figure(7, Cents(123_456)));
}

#[test]
fn a_demo_keeps_the_sign_a_colour_would_have_given_away_anyway() {
    install_with_salt(7);
    assert!(figure(Cents(-123_456)).starts_with('-'));
    assert!(whole_figure(Cents(-123_456)).starts_with('-'));
}

/// A field with nothing in it has no figure to hide, and scrambling it
/// would say the opposite -- that something is there.
#[test]
fn a_demo_leaves_an_empty_field_empty() {
    install_with_salt(7);
    assert_eq!(typed(""), "");
    assert_ne!(typed("12.50"), "12.50");
}
```

Delete `a_demo_blocks_a_cent_and_a_million_identically`: it asserted the property this feature deliberately gives up, and the spec's first decision says so. Replace it with the property that replaces it:

```rust
/// The width a figure draws at is the width it would have drawn at, which
/// is what lets every column stay where it was laid out.
#[test]
fn a_scrambled_figure_is_as_wide_as_the_figure_it_replaces() {
    install_with_salt(7);
    assert_eq!(figure(Cents(1)).len(), "0.01".len());
    assert_eq!(figure(Cents(100_000_000)).len(), "1,000,000.00".len());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --features demo --lib demo::tests`
Expected: FAIL — `assert_ne!` fires because `figure` still returns `██████`.

- [ ] **Step 3: Wire the three functions**

In `src/demo/mod.rs`, delete `MASK` and `masked`, and route each function into `mask`:

```rust
pub(crate) fn figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::figure(salt, cents);
    }
    cents.to_string()
}

pub(crate) fn whole_figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::whole_figure(salt, cents);
    }
    cents.to_whole_dollars()
}

pub(crate) fn typed(raw: &str) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        if !raw.is_empty() {
            return mask::typed(salt, raw);
        }
    }
    raw.to_string()
}
```

Rewrite the module doc's second paragraph to describe scrambling rather than blocking, keeping the "absolute figures only" paragraph as it is.

- [ ] **Step 4: Run to verify they pass, then find the fallout**

```bash
cargo test --features demo --lib demo::tests
grep -rn "██████" --include='*.rs' src
```

Expected: the `demo` tests PASS; the grep lists 25 assertions in 11 other files.

- [ ] **Step 5: Rewrite each surviving mask assertion**

Each falls into one of three shapes. Apply the matching rewrite:

*Shape A — `assert!(text.contains("██████"))`, an "it was blocked" check.* Replace with the absence of the real figure the test set up. For example in `src/tui/overview.rs:260`:

```rust
assert!(!text.contains("1,000.00"), "the balance survived: {text:?}");
```

*Shape B — `assert_eq!(form.display(F::Amount).plain_text(), "██████")`.* Replace with a pair, since equality against a scrambled figure needs the salt the test installed:

```rust
crate::demo::install_with_salt(7);
// …
let drawn = form.display(FundField::Actual).plain_text();
assert_ne!(drawn, "300.00");
assert_eq!(drawn.len(), "300.00".len());
```

*Shape C — a whole rendered line, as in `src/tui/planning.rs:2690`.* Assert the wording that is not a figure, and the absence of the one that is:

```rust
let line = /* … as the test already builds it … */;
assert!(line.starts_with("pinned "));
assert!(line.contains(" on 2026-08-14 · excess has since moved "));
assert!(!line.contains("1,200"), "{line}");
```

Every test touched here must call `crate::demo::install_with_salt(7)` instead of `crate::demo::install(true)`, so a scrambled figure is the same scrambled figure on every run and a failure is reproducible. Update `a_demo_leaves_no_figure_on_any_screen`'s positive check the same way — it can no longer look for `██████`:

```rust
assert!(
    screen == '9' || drawn.contains(&crate::demo::figure(Cents::from_dollars(1_000))),
    "screen {screen} drew no scrambled figure, so the check above passed for free:\n{drawn}"
);
```

- [ ] **Step 6: Run the whole suite**

```bash
cargo test --features demo
```

Expected: PASS. If a test fails on a figure that was never in `DEMO_FIXTURE_FIGURES`, that is a real leak — fix the call site rather than the assertion.

- [ ] **Step 7: See it**

```bash
cargo run --features demo --bin mm -- --demo
```

Expected: every screen draws figures of the right width in the right columns, none of them real; percentages, dates and names are untouched. Quit with `q`.

- [ ] **Step 8: Verify both builds**

```bash
cargo fmt && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(demo): scramble a figure's digits rather than blocking them" -m "Every absolute figure draws at its own width with none of its own digits. What this gives up, deliberately, is the order of magnitude the fixed-width mask hid."
```

---

## Task 4: The pseudoword rule

Pure, unit-tested, and called by nothing. The app is unchanged at the end of this task.

**Files:**
- Modify: `src/demo/mask.rs`, `src/demo/mod.rs`

**Interfaces:**
- Consumes: `mask::{hashed, hashed_text}` from Task 2.
- Produces:
  - `crate::demo::mask::text(salt: u64, s: &str) -> String`
  - `crate::demo::text(s: &str) -> Cow<'_, str>` — borrowed when this is not a demo

- [ ] **Step 1: Write the failing tests**

Add to `src/demo/mask.rs`'s test module:

```rust
/// A column is laid out for the name it draws, so the replacement is as
/// wide as what it replaces.
#[test]
fn a_pseudonym_is_as_long_as_the_name_it_replaces() {
    for name in ["Everyday", "Rainy Day", "CHK", "Home Down Payment", "a"] {
        assert_eq!(text(7, name).chars().count(), name.chars().count(), "{name}");
    }
}

/// Spaces and punctuation are the shape of a name rather than part of it.
#[test]
fn a_pseudonym_keeps_the_spaces_and_punctuation_around_its_words() {
    let drawn = text(7, "CHK — Everyday");
    assert_eq!(drawn.matches(' ').count(), 2);
    assert!(drawn.contains('—'));
    assert_eq!(text(7, "—"), "—");
    assert_eq!(text(7, "Mom & Dad").matches('&').count(), 1);
}

/// `Rainy Day` and `Rainy Fund` still share a word, so a demo reads as one
/// person's accounts rather than as noise.
#[test]
fn one_word_reads_the_same_way_everywhere_in_a_run() {
    let day = text(7, "Rainy Day");
    let fund = text(7, "Rainy Fund");
    assert_eq!(
        day.split(' ').next().unwrap(),
        fund.split(' ').next().unwrap()
    );
}

/// A name is not its own pseudonym, and a screenshot does not carry the
/// salt that made one.
#[test]
fn a_pseudonym_is_neither_the_name_nor_the_same_under_another_salt() {
    assert_ne!(text(7, "Everyday"), "Everyday");
    assert_ne!(text(7, "Everyday"), text(8, "Everyday"));
}

/// A code is recognisable as a code and a year as a year: what a character
/// *is* survives, and only which one it is changes.
#[test]
fn a_pseudonym_keeps_case_and_keeps_a_digit_a_digit() {
    assert!(text(7, "CHK").chars().all(|c| c.is_ascii_uppercase()));
    let lego = text(7, "Lego 2026");
    let (word, year) = lego.split_once(' ').unwrap();
    assert!(word.starts_with(|c: char| c.is_uppercase()));
    assert!(word[1..].chars().all(|c| c.is_lowercase()));
    assert!(year.chars().all(|c| c.is_ascii_digit()));
    assert_ne!(year, "2026");
}

/// Pronounceable is the whole point: a demo is read aloud across a table.
#[test]
fn a_pseudonym_alternates_consonants_and_vowels() {
    let drawn = text(7, "Everyday").to_lowercase();
    for (i, c) in drawn.chars().enumerate() {
        let vowel = VOWELS.contains(&(c as u8));
        assert_eq!(vowel, i % 2 == 1, "{drawn} breaks at {i}");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --features demo --lib demo::mask`
Expected: FAIL — `cannot find function text in this scope`.

- [ ] **Step 3: Write the implementation**

In `src/demo/mask.rs`, above the tests:

```rust
/// The letters a pseudoword alternates between. Fourteen consonants that
/// read cleanly beside any vowel, and the five vowels.
const CONSONANTS: [u8; 14] = *b"bdfgklmnprstvz";
const VOWELS: [u8; 5] = *b"aeiou";

/// Every word in `s` replaced by a pseudoword of the same length.
///
/// A *word* is a run of alphanumerics; everything between them -- spaces,
/// `—`, `/`, `&`, punctuation -- is the shape of the name rather than part of
/// it, and passes through where it stands. That is also what leaves
/// [`crate::description::render`]'s em dash alone.
pub(super) fn text(salt: u64, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            word.push(c);
            continue;
        }
        out.push_str(&pseudoword(salt, &word));
        word.clear();
        out.push(c);
    }
    out.push_str(&pseudoword(salt, &word));
    out
}

/// One word, keyed on itself.
///
/// Keying on the lowercased word rather than on the whole string is what
/// makes the screens hang together: the same word reads the same way
/// wherever it appears in the run, so an account named in a title and in a
/// ledger row is recognisably the same account.
///
/// A digit stays a digit and a letter stays a letter, so a code still reads
/// as a code and a year as a year. Case is copied character by character,
/// which is what keeps `CHK` shouting and `Everyday` capitalised.
fn pseudoword(salt: u64, word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    let key = hashed_text(salt, &word.to_lowercase()) as i64;
    word.chars()
        .enumerate()
        .map(|(i, c)| {
            let h = hashed(salt, key, i as i32);
            if c.is_numeric() {
                return char::from(b'0' + (h % 10) as u8);
            }
            let letters: &[u8] = if i % 2 == 0 { &CONSONANTS } else { &VOWELS };
            let letter = char::from(letters[(h % letters.len() as u64) as usize]);
            match c.is_uppercase() {
                true => letter.to_ascii_uppercase(),
                false => letter,
            }
        })
        .collect()
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --features demo --lib demo::mask`
Expected: PASS.

- [ ] **Step 5: Add the crate-root API**

In `src/demo/mod.rs`, beside `figure`:

```rust
/// What a screen draws in place of a name the owner typed.
///
/// Borrowed when this is not a demo, which is what keeps it callable in a
/// draw path that has a `&str` and wants one back.
///
/// **Called where text becomes a `Label` or a `Cell`, never where a screen
/// builds its rows.** A form prefills from the row a screen is holding --
/// `App::open_goal_edit` hands `GoalForm` the selected row's own name -- so a
/// pseudonym written into view state is a pseudonym `Enter` would commit.
pub(crate) fn text(s: &str) -> Cow<'_, str> {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return Cow::Owned(mask::text(salt, s));
    }
    Cow::Borrowed(s)
}
```

with `use std::borrow::Cow;` at the top of the module.

- [ ] **Step 6: Test the API's two states**

Add to `src/demo/mod.rs`'s test module:

```rust
#[test]
fn an_ordinary_run_prints_the_name_itself() {
    assert_eq!(text("Rainy Day"), "Rainy Day");
    assert!(matches!(text("Rainy Day"), Cow::Borrowed(_)));
}

#[test]
fn a_demo_replaces_a_name_with_a_pseudonym_of_its_own_length() {
    install_with_salt(7);
    assert_eq!(text("Rainy Day"), mask::text(7, "Rainy Day"));
    assert_ne!(text("Rainy Day"), "Rainy Day");
}
```

- [ ] **Step 7: Verify both builds**

```bash
cargo fmt && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS. `text` is unused outside its tests until Task 5, so add `#[allow(dead_code)]` **only** if clippy objects, and remove it in Task 5.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(demo): a name's pseudonym, keyed on the word" -m "Same length, same case, same punctuation, a digit still a digit -- and the same word reads the same way all run, so a demo looks like one person's accounts rather than noise."
```

---

## Task 5: Accounts and descriptions

Two seams cover four of the seven text columns. This is where names start disappearing from the screens.

**Files:**
- Modify: `src/account_label.rs` (`render_with`)
- Modify: `src/description.rs` (`render`)
- Modify: `src/tui/ledger.rs:493`, `src/tui/app/ledger.rs:211,269,299`, `src/report/html/ledger.rs:58` — the `render` callers
- Modify: `src/tui/destination.rs` (`Offered.container`), `src/tui/goal_form.rs` (`AllocationForm::container_name` via `unallocated_line`), `src/tui/planning.rs` (the Destinations block's `transfer::Container` name), `src/tui/accounts.rs` (the `Code` column) — the four account displays outside `Account`, listed in `src/tui/CLAUDE.md`
- Modify: `src/tui/app/accounts.rs:35,160` — the two status lines naming an account
- Test: `src/tui/app/mod.rs` (the screen sweep)

**Interfaces:**
- Consumes: `crate::demo::text` from Task 4.
- Produces: `crate::description::render(raw: &str) -> Cow<'_, str>` (was `-> &str`); `Account::render_with` handing its callback an obfuscated `&str` under a demo.

- [ ] **Step 1: Write the failing test**

Add to `src/tui/app/mod.rs`'s test module, beside the figure sweep:

```rust
/// Every name and description `app_with_two_rows_on_every_list` puts in the
/// database, as an ordinary run would print it.
///
/// `Paycheck` and `Transfer` are deliberately absent: each is also one of
/// the app's own words -- a Planning row, a form title -- so an absence
/// check on either would fail on vocabulary rather than on a leak.
const DEMO_FIXTURE_NAMES: [&str; 18] = [
    "Everyday", "Rainy Day", "Card One", "Card Two", "CHK", "SAV", "CC1", "CC2", "Whole Foods",
    "Rent", "Movies", "Batteries", "Vacation 2027", "Couch", "Mortgage", "Gym", "Bonds",
    "Domestic",
];

/// The net for names, and the Accounts screen is in it: it draws no figure
/// at all, which is exactly why the figure sweep exempts it and this one
/// must not.
///
/// A fixed salt rather than a drawn one, so a failure is reproducible and a
/// pseudonym cannot collide its way to a pass on one run in a thousand.
#[test]
fn a_demo_leaves_no_name_on_any_screen() {
    crate::demo::install_with_salt(7);
    let mut app = app_with_two_rows_on_every_list();
    for screen in "123456789".chars() {
        press(&mut app, KeyCode::Char(screen));
        let drawn = drawn(&mut app);
        for name in DEMO_FIXTURE_NAMES {
            assert!(
                !drawn.contains(name),
                "{name} survived on screen {screen}:\n{drawn}"
            );
        }
    }
}

/// The other half of the sweep above: an absence check passes for free over
/// a screen that drew nothing, so every screen must be shown to have drawn
/// a pseudonym for an account it holds.
#[test]
fn every_screen_draws_the_pseudonym_of_an_account_it_holds() {
    crate::demo::install_with_salt(7);
    let mut app = app_with_two_rows_on_every_list();
    let everyday = crate::demo::text("Everyday").to_string();
    let chk = crate::demo::text("CHK").to_string();
    for screen in "1234789".chars() {
        press(&mut app, KeyCode::Char(screen));
        let drawn = drawn(&mut app);
        assert!(
            drawn.contains(&everyday) || drawn.contains(&chk),
            "screen {screen} named no account, so the sweep passed for free:\n{drawn}"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --features demo --lib a_demo_leaves_no_name_on_any_screen`
Expected: FAIL — `Everyday survived on screen 1`.

- [ ] **Step 3: Mask the account seam**

In `src/account_label.rs`, `render_with` becomes:

```rust
pub fn render_with<T>(&self, f: impl FnOnce(&str, AccountColor) -> T) -> T {
    let text = crate::demo::text(&self.text);
    f(
        &text,
        self.color.unwrap_or_else(|| AccountColor::derived(self.id)),
    )
}
```

Add a paragraph to its doc comment: this is the one place an account's text is read, so it is the one place a demo has to reach to cover every account display in the app.

- [ ] **Step 4: Mask the description seam**

In `src/description.rs`:

```rust
/// The text to draw for a stored description, `—` when there is none.
///
/// A demo replaces the text and leaves the em dash alone: an absence is not
/// something to hide, and a row with nothing in its Description column reads
/// the same in a demonstration as it does in an ordinary run.
pub fn render(raw: &str) -> Cow<'_, str> {
    if raw.trim().is_empty() {
        return Cow::Borrowed("—");
    }
    crate::demo::text(raw)
}
```

with `use std::borrow::Cow;`. Fix the four callers: `src/tui/ledger.rs:493` already calls `.to_string()`; `src/tui/app/ledger.rs:211,269,299` interpolate through `format!`, which `Cow` satisfies; `src/report/html/ledger.rs:58` passes into `escape`, which takes `&str`, so it becomes `escape(&description::render(&r.description))`. Add to that module's existing tests:

```rust
#[cfg(feature = "demo")]
#[test]
fn a_demo_replaces_a_description_and_leaves_an_absence_alone() {
    crate::demo::install_with_salt(7);
    assert_ne!(render("Whole Foods"), "Whole Foods");
    assert_eq!(render(""), "—");
}
```

- [ ] **Step 5: Mask the four account displays outside `Account`**

These are listed in `src/tui/CLAUDE.md`'s account-color section as the entire set of account displays that do not go through `Account`, so they are the entire set the seam in step 3 misses. Wrap each in `crate::demo::text` where it becomes a cell:

- `src/tui/destination.rs`, the `Cell::from(container.clone())` in `render` — `Cell::from(crate::demo::text(&container).into_owned())`
- `src/tui/goal_form.rs`, `unallocated_line`'s use of `container_name`
- `src/tui/planning.rs`, the Destinations block row built from `transfer::Container`'s name
- `src/tui/accounts.rs`, the bare `Cell` holding the `Code` column

- [ ] **Step 6: Mask the two status lines that name an account**

`src/tui/app/accounts.rs:35` and `:160`:

```rust
self.status = format!("{} added", crate::demo::text(new.name.as_str()));
self.status = format!("{} saved", crate::demo::text(edit.name.as_str()));
```

- [ ] **Step 7: Run to verify it passes**

```bash
cargo test --features demo --lib a_demo_leaves_no_name_on_any_screen
cargo test --features demo --lib every_screen_draws_the_pseudonym
```

Expected: the second PASSES; the first still FAILS, now on a goal or bill name (`Vacation 2027`, `Mortgage`) rather than on an account. That is Task 6's work — leave the test failing and **do not commit a failing suite**: mark the first test `#[ignore = "goal, bill, fund and recurring names land in the next task"]` with that exact reason, and remove the attribute in Task 6.

- [ ] **Step 8: Verify both builds**

```bash
cargo fmt && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(demo): accounts and descriptions through the seams that already exist" -m "render_with is the only reader of an account's text and description::render the only rule for a description, so one line each covers every screen that draws one -- plus the four account displays CLAUDE.md already lists as outside that guarantee."
```

---

## Task 6: Goal, bill, fund and recurring names on the screens

**Files:**
- Modify: `src/tui/savings.rs:333` (goal name cell), `src/tui/fund.rs:342` (fund name cell), `src/tui/recurring_goal.rs:341` (name cell), `src/tui/recurring_txn.rs:369` (description cell), `src/tui/worksheet.rs:616` (line name cell), `src/tui/picker.rs:171` (entry name cell), `src/tui/planning.rs:565-573` (the `bills` closure)
- Test: `src/tui/app/mod.rs`, `src/tui/savings.rs`

**Interfaces:**
- Consumes: `crate::demo::text`.
- Produces: no new API. The screens draw pseudonyms; their view-state rows keep the real text.

- [ ] **Step 1: Un-ignore the sweep and run it**

Remove the `#[ignore]` added in Task 5 step 7.

Run: `cargo test --features demo --lib a_demo_leaves_no_name_on_any_screen`
Expected: FAIL — `Vacation 2027 survived on screen 4`.

- [ ] **Step 2: Mask the six name cells**

Each is a `Cell::from(<name>.clone())` in a render function. The pattern:

```rust
// src/tui/savings.rs, in the row's cells
Cell::from(crate::demo::text(&r.name).into_owned()),
```

Apply the same to `src/tui/fund.rs:342` (`r.name`), `src/tui/recurring_goal.rs:341` (`r.name`), `src/tui/recurring_txn.rs:369` (`r.description`), `src/tui/worksheet.rs:616` (`line.name`) and `src/tui/picker.rs:171` (`entry.name`).

**Not** the `matcher.matches(&row.name, …)` calls a few lines above several of them: a needle is matched against the stored text so `/` narrows a list by the owner's own vocabulary, exactly as it matches a real amount today.

- [ ] **Step 3: Mask the bill labels**

The Planning screen's rows come from `plan_rows::rows`, whose labels are the app's own words *except* for the bills fed in. `src/tui/planning.rs`'s `bills` closure is where the owner's text enters that list, and where the screen may mask it without touching the shared list the report also reads:

```rust
Ok(plan_rows::Bill {
    id: b.id,
    label: crate::demo::text(&b.label).into_owned(),
    monthly: b.cents,
    biweekly: calc::biweekly(b.cents, periods)?,
})
```

This is safe for the same reason the goal name is not: `App::open_bill_edit` re-reads the row with `bill::get` rather than taking the screen's copy.

- [ ] **Step 4: Run the sweep**

Run: `cargo test --features demo --lib a_demo_leaves_no_name_on_any_screen`
Expected: PASS.

- [ ] **Step 5: Write the test that pins the needle**

In `src/tui/savings.rs`'s test module:

```rust
/// A needle is matched against the stored text, not against what the screen
/// drew: a demo narrows a list by the owner's own vocabulary, exactly as it
/// does by a real amount.
#[cfg(feature = "demo")]
#[test]
fn a_demo_still_finds_a_goal_by_its_real_name() {
    crate::demo::install_with_salt(7);
    let mut savings = /* the fixture this module's other filter tests build */;
    savings.filter("Couch");
    assert_eq!(names(&savings), ["Couch"]);
}
```

Build the fixture the way the neighbouring filter test in that module does, and assert against `savings.rows()` — view state, which holds the real name.

- [ ] **Step 6: Run it**

Run: `cargo test --features demo --lib a_demo_still_finds_a_goal_by_its_real_name`
Expected: PASS.

- [ ] **Step 7: See it**

```bash
cargo run --features demo --bin mm -- --demo
```

Expected: screens 1, 4, 6, 7, 8 draw pseudonyms in every name column, at the same widths, with the Planning bill rows renamed and its line labels (`Bills`, `Future Housing`, `Excess (Actual)`) untouched.

- [ ] **Step 8: Verify both builds**

```bash
cargo fmt && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(demo): goal, bill, fund and recurring names on the screens" -m "Masked where a name becomes a cell rather than where a screen builds its rows: a form prefills from those rows, so a pseudonym in view state is a pseudonym Enter would commit. The / needle goes on matching the stored text."
```

---

## Task 7: Forms, status lines, confirmations and prose

The layer the screen sweep cannot see, and the one where a mistake would reach a write.

**Files:**
- Modify: `src/tui/goal_form.rs` (`GoalField::Name` in `display`), `src/tui/fund.rs:225` (`FundField::Name`), `src/tui/recurring_goal.rs:243` (`RecurringGoalField::Name`), `src/tui/recurring_txn.rs:224` (`RecurringTxnField::Description`), `src/tui/planning.rs:965` (`BillField::Label`), `src/tui/form.rs` (`TxnField::Description`, and `render_popup`'s `s.description`)
- Modify: `src/tui/app/savings.rs:159,176,215`, `src/tui/app/funds.rs:73,97`, `src/tui/app/recurring.rs:77,88,108,148,205,220`, `src/tui/app/planning.rs:335,387`, `src/tui/app/ledger.rs:208`, `src/tui/app/worksheet.rs:242`
- Modify: `src/transfer.rs` — `unclaimed_by_container` (goal names and the container's account name)
- Test: `src/tui/app/mod.rs`, `src/tui/goal_form.rs`, `src/transfer.rs`

**Interfaces:**
- Consumes: `crate::demo::text`.
- Produces: no new API.

- [ ] **Step 1: Write the failing tests**

The safety test first, in `src/tui/app/mod.rs`:

```rust
/// The one thing a demo must never do: reach a write.
///
/// `App::open_goal_edit` prefills the form from the *row the screen is
/// holding*, so a pseudonym written into view state is a pseudonym `Enter`
/// commits. The field draws one and the buffer holds the name.
#[cfg(feature = "demo")]
#[test]
fn a_demo_draws_a_pseudonym_in_a_name_field_and_commits_the_name() {
    crate::demo::install_with_salt(7);
    let mut app = app();
    press(&mut app, KeyCode::Char('4'));
    press(&mut app, KeyCode::Char('e'));
    let Some(Modal::Goal(form)) = &app.modal else {
        panic!("e opened nothing: {}", app.status);
    };
    let drawn = form.display(GoalField::Name).plain_text();
    assert_ne!(drawn, "Vacation 2027");
    assert_eq!(drawn.chars().count(), "Vacation 2027".chars().count());
    assert_eq!(form.commit().unwrap().name, "Vacation 2027");
}
```

Then the form sweep, beside `a_demo_leaves_no_figure_on_any_form_a_row_opens` and built from it — the same `(screen, key)` table, the same fixtures, the same "a modal actually opened" guard, asserting names instead of figures:

```rust
/// The name sweep one layer in. A form prefills from the row it opens on,
/// so a form is where a real name is most likely to reach the screen, not
/// least -- which is how `BillField::Amount` came to be the one amount
/// field the figures first missed.
#[cfg(feature = "demo")]
#[test]
fn a_demo_leaves_no_name_on_any_form_a_row_opens() {
    crate::demo::install_with_salt(7);
    for (screen, key) in [
        ('2', 'a'), ('2', 'e'), ('2', 'r'), ('2', 't'),
        ('4', 'e'), ('4', 'a'), ('4', 'A'), ('4', 'n'),
        ('5', 'e'), ('5', 'E'), ('5', 'a'),
        ('6', 'e'), ('6', 'E'),
        ('7', 'a'), ('8', 'a'), ('9', 'e'),
    ] {
        let mut app = match screen {
            '6' => app_with_two_rows_on_every_list(),
            _ => planning_app(),
        };
        press(&mut app, KeyCode::Char(screen));
        match (screen, key) {
            ('5', _) => select_first_bill(&mut app),
            ('2', 'r') => press(&mut app, KeyCode::Tab),
            _ => {}
        }
        press(&mut app, KeyCode::Char(key));
        assert!(
            app.modal.is_some(),
            "{key} opened nothing on screen {screen}, so the check below \
             would pass without a form ever being drawn: {}",
            app.status
        );
        let drawn = drawn(&mut app);
        // `planning_app`'s own names, and the fixture names for screen 6.
        let names: &[&str] = match screen {
            '6' => &DEMO_FIXTURE_NAMES,
            _ => &["Everyday", "Rainy Day", "Brokerage", "Mortgage", "HOA", "Coworking",
                   "Bill Payments", "Home Down Payment", "Emergency Savings", "Dropbox"],
        };
        for name in names {
            assert!(
                !drawn.contains(name),
                "{name} survived {key} on screen {screen}:\n{drawn}"
            );
        }
    }
}
```

And the prose, in `src/transfer.rs`'s test module, extending the existing demo test there:

```rust
/// `diagnose` writes goal and account names into prose for the Planning
/// screen to draw, which is why the mask lives at the crate root rather
/// than under `tui`.
#[cfg(feature = "demo")]
#[test]
fn a_demo_names_no_goal_in_the_prose_it_writes() {
    crate::demo::install_with_salt(7);
    // the fixture the neighbouring ambiguous-plug test builds
    let text = diagnose(&db, &lines).unwrap().join("\n");
    assert!(!text.contains("Lego"), "{text}");
    assert!(!text.contains("Brokerage"), "{text}");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --features demo --lib a_demo_draws_a_pseudonym_in_a_name_field
cargo test --features demo --lib a_demo_leaves_no_name_on_any_form
cargo test --features demo --lib a_demo_names_no_goal_in_the_prose
```

Expected: all three FAIL, each naming the text that survived.

- [ ] **Step 3: Mask the six name fields**

Each form's `display(field)` is where a field becomes a `Label`, which is where `demo::typed` already masks an amount — and never `Field::given`, which is what commits. Six sites:

```rust
GoalField::Name => crate::demo::text(self.name.value()).into_owned(),
```

and the same shape for `FundField::Name`, `RecurringGoalField::Name`, `RecurringTxnField::Description`, `BillField::Label` and `TxnField::Description`.

- [ ] **Step 4: Mask the autocomplete popup**

`src/tui/form.rs`'s `render_popup` builds each line from `s.description`:

```rust
TextLine::from(format!(
    "{marker} {}   {}   ×{}",
    crate::demo::text(&s.description),
    crate::demo::figure(s.cents),
    s.uses
)),
```

The popup's *suggestions* keep their real text: `Tab` accepts one into the buffer, and the buffer is what commits.

- [ ] **Step 5: Mask the status lines and confirmation labels**

Every `format!` in the file list above that interpolates a goal name, a bill label, a fund name or a recurring description wraps it in `crate::demo::text`. For example:

```rust
// src/tui/app/savings.rs
self.status = format!("updated {}", crate::demo::text(&edit.name));
// src/tui/app/planning.rs, the delete confirmation
let label = format!(
    "{}  {}",
    crate::demo::text(&found.label),
    crate::demo::figure(found.cents)
);
```

`src/tui/app/planning.rs:360`'s `edit.label` is a **Planning line label** — the app's own word — and stays as it is.

- [ ] **Step 6: Mask the prose**

In `src/transfer.rs`'s `unclaimed_by_container`:

```rust
let names: Vec<String> = goals
    .iter()
    .filter(|g| g.container_account_id == id)
    .map(|g| crate::demo::text(&g.name).into_owned())
    .collect();
// …
out.push(format!(
    "  {}: {count}{listed}",
    crate::demo::text(account::get(db, id)?.name.as_str())
));
```

- [ ] **Step 7: Run the three tests, then the suite**

```bash
cargo test --features demo --lib a_demo_
cargo test --features demo
cargo test
```

Expected: PASS throughout.

- [ ] **Step 8: Drive it**

```bash
cargo run --features demo --bin mm -- --demo
```

Open a goal on Savings with `e`, confirm the Name field draws a pseudonym, press Enter, and confirm the row's real name is unchanged on the screen behind — the row still reads as its own pseudonym rather than a new one, because the write put back the name the buffer held.

- [ ] **Step 9: Verify both builds**

```bash
cargo fmt && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(demo): names in forms, status lines and prose" -m "Masked where a field becomes a Label and never where Field::given is handed its text, so what a form draws is a pseudonym and what Enter commits is the name. A form sweep and the goal form's own test hold that shut."
```

---

## Task 8: Documentation

**Files:**
- Modify: `README.md` (Demo mode), `CLAUDE.md` (the `mm --demo` invariant), `src/tui/CLAUDE.md` (the masking invariants), `src/report/CLAUDE.md` (the wording about where the mask lives)

**Interfaces:** none.

- [ ] **Step 1: Rewrite the README's Demo mode section**

Keep the feature build note from Task 1. Replace the prose about blocks with what the run now does: every absolute dollar figure drawn with another figure's digits, every account name, goal name, bill label, fund name and transaction description drawn as a same-length pronounceable pseudoword, with one word reading the same way all run. Say plainly what is given up — a figure's order of magnitude and a name's length are visible, and derived figures no longer reconcile — and what is deliberately left alone: percentages, dates, counts, and the app's own words. Keep the two paragraphs that still hold: what is typed still parses, and `mm report` refuses the flag.

- [ ] **Step 2: Rewrite the root `CLAUDE.md` bullet**

`**`mm --demo` blocks the figures and nothing else.**` is no longer true. One statement of the rule, in the shape the file uses: what a demo replaces (absolute figures, owner-entered text), what it does not (percentages, dates, counts, the app's own vocabulary, every match key), that it is display-only and cannot reach a write, and that it compiles only under the `demo` feature. Point at `src/tui/CLAUDE.md` for where a caller says whether its figure is money, as the bullet already does.

- [ ] **Step 3: Rewrite `src/tui/CLAUDE.md`'s invariants**

Three changes, each a statement of the rule as it now stands:

- "**Percentages, dates, counts and names are never masked**" becomes a statement that names *are* replaced and the other three are not, with the reason: a percentage is a shape rather than a sum, a scrambled date is not a date, and a count over a list of rows the reader can see would read as a fault.
- The fixed-width-mask paragraph — six blocks fitting the narrowest money column — is replaced by the simpler fact that a scrambled figure and a pseudonym are as wide as what they replace, so a demo draws in an ordinary run's widths.
- Add the rule Task 6 and Task 7 turn on: **text is masked where it becomes a `Label` or a `Cell`, never where a screen builds its rows**, because a form prefills from those rows — `App::open_goal_edit` is the example — and the two sweeps that hold it shut.

Update the account-color section's four-item residual list to say that each of those four displays reaches the mask on its own, since none of them goes through `Account`.

- [ ] **Step 4: Touch `src/report/CLAUDE.md`**

The report still never runs under a demo. What changes is the sentence explaining why the page formats `Cents` directly: it is the *screens* that install the mask, and the mask is now feature-gated too.

- [ ] **Step 5: Check the docs against the code**

```bash
grep -rn "██████\|blocks every\|blocked out" README.md CLAUDE.md src/*/CLAUDE.md
```

Expected: no hits. Any survivor is prose describing a mask that no longer exists.

- [ ] **Step 6: Verify**

```bash
cargo fmt --check && cargo test && cargo test --features demo
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "docs: what a demo replaces, and what it deliberately does not" -m "The masking invariants in tui/CLAUDE.md, the root bullet, and the README's Demo mode section now describe the obfuscation and say plainly what it gives up: magnitude, name length, and reconciliation."
```

---

## Self-Review

**Spec coverage.** Feature gate → Task 1 (with CI). Salt → Task 2. Digit rule, cross-format consistency, leading digit, zeros, `typed` keying → Tasks 2–3. Pseudowords, case, digits-in-words, per-word determinism → Task 4. The two seams → Task 5. The four residual account displays → Task 5. Goal/recurring/bill/fund names → Task 6. Forms, status lines, confirmations, autocomplete, `diagnose` prose → Task 7. Search matching stored text → Task 6 step 5. Writes never seeing a pseudonym → Task 7 step 1. Both sweeps asserting something arrived → Task 5 step 1. Docs → Task 8. Out-of-scope items (newtypes, a reproducible salt, the report) appear in no task, as intended.

**Type consistency.** `mask::scramble(u64, i64, &str) -> String`, `mask::figure/whole_figure(u64, Cents) -> String`, `mask::typed(u64, &str) -> String`, `mask::text(u64, &str) -> String`, `mask::hashed(u64, i64, i32) -> u64`, `mask::hashed_text(u64, &str) -> u64`, `demo::text(&str) -> Cow<'_, str>`, `description::render(&str) -> Cow<'_, str>`, `demo::install_with_salt(u64)` — each used under exactly that name and signature everywhere it appears.

**Known sequencing wrinkle.** Task 5 leaves `a_demo_leaves_no_name_on_any_screen` `#[ignore]`d with the reason written into the attribute; Task 6 step 1 removes it. That is the one place a task ships a test that does not yet pass, and it ships green because the attribute is explicit about why.
