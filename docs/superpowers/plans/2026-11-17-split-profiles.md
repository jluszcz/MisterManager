# Split Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Named sets of the three Planning split percentages, switched live with `←`/`→` on the Split block, with `a`/`e`/`d` contextual there and `e` the one edit key on Planning.

**Architecture:** A new `split_profile` table (preserved across `--replace`) replaces the three `SPLIT_*` setting keys; `plan::settings_from_db` reads the active row into the unchanged `PlanSettings`. `plan_rows` names the profile on the Split heading, which becomes an editable row (`Target::SplitProfile`), and the Planning screen's key handler branches on whether the cursor is in the Split block.

**Tech Stack:** Rust, rusqlite (only in `src/db/`), ratatui/crossterm (only in `src/tui/`), `finance-utils` migrations.

**Spec:** `docs/superpowers/specs/2026-11-17-split-profiles-design.md`

## Global Constraints

- `rusqlite` only inside `src/db/`; every `SELECT` of a row built from a `select_split_profile!` macro; a `get` by id errors on a missing row.
- Schema change is a new arm in `db::migration::MIGRATIONS` (version 23); `schema.sql` is not edited.
- New table gets an id type via `row_id!(SplitProfileId, "split_profile")` in `src/db/id.rs`.
- `split_profile` goes in `PRESERVED_TABLES` (`src/db/mod.rs`).
- Profile names are owner-entered text: drawn through `crate::demo::text` on the screen; edit buffers keep the real text.
- No real data in fixtures: profile names in tests are invented (`Contributing`, `Maxed`), percentages are round numbers.
- Test names are full sentences; walks use `walk_until!`.
- Doc comments describe the code as it is; module `AGENTS.md` files and the root `AGENTS.md` updated where this change makes them wrong.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and the same with `--all-features` pass; `cargo test` and `cargo test --features import` pass.
- No commits unless the owner asks; when asked, use the `jluszcz:commit` skill.

## Review Focus

1. **A database written before this change** — the migration must seed `Default` from the three stored keys (or the defaults 30/20/10 when unset), clamp any out-of-range stored share into `0..=100` rather than fail the open, and delete the three keys. Test in Task 1.
2. **`mm import --replace` on a database with several profiles** — profiles survive, and the sheet's percentages land in the *active* one only. Test in Task 2.
3. **`←`/`→` with one profile** — no write, status says there is only one and that `a` adds another. Test in Task 4.
4. **Deleting the active profile** — the next profile by id (else the previous) becomes active, and the waterfall immediately reads it. Test in Task 1 (db) and Task 4 (screen).
5. **`a`/`d` on a non-Split row still add/delete bills; `E` does nothing.** Test in Task 4.

---

### Task 1: The `split_profile` table and its query module

**Files:**
- Modify: `src/db/migration.rs` (arm 23 + data fn `seed_split_profile`)
- Modify: `src/db/id.rs` (`row_id!(SplitProfileId, "split_profile")`)
- Create: `src/db/split_profile.rs`
- Modify: `src/db/mod.rs` (`pub mod split_profile;`, re-export `SplitProfileId`, add to `PRESERVED_TABLES`)

**Interfaces:**
- Produces:
  - `pub struct Shares { pub future_housing: BasisPoints, pub retirement: BasisPoints, pub investment: BasisPoints }` (`Copy, Clone, Debug, PartialEq, Eq`), with `pub fn total(self) -> BasisPoints`.
  - `pub struct SplitProfile { pub id: SplitProfileId, pub name: String, pub shares: Shares, pub active: bool }`
  - `pub fn list(db: &Db) -> Result<Vec<SplitProfile>>` — ordered by id (the `←`/`→` order).
  - `pub fn get(db: &Db, id: SplitProfileId) -> Result<SplitProfile>`
  - `pub fn active(db: &Db) -> Result<SplitProfile>` — errors "no active split profile" when none (a corrupt database).
  - `pub fn insert(db: &Db, name: &str, shares: Shares) -> Result<SplitProfileId>` — inserted inactive; trims name, refuses empty and a case-insensitive duplicate with a message naming it.
  - `pub fn rename(db: &Db, id: SplitProfileId, name: &str) -> Result<()>` — same refusals as `insert`.
  - `pub fn set_shares(db: &Db, id: SplitProfileId, shares: Shares) -> Result<()>`
  - `pub fn set_active(db: &Db, id: SplitProfileId) -> Result<()>` — one `Db::transaction`: clear every `active`, then set this one; errors on a missing id.
  - `pub fn delete(db: &Db, id: SplitProfileId) -> Result<SplitProfileId>` — one transaction; refuses the last profile ("the last split profile cannot be deleted"); when the deleted row was active, activates the next by id, else the previous; returns the id now active.

- [ ] **Step 1: Migration arm 23.** SQL:
  ```sql
  CREATE TABLE split_profile (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    future_housing_bp INTEGER NOT NULL CHECK (future_housing_bp BETWEEN 0 AND 10000),
    retirement_bp INTEGER NOT NULL CHECK (retirement_bp BETWEEN 0 AND 10000),
    investment_bp INTEGER NOT NULL CHECK (investment_bp BETWEEN 0 AND 10000),
    active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0, 1))
  );
  CREATE UNIQUE INDEX split_profile_name ON split_profile (name COLLATE NOCASE);
  CREATE UNIQUE INDEX split_profile_one_active ON split_profile (active) WHERE active = 1;
  ```
  `data: Some(seed_split_profile)`: read the three stored strings `planning.split_down_payment_pct`, `planning.split_retirement_pct`, `planning.split_investment_pct` (bp as i64; unset or unparsable → 3000 / 2000 / 1000), clamp each to `0..=10000`, insert one row named `Default` with `active = 1`, then `DELETE FROM setting WHERE key IN (...)` those three. Literals belong in the migration: it is frozen history, not a reader of `key::`.

- [ ] **Step 2: Write failing tests** in `src/db/split_profile.rs` `mod tests`:
  - `a_fresh_database_starts_with_one_active_default_profile_at_thirty_twenty_ten` — `active()` is `Default`, shares `3000/2000/1000`, `list().len() == 1`.
  - `set_active_leaves_exactly_one_profile_active` — insert `Maxed`, `set_active`, assert `active().name == "Maxed"` and exactly one `p.active` in `list()`.
  - `deleting_the_active_profile_activates_the_next_one_by_id` and `deleting_the_last_listed_active_profile_activates_the_one_before_it`.
  - `the_last_profile_cannot_be_deleted`.
  - `a_name_already_used_in_another_case_is_refused` (`insert("default", …)` errs) and `a_blank_name_is_refused`.
  - In `src/db/migration.rs` tests (match the existing arm-test pattern there): `the_split_profile_arm_seeds_default_from_the_stored_splits_and_drops_the_keys` — build a database at version 22 with the three keys set to `1050`, `2050`, `6900`, migrate, assert the `Default` shares and that `setting` no longer holds the keys; and `a_stored_split_outside_a_share_is_clamped_rather_than_failing_the_open` (`15000` → `10000`, `-600` → `0`).
- [ ] **Step 3: Run** `cargo test --lib split_profile` — expect FAIL (module missing).
- [ ] **Step 4: Implement** `src/db/split_profile.rs` per Interfaces, the id type, `mod.rs` wiring and `PRESERVED_TABLES` entry (with the reason: the owner's rows, which the workbook carries nothing of).
- [ ] **Step 5: Run** `cargo test --lib` — expect PASS, including `every_table_the_schema_creates_is_either_cleared_or_deliberately_kept`.

### Task 2: The waterfall and the import read and write the active profile

**Files:**
- Modify: `src/plan.rs` (`settings_from_db`, `check_splits` → `check_shares`, its tests)
- Modify: `src/db/setting.rs` (remove `SPLIT_*` keys; tests at ~378, ~474, ~579 switch to `BILL_PAYMENT_PCT` or drop the entries)
- Modify: `src/import/mod.rs` (~233–250)
- Modify: `tests/planning_from_workbook.rs` (~211–227)
- Modify: `src/rate.rs:38`, `src/calc/AGENTS.md`, `src/import/AGENTS.md` (references to `check_splits`)

**Interfaces:**
- Consumes: Task 1's `split_profile::{active, set_shares, Shares}`.
- Produces: `pub fn check_shares(shares: Shares) -> Result<()>` in `src/plan.rs` — the same two refusals and messages `check_splits` has today (each share in `0..=100`, then the total ≤ 100), on values rather than the database. `check_splits` is removed.

- [ ] **Step 1: Convert the `check_splits` tests** in `src/plan.rs` (~535–610) to `check_shares` on literal `Shares` with the same values and the same asserted message fragments.
- [ ] **Step 2: Add** `settings_read_the_active_profiles_shares` — insert `Maxed` at `1000/4000/2000`, `set_active`, assert `settings_from_db` returns those in `future_housing_pct/retirement_pct/investment_pct`.
- [ ] **Step 3: Run** `cargo test --lib plan::` — expect FAIL.
- [ ] **Step 4: Implement.** `settings_from_db` takes the three shares from `split_profile::active(db)?`. Import: start from the active profile's `Shares`, overwrite each from its `F25:F27` cell when the cell holds a rate (a sheet carrying two of three keeps the third, as today), `check_shares`, then `set_shares` on the active id. `F19` keeps writing `BILL_PAYMENT_PCT`. Remove the three key constants.
- [ ] **Step 5: Import test.** In `src/import/` unit tests or `tests/` (whichever already covers the Planning cells without the workbook; else under `--features import` with an invented sheet), add `a_replace_writes_the_sheets_splits_into_the_active_profile_and_keeps_the_others`: two profiles, `Maxed` active, run the import, assert `Maxed` holds the sheet's shares and `Default`'s are unchanged and both still exist. Update `tests/planning_from_workbook.rs` to assert the three split cells against `split_profile::active(&db)` shares.
- [ ] **Step 6: Run** `cargo test`, `cargo test --features import`, both clippy runs — expect PASS.

### Task 3: The Split heading names the profile and edits write to it

**Files:**
- Modify: `src/plan_rows.rs` (`Target::SplitProfile`, `Input::split_profile`, the Split heading)
- Modify: `src/tui/planning/target.rs` (`write`, `write_split`, `is_money`, `in_split_block`, tests)
- Modify: `src/tui/planning/view.rs` (`View` fields, `build`)
- Modify: `src/tui/app/planning.rs` (`planning_view` fills the new fields)
- Modify: `src/report/mod.rs` (~164, ~374: `PlanView::split_profile: String`), `src/report/html/planning.rs` (pass it)

**Interfaces:**
- Consumes: Task 1 (`split_profile::{active, list, rename, set_shares}`).
- Produces:
  - `Target::SplitProfile` — the Split heading row's target. `write(db, today, raw)` renames the active profile. `is_money` false.
  - `impl Target { pub fn in_split_block(self) -> bool }` — true for `SplitProfile`, `FutureHousingPct`, `RetirementPct`, `InvestmentPct`.
  - `plan_rows::Input { split_profile: &'a str, .. }` — the name as the medium draws it (the screen passes it through `crate::demo::text`, the report passes it as stored).
  - `planning_screen::View { split_profile: String, split_profiles: usize, .. }`.

- [ ] **Step 1: Failing tests.**
  - `plan_rows`: `the_split_heading_names_the_profile_and_is_the_profiles_row` — with `split_profile: "Maxed"`, the heading row's label text is `Split · Maxed` and `target == Some(Target::SplitProfile)`.
  - `target.rs`: rewrite `each_target_writes_its_own_setting`'s split half and the `write_split` tests (~360–535) to assert against `split_profile::active(&db).shares`; keep the existing refusal-message assertions. Add `a_split_edit_writes_only_the_active_profile` (two profiles; editing Retirement leaves the inactive one's shares alone) and `writing_the_profile_target_renames_the_active_profile`.
  - `view.rs`: `the_heading_shows_switch_arrows_only_when_there_is_another_profile` — `split_profiles: 1` → label `Split · Default`; `2` → label ends ` ◀▶`.
- [ ] **Step 2: Run** `cargo test --lib planning` — expect FAIL.
- [ ] **Step 3: Implement.** `rows()` pushes `Row { target: Some(Target::SplitProfile), ..Row::heading(&format!("Split · {}", input.split_profile)) }`. `write_split` reads the active profile, replaces its one share, checks the set bound with today's message, and calls `set_shares`. The screen's `build` appends ` ◀▶` to the `Target::SplitProfile` row's label when `split_profiles > 1`. Every other `Input { .. }` construction (tests in `plan_rows.rs`) gains the field.
- [ ] **Step 4: Run** `cargo test`, `cargo test --all-features` — expect PASS.

### Task 4: Planning keys — `←`/`→`, contextual `a`/`d`, one `e`

**Files:**
- Modify: `src/tui/app/planning.rs` (`planning_key`, new openers/commits, remove `open_bill_edit`'s `E` binding, tests)
- Modify: `src/tui/modal.rs` (`ValueTarget::NewSplitProfile`, `Confirm::DeleteSplitProfile(SplitProfileId)` across `title`/`prompt`/`cancelled`/`commit`)
- Modify: `src/tui/app/mod.rs:~931` (route `ValueTarget::NewSplitProfile`)
- Modify: `src/tui/planning/target.rs` (`Target::Bill`'s `write` arm), `src/db/bill.rs` (remove `set_amount` and its tests if nothing else calls it)
- Modify: `src/tui/help.rs` (`PLANNING` entries), `src/tui/AGENTS.md` (Planning keys), root `AGENTS.md` (architecture row for `src/db/split_profile.rs`; an invariant bullet: profiles are the owner's and preserved, exactly one active, the import writes the active one)

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: `App::cycle_split_profile(&mut self, forward: bool) -> Result<()>`, `App::commit_new_split_profile(&mut self) -> Result<()>`.

Behaviour, with the cursor on a row whose target `in_split_block()`:
- `→`/`←`: activate the next/previous profile by id, wrapping; `reload()`; status `split profile: <name>` (masked). With one profile: no write, status `only one split profile — a adds another`.
- `a`: `Modal::Value(ValueTarget::NewSplitProfile, ValueForm::new("New profile", ""))`; commit inserts a copy of the active profile's shares under the typed name, activates it, reloads, status `<name> added`. A refused name keeps the form.
- `d`: with one profile, status `the last split profile cannot be deleted`; else `Confirm::DeleteSplitProfile(active.id)` labelled `<name>  <fh>% / <rt>% / <inv>%`; `y` deletes, status `split profile deleted · <now active name>`.
- `e` on the heading: `ValueForm::new("Profile", <real active name>)` under `ValueTarget::Planning(Target::SplitProfile)`.

Elsewhere `a`/`d` keep the bill add/delete; `e` on a `Target::Bill` row opens `BillForm::edit` (what `E` did); `E` is unbound. `Target::Bill`'s `write` arm bails `a bill is edited through its form`.

- [ ] **Step 1: Failing tests** in `src/tui/app/planning.rs` `mod tests` (use the existing `press` helper and `walk_until!` to reach rows):
  - `right_on_the_split_block_switches_the_live_plan_to_the_next_profile` — two profiles with different Retirement shares; after `→` the Retirement row's figure and `plan::settings_from_db` both reflect the second; cursor still on the same row.
  - `left_from_the_first_profile_wraps_to_the_last`.
  - `arrows_with_one_profile_write_nothing_and_say_so`.
  - `a_on_the_split_block_adds_a_copy_of_the_active_profile_and_switches_to_it`.
  - `a_on_a_bill_row_still_opens_the_bill_form`.
  - `d_on_the_split_block_deletes_the_active_profile_and_activates_a_neighbour`.
  - `d_on_the_only_profile_is_refused_without_a_dialog`.
  - `e_on_the_split_heading_renames_the_profile`.
  - `e_on_a_bill_opens_the_full_bill_form` — `Modal::Bill` with `editing == Some(id)`; replace the existing `E` tests (~1017, ~1047, ~1108, ~1128) with `e`.
  - `capital_e_does_nothing_on_planning`.
- [ ] **Step 2: Run** `cargo test --lib tui::app::planning` — expect FAIL.
- [ ] **Step 3: Implement** per the behaviour above; help entries: `e` detail gains "a bill opens its full form; the Split heading renames the profile", `a`/`d` details say what they do on the Split block, new `←/→` entry (`Label::Own("profile")`); drop the `E` entry. Fix any help/footer test that enumerates Planning keys.
- [ ] **Step 4: Run** `cargo fmt`, `cargo test`, `cargo test --all-features`, both clippy runs — expect PASS.
- [ ] **Step 5: Manual check** with the `run` skill against a scratch database (`--db` in the scratchpad): add a profile, switch with `→`, edit a share, delete it, and confirm the footer and Help read correctly.
