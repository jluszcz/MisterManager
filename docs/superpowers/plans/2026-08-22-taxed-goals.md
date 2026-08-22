# Taxed Goals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Store a savings goal's pre-tax **base** and derive its **target** through `calc::tax` on every read, so a taxed goal funds to what the item costs at the register rather than to its sticker price.

**Architecture:** One schema arm renames `goal.goal_cents` to `goal.base_cents` and adds `goal.taxed`. A new policy module `src/goal.rs` — the same shape as `src/fund.rs` — reads the `goal` table and `setting::key::TAX_RATE` out of `db` and hands `calc::tax` the base, producing a `Funding { goal, current, target }`. Every reader that wants a *target* (the Planning gates, the payday plug's set, `$/Pay`, the Savings screen's percent and expiry) goes through that module; readers that want names and ids keep using `db::goal` untouched. The two forms that edit a goal hold the base and show what it comes to.

**Tech Stack:** Rust, rusqlite (confined to `src/db/`), ratatui/crossterm (confined to `src/tui/`), anyhow, chrono.

**Spec:** `docs/superpowers/specs/2026-08-22-taxed-goals-design.md`

## Global Constraints

- **No real data in any tracked file.** No real balance, institution name, account code, or goal name traceable to a real person. Invented fixtures use the vocabulary in the root `CLAUDE.md`: cash `CHK`/`SAV`/`BKR`/`NST` named `Everyday`/`Rainy Day`/`Brokerage`/`Nest Egg`; credit `CC1`/`CC2`/`CC3`/`CHK` named `Card One`/`Card Two`/`Card Three`/`Everyday Card`. Every money literal is invented and round.
- **`rusqlite` is named only inside `src/db/`.** `calamine` only inside `src/import/`. `ratatui`/`crossterm` only inside `src/tui/`.
- **`schema.sql` is frozen at version 1.** A schema change is an arm appended to `db::migration::MIGRATIONS` and nothing else — no constant to bump. An arm names the schema as it stood when written, must survive replaying against an empty database, and a change of *meaning* is an arm too.
- **Settings are never addressed by a bare string.** `key::TAX_RATE` is the constant; a literal at a call site defeats the point.
- **Test names are full sentences describing the scenario** (`shortfall_of_an_overfunded_goal_is_zero_not_negative`, not `test_shortfall_3`). Unit tests live in `mod tests` at the bottom of the file under test and run against `db::open_in_memory`.
- **Comments describe the code as it is, not how it got there.** No "changed to…", "previously…", "kept for backwards compatibility".
- **Doc comments in this codebase use `--`, not an em dash**, inside `///` blocks. Markdown files use em dashes.
- **Verification command for every task:**
  ```bash
  cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
  ```
  Never `--no-verify` a commit; the pre-commit hook runs `cargo fmt --check`.
- **Commit on a feature branch.** The branch for this work is `GoalTax`, which is already checked out in this worktree. Never commit to `main`.
- **The branch already carries an unreleased first attempt** at this feature (uncommitted changes to `src/tui/app.rs`, `src/tui/goal_form.rs`, `src/tui/CLAUDE.md`): `Add Tax` as a transient form field that rewrote the target at commit. Do **not** unpick it first. The selector, the note, `GoalForm`'s `rate`, and `NO_TAX_RATE` all survive; Task 5 changes what `commit` does with the flag.

---

## File Structure

| File | Change | Responsibility after the change |
|---|---|---|
| `src/db/migration.rs` | Modify | Gains arm 4: the rename and the flag. |
| `src/db/goal.rs` | Modify | Queries only. `Goal`/`NewGoal`/`GoalEdit` carry `base_cents` and `taxed`. `shortfall` **leaves** — it is a target reader and the rate is out of reach here. |
| `src/goal.rs` | **Create** | The policy over `db::goal`: reads the table and `key::TAX_RATE`, feeds `calc::tax`, produces `Funding`. Owns `NO_TAX_RATE`. |
| `src/lib.rs` | Modify | Declares `pub mod goal;`. |
| `src/plan.rs` | Modify | `remaining` gates on `goal_engine::shortfall`. |
| `src/transfer.rs` | Modify | `shares_of` filters on `f.current < f.target`. |
| `src/tui/mod.rs` | Modify | `paycheck_ask` takes `&Funding`. |
| `src/tui/savings.rs` | Modify | `set_goals` takes `Vec<Funding>`; `Row` carries base and flag for the `e` prefill. |
| `src/tui/app.rs` | Modify | Feeds the screens from `goal_engine`; the picker hands the flag across instead of spending it. |
| `src/tui/goal_form.rs` | Modify | `GoalField::Taxed`; `commit` stores the flag rather than rewriting the figure. |
| `src/tui/recurring_goal.rs` | Modify | The `Base` field gains the same note. |
| `src/import/savings.rs` | Modify | Imported goals arrive `taxed: false` holding their target. |
| `CLAUDE.md`, `src/tui/CLAUDE.md` | Modify | The architecture table, the derived-target invariant, the replaced `Add Tax` invariant. |

---

### Task 1: The schema arm, the rename, and the flag on the row

The column rename is the point of doing this as one arm: for a taxed goal the stored number stops being the goal and becomes the base it is derived from, and no `CHECK` can catch a change of meaning. Renaming makes the compiler visit every reader.

**This task changes no behavior.** Every goal comes out `taxed = 0`, every reader goes on reading what is now called `base_cents`, and for an untaxed goal the base *is* the target. The tests that pass before this task pass after it.

**Files:**
- Modify: `src/db/migration.rs` (append to `MIGRATIONS`, add one test)
- Modify: `src/db/goal.rs` (`Goal`, `NewGoal`, `GoalEdit`, `from_row`, `select_goal!`, `insert`, `update`, `shortfall`, tests)
- Modify: `src/import/savings.rs:271`, `src/import/savings.rs:300`
- Modify: `src/plan.rs`, `src/transfer.rs`, `src/db/mod.rs`, `src/db/recurring_goal.rs`, `src/tui/app.rs`, `src/tui/savings.rs`, `src/tui/mod.rs`, `src/tui/planning.rs`, `src/tui/goal_form.rs` (mechanical: `goal_cents` → `base_cents`, `taxed: false` at every struct literal)
- Modify: `tests/import_savings.rs:159`, `tests/import_savings.rs:206`
- Do **not** modify: `src/db/schema.sql` (frozen at version 1)

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  // src/db/goal.rs
  pub struct Goal {
      pub id: GoalId,
      pub name: String,
      pub container_account_id: AccountId,
      pub base_cents: Cents,
      pub goal_date: Option<NaiveDate>,
      pub recurring_goal_id: Option<RecurringGoalId>,
      pub interest_eligible: bool,
      pub closed: bool,
      pub sort: i64,
      pub favorite: bool,
      pub taxed: bool,
  }

  pub struct NewGoal {
      pub name: String,
      pub container_account_id: AccountId,
      pub base_cents: Cents,
      pub goal_date: Option<NaiveDate>,
      pub recurring_goal_id: Option<RecurringGoalId>,
      pub interest_eligible: bool,
      pub sort: i64,
      pub taxed: bool,
  }

  pub struct GoalEdit {
      pub name: String,
      pub base_cents: Cents,
      pub goal_date: Option<NaiveDate>,
      pub interest_eligible: bool,
      pub taxed: bool,
  }
  ```
  `db::goal::shortfall` still exists and still reads `base_cents`; Task 2 removes it.

- [ ] **Step 1: Write the failing migration test**

Append to `mod tests` at the bottom of `src/db/migration.rs`, after `an_arm_may_carry_a_data_half_and_no_sql`:

```rust
    /// The real chain rather than the synthetic one, because this arm renames
    /// a column that existing rows carry values in: what matters is that the
    /// values are still there afterwards, under the new name and under the
    /// reading `taxed = 0` gives them.
    ///
    /// `MIGRATIONS[..2]` is where a database written by the previous build
    /// sits -- the head of a two-arm chain is version 3.
    #[test]
    fn arm_four_renames_the_goal_column_and_leaves_every_goal_untaxed() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, SCHEMA, &MIGRATIONS[..2]).unwrap();
        assert_eq!(version(&conn), 3);
        conn.execute(
            "INSERT INTO account (id, code, name, kind, grp)
             VALUES (1, 'SAV', 'Rainy Day', 'cash', 'savings')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO goal (id, name, container_account_id, goal_cents)
             VALUES (1, 'Couch', 1, 106500)",
            [],
        )
        .unwrap();

        apply(&conn, SCHEMA, MIGRATIONS).unwrap();

        assert_eq!(version(&conn), 4);
        let (base, taxed): (i64, i64) = conn
            .query_row("SELECT base_cents, taxed FROM goal WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(base, 106_500, "the figure did not survive the rename");
        assert_eq!(
            taxed, 0,
            "an existing goal already holds its target, so it is not taxed"
        );
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test --lib migration::tests::arm_four -- --nocapture`
Expected: FAIL. `MIGRATIONS[..2]` is the whole chain today, so `version` comes back 3, not 4, and the `SELECT base_cents` errors with "no such column: base_cents".

- [ ] **Step 3: Append the arm**

In `src/db/migration.rs`, add a third element to `MIGRATIONS`, after the `version: 3` arm:

```rust
    Migration {
        version: 4,
        // A taxed goal stores what the item costs on the shelf and derives
        // what it costs at the register, so the column stops being the goal
        // and starts being the base the goal is computed from. Renaming it is
        // what makes the compiler visit every reader: a change of meaning is
        // one no `CHECK` can catch.
        //
        // `NOT NULL DEFAULT 0` rather than nullable, for the reason `favorite`
        // is: there is no third state between taxed and not.
        sql: "ALTER TABLE goal RENAME COLUMN goal_cents TO base_cents;
              ALTER TABLE goal ADD COLUMN taxed INTEGER NOT NULL DEFAULT 0",
        // Nothing to move, and nothing that *could* be moved. Every existing
        // goal's stored figure already is its target: an imported one came off
        // a sheet whose goal column holds whatever the owner put there, and a
        // picker-created one had `calc::tax` applied before the insert.
        // `taxed = 0` is exactly the reading those rows want.
        //
        // Deliberately no back-fill from `recurring_goal.taxed` through
        // `goal.recurring_goal_id`: those goals hold a taxed figure already,
        // so flagging them would tax it twice, and the lambda ceilings, so the
        // base cannot be recovered by inverting it.
        data: None,
    },
```

- [ ] **Step 4: Run the migration tests to verify they pass**

Run: `cargo test --lib migration::`
Expected: PASS, including `every_arm_declares_the_version_its_position_gives_it`.

- [ ] **Step 5: Rename the field and add the flag in `src/db/goal.rs`**

Five edits in that file:

1. `Goal` — rename `goal_cents` to `base_cents` and append the flag with its doc comment:

```rust
    /// Whether this goal's target is `calc::tax` of its base rather than the
    /// base itself. The base is what the table holds; `crate::goal::target`
    /// is the one place the derivation happens.
    pub taxed: bool,
```

2. `NewGoal` — rename `goal_cents` to `base_cents`, append `pub taxed: bool,`.

3. `from_row` — `base_cents: Cents(row.get(3)?)` and, after `favorite`, `taxed: row.get::<_, i64>(10)? != 0,`.

4. `select_goal!` — both arms. The plain arm's column list becomes:

```rust
            "SELECT id, name, container_account_id, base_cents, goal_date,
                    recurring_goal_id, interest_eligible, closed, sort, favorite,
                    taxed
               FROM goal ",
```
   and the `with_balance` arm's, whose sum moves from index 10 to index 11:
```rust
            "SELECT g.id, g.name, g.container_account_id, g.base_cents, g.goal_date,
                    g.recurring_goal_id, g.interest_eligible, g.closed, g.sort, g.favorite,
                    g.taxed,
                    COALESCE((SELECT SUM(a.cents) FROM allocation a WHERE a.goal_id = g.id), 0)
               FROM goal g ",
```
   Both `query_map` closures that read that sum move from `row.get(10)?` to `row.get(11)?` — in `list_with_balances` and in `all_with_balances`. Update the macro's own doc comment: "followed by the goal's allocation sum as column index 11".

5. `insert` and `update` write the two columns:

```rust
pub fn insert(db: &Db, goal: &NewGoal) -> Result<GoalId> {
    db.conn.execute(
        "INSERT INTO goal
           (name, container_account_id, base_cents, goal_date, recurring_goal_id, interest_eligible, sort, taxed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            goal.name,
            goal.container_account_id,
            goal.base_cents.0,
            goal.goal_date.map(iso),
            goal.recurring_goal_id,
            goal.interest_eligible as i64,
            goal.sort,
            goal.taxed as i64
        ],
    )?;
    Ok(GoalId(db.conn.last_insert_rowid()))
}
```

```rust
pub fn update(db: &Db, id: GoalId, edit: &GoalEdit) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE goal SET name = ?2, base_cents = ?3, goal_date = ?4, \
         interest_eligible = ?5, taxed = ?6 WHERE id = ?1",
        params![
            id,
            edit.name,
            edit.base_cents.0,
            edit.goal_date.map(iso),
            edit.interest_eligible as i64,
            edit.taxed as i64,
        ],
    )?;
    ensure!(changed == 1, "no goal with id {id}");
    Ok(())
}
```

`GoalEdit` itself: rename `goal_cents` to `base_cents` and append `pub taxed: bool,`. Its doc comment gains a sentence:

```rust
/// The flag is here rather than being a column with one writer of its own,
/// the way `favorite` is: the goal form has a field for it, so an edit that
/// left it alone would make the field unable to take a mark back off.
```

`shortfall` reads `goal.base_cents` for now; Task 2 moves the whole function.

- [ ] **Step 6: Fix every call site the compiler names**

Run: `cargo build --all-targets 2>&1 | head -80`

Fix each error with one of two mechanical substitutions:
- a read of `.goal_cents` becomes `.base_cents`;
- a `NewGoal { .. }` or `GoalEdit { .. }` literal gains `taxed: false,` as its last field.

The sites, so none is missed:

| File | Sites |
|---|---|
| `src/db/goal.rs` | the `new_goal` test helper (`base_cents`, `taxed: false`), `get_round_trips_every_field`, `update_*` tests |
| `src/db/mod.rs` | the raw `INSERT INTO goal (... goal_cents ...)` at ~line 276 → `base_cents`; the two `NewGoal` literals |
| `src/db/recurring_goal.rs` | two `NewGoal` literals in tests |
| `src/import/savings.rs` | the goal-block `NewGoal` (~line 271) and the bucket-block `NewGoal` (~line 300) |
| `src/plan.rs` | four `NewGoal` literals in tests |
| `src/transfer.rs` | three `NewGoal` literals in tests |
| `src/tui/mod.rs` | `paycheck_ask`'s `goal.goal.goal_cents` |
| `src/tui/savings.rs` | `percent_complete` doc, `set_goals`'s three reads, the `goal` test helper's `Goal` literal |
| `src/tui/planning.rs` | one `NewGoal` literal in tests |
| `src/tui/app.rs` | `commit_goal`'s `goal_cents: edit.goal_cents`, `commit_picker`'s `goal_cents`, and the `NewGoal` literals in tests |
| `src/tui/goal_form.rs` | `commit`'s `GoalEdit` literal (add `taxed: false,` — the commit still applies the lambda itself at this stage), and its tests' `.goal_cents` assertions |
| `tests/import_savings.rs` | two `g.goal.goal_cents` reads |

For the two `src/import/savings.rs` literals add a comment above `taxed: false`, once, at the goal block:

```rust
                // The sheet's goal column holds whatever the owner typed, tax
                // included where they applied it, and carries no flag beside
                // it. An imported goal therefore arrives holding its target,
                // which is what untaxed means.
                taxed: false,
```

- [ ] **Step 7: Run the whole suite**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS. Behavior is unchanged; only names moved.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(db): store a goal's base and record whether it is taxed

Arm 4 renames goal.goal_cents to goal.base_cents and adds goal.taxed.
Every existing goal's stored figure already is its target, so taxed = 0
is the reading those rows want and the arm has no data half.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `src/goal.rs`, the policy module, and the Planning gates

`db::goal` is queries and `calc::tax` is pure; neither may read a setting. This is the module between them, the same shape as `src/fund.rs` — "reads the table and the rate out of `db`, feeds `calc`".

**Files:**
- Create: `src/goal.rs`
- Modify: `src/lib.rs`
- Modify: `src/db/goal.rs` (delete `shortfall` and move its four tests out)
- Modify: `src/plan.rs`
- Modify: `src/tui/goal_form.rs` (`NO_TAX_RATE` moves down to `crate::goal`)
- Modify: `src/tui/app.rs` (the import of `NO_TAX_RATE`)
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: `db::goal::{Goal, GoalWithBalance, all_with_balances, list_with_balances, get, balance}`, `db::setting::key::TAX_RATE`, `calc::tax`.
- Produces:
  ```rust
  // src/goal.rs
  pub const NO_TAX_RATE: &str;
  pub struct Funding { pub goal: db::goal::Goal, pub current: Cents, pub target: Cents }
  pub fn target(goal: &Goal, rate: Option<BasisPoints>) -> Result<Cents>;
  pub fn all_with_balances(db: &Db) -> Result<Vec<Funding>>;
  pub fn list_with_balances(db: &Db, container: AccountId) -> Result<Vec<Funding>>;
  pub fn shortfall(db: &Db, goal_id: GoalId) -> Result<Cents>;
  ```
  `db::goal::shortfall` no longer exists. Tasks 3–7 import this module as `use crate::goal as goal_engine;` — the alias `src/tui/app.rs` already uses for `crate::fund as fund_engine`.

- [ ] **Step 1: Write the failing tests**

Create `src/goal.rs` containing **only** the test module for now, so the tests fail to compile against functions that do not exist yet:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Kind};
    use crate::db::goal::NewGoal;
    use crate::db::setting;

    fn seeded() -> (db::Db, crate::db::AccountId) {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        (db, savings)
    }

    fn new_goal(name: &str, container: crate::db::AccountId, base: i64, taxed: bool) -> NewGoal {
        NewGoal {
            name: name.to_string(),
            container_account_id: container,
            base_cents: Cents::from_dollars(base),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            sort: 0,
            taxed,
        }
    }

    /// Most goals are not taxed, and for those the stored figure is the
    /// answer: the derivation must not touch them.
    #[test]
    fn an_untaxed_goals_target_is_its_base() {
        let g = db::goal::Goal {
            id: crate::db::GoalId(1),
            name: "Couch".to_string(),
            container_account_id: crate::db::AccountId(1),
            base_cents: Cents::from_dollars(1_000),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            closed: false,
            sort: 0,
            favorite: false,
            taxed: false,
        };
        assert_eq!(
            target(&g, Some(BasisPoints(625))).unwrap(),
            Cents::from_dollars(1_000)
        );
    }

    /// 6.25% of $1,000 is $1,062.50, which the lambda's $5 increment carries
    /// up to $1,065. That is what the goal has to be funded to, because that
    /// is what the item costs at the register.
    #[test]
    fn a_taxed_goals_target_is_what_the_lambda_makes_of_its_base() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        let funded = list_with_balances(&db, savings).unwrap();
        assert_eq!(funded[0].target, Cents(106_500));
        assert_eq!(
            funded[0].goal.base_cents,
            Cents::from_dollars(1_000),
            "the stored figure is still the base"
        );
    }

    /// An unset key normally means a feature is off, but the flag on the row
    /// says tax is wanted. Quietly targeting the base would move the Planning
    /// waterfall's plug on the strength of a missing setting.
    #[test]
    fn a_taxed_goal_with_no_rate_on_record_is_an_error_naming_the_key() {
        let (db, savings) = seeded();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        let err = all_with_balances(&db).unwrap_err().to_string();
        assert!(err.contains(key::TAX_RATE.name()), "{err}");
        assert!(err.contains("Couch"), "{err}");
    }

    /// The rate is only wanted by a goal that asked for tax, so a database no
    /// `Constants` sheet has reached still resolves every other goal.
    #[test]
    fn an_untaxed_goal_resolves_on_a_database_with_no_rate() {
        let (db, savings) = seeded();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, false)).unwrap();

        let funded = all_with_balances(&db).unwrap();
        assert_eq!(funded[0].target, Cents::from_dollars(1_000));
    }

    #[test]
    fn shortfall_is_the_target_less_the_balance() {
        let (db, savings) = seeded();
        let id = db::goal::insert(&db, &new_goal("Roth IRA", savings, 5_500, false)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(2_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents::from_dollars(3_500));
    }

    /// The shortfall of a taxed goal is measured against the taxed figure, or
    /// a goal funded to its base would come up short at the register -- which
    /// is the bug this whole feature exists to prevent.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_short_by_the_tax() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents(6_500));
    }

    /// Emergency Savings sits above its target in the live workbook, so an
    /// overfunded goal is a real state and not a hypothetical. Reporting a
    /// negative need would turn the emergency gate on and divert the entire
    /// discretionary split into a bucket that is already full.
    #[test]
    fn shortfall_of_an_overfunded_goal_is_zero_not_negative() {
        let (db, savings) = seeded();
        let id = db::goal::insert(&db, &new_goal("Emergency", savings, 100_000, false)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(120_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents::ZERO);
    }

    /// A dangling id is a corrupt database, not an unfunded goal. Returning
    /// zero here would silently switch a Planning gate off -- the exact
    /// failure this whole indirection exists to remove.
    #[test]
    fn shortfall_of_a_missing_goal_is_an_error() {
        let (db, _) = seeded();
        let err = shortfall(&db, crate::db::GoalId(999)).unwrap_err();
        assert!(err.to_string().contains("999"), "{err}");
    }
}
```

Add `pub mod goal;` to `src/lib.rs`, in alphabetical order — between `pub mod gate;` and `pub mod import;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib goal::tests 2>&1 | head -30`
Expected: FAIL to compile — `cannot find function target in this scope`, and the same for `all_with_balances`, `list_with_balances`, `shortfall`.

- [ ] **Step 3: Write the module above its tests**

Prepend to `src/goal.rs`:

```rust
//! The target every screen funds a goal to: the `goal` table and the sales
//! tax rate, fed to `calc::tax`.
//!
//! The shape of `fund.rs` -- reads rows and a setting out of `db`, hands
//! plain values to `calc`, hands the result back up. `db::goal` stays
//! queries, `calc::tax` stays pure, and `key::TAX_RATE` is read in one place.
//!
//! **The derived figure is the target everywhere, not a decoration.** A taxed
//! goal's shortfall, its percentage complete, its `$/Pay`, whether it counts
//! as still short for the payday plug, and whether it draws as overdue are
//! all computed against it. The stored number is the base those come from,
//! and the only places it is shown as itself are the two forms that edit it.

use crate::db::goal::{self, Goal, GoalWithBalance};
use crate::db::setting::{self, key};
use crate::db::{AccountId, Db, GoalId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result};

/// What a taxed goal with no rate on record reports, on the read side and on
/// the form that would otherwise write one.
///
/// Here rather than in `tui`, so the module that refuses to *derive* a target
/// and the form that refuses to *store* one say the same sentence.
pub const NO_TAX_RATE: &str = "no sales tax rate is configured; import Constants first";

/// A goal, its balance, and the target it is funded to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Funding {
    /// Carries `base_cents` and `taxed` -- what the table holds.
    pub goal: Goal,
    pub current: Cents,
    /// `base_cents`, or `calc::tax` of it. Derived on every read, the way a
    /// fund's target percentage is: a rate that changes must not leave a
    /// stored figure behind quoting the old one.
    pub target: Cents,
}

/// What a goal is funded to.
///
/// **A taxed goal with no rate on record is an error, not a silent fallback
/// to the base.** An unset key normally means a feature is off, but the flag
/// on the row says tax is wanted, and quietly targeting the base would move
/// the Planning waterfall's plug on the strength of a missing setting -- the
/// same reasoning that makes a dangling gate key an error. The state is hard
/// to reach anyway: the rate arrives with `Constants`, and the goal form
/// refuses to create a taxed goal without one.
pub fn target(goal: &Goal, rate: Option<BasisPoints>) -> Result<Cents> {
    if !goal.taxed {
        return Ok(goal.base_cents);
    }
    let rate = rate.with_context(|| {
        format!(
            "{:?} is taxed but {} is unset: {NO_TAX_RATE}",
            goal.name,
            key::TAX_RATE
        )
    })?;
    crate::calc::tax(goal.base_cents, rate)
}

/// The rate once, for a whole list. Reading it per goal would be one query
/// per row for a figure that cannot change while the list is being built.
fn derive(rate: Option<BasisPoints>, rows: Vec<GoalWithBalance>) -> Result<Vec<Funding>> {
    rows.into_iter()
        .map(|g| {
            Ok(Funding {
                target: target(&g.goal, rate)?,
                goal: g.goal,
                current: g.current,
            })
        })
        .collect()
}

/// Every open goal in every container, with its balance and its target, in
/// `db::goal::all_with_balances` order.
pub fn all_with_balances(db: &Db) -> Result<Vec<Funding>> {
    let rate = setting::get(db, key::TAX_RATE)?;
    derive(rate, goal::all_with_balances(db)?)
}

/// One container's open goals, in the order every screen shows them.
pub fn list_with_balances(db: &Db, container: AccountId) -> Result<Vec<Funding>> {
    let rate = setting::get(db, key::TAX_RATE)?;
    derive(rate, goal::list_with_balances(db, container)?)
}

/// How much a goal still needs: its target less its balance, clamped at zero.
///
/// Here rather than in `db::goal` because it is a *target* reader, and the
/// rate cannot be reached from `db`. Leaving a second one behind would let
/// `plan::remaining` go on gating the waterfall against a base.
///
/// Errors if the goal does not exist. A caller holding an id for a goal that
/// is gone is looking at a corrupt database, not at an unfunded goal, and
/// reporting zero there would silently disable a Planning gate.
///
/// Clamped because goals overshoot: Emergency Savings sits above its target
/// in the live workbook, and a negative need would read as "needs funding"
/// at every call site that compares against zero.
///
/// Ignores `closed`, as `container_excess` does -- a closed goal's
/// allocations still count.
pub fn shortfall(db: &Db, goal_id: GoalId) -> Result<Cents> {
    let found = goal::get(db, goal_id)?.with_context(|| format!("no goal with id {goal_id}"))?;
    let target = target(&found, setting::get(db, key::TAX_RATE)?)?;
    let remaining = target - goal::balance(db, goal_id)?;
    Ok(if remaining < Cents::ZERO {
        Cents::ZERO
    } else {
        remaining
    })
}
```

Add `use chrono::NaiveDate;` to the test module's imports (the allocation tests need it).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib goal::tests`
Expected: PASS, 8 tests.

- [ ] **Step 5: Delete `db::goal::shortfall` and its tests**

In `src/db/goal.rs`, delete the whole `pub fn shortfall` and its doc comment, and delete these four tests from `mod tests`: `shortfall_is_the_target_less_the_balance`, `shortfall_of_an_exactly_funded_goal_is_zero`, `shortfall_of_an_overfunded_goal_is_zero_not_negative`, `shortfall_of_a_missing_goal_is_an_error`. Their replacements are in `src/goal.rs` from Step 1.

In `db::goal::get`'s doc comment, the sentence "which is why [`shortfall`] and [`move_value`] put the `Option` back into that shape immediately" loses its dangling link — make it read "which is why [`crate::goal::shortfall`] and [`move_value`] put the `Option` back into that shape immediately".

In `src/db/id.rs`, the module doc at line ~5 mentions `shortfall`; retarget the link the same way if it is an intra-doc link, or leave the prose alone if it is not.

- [ ] **Step 6: Point the Planning gates at the new reader**

In `src/plan.rs`, add below the existing `use crate::db::...` lines:

```rust
use crate::goal as goal_engine;
```

and change the one line in `remaining`:

```rust
        Some(id) => goal_engine::shortfall(db, id).with_context(|| format!("setting {key} = {id}")),
```

Add a test to `src/plan.rs`'s `mod tests`, after `a_gate_setting_reports_its_goals_outstanding_need`:

```rust
    /// A gate over a taxed goal is not satisfied until the *taxed* figure is
    /// funded. Gating on the base would open the waterfall's next tier while
    /// the goal is still short of what the item costs.
    #[test]
    fn a_gate_over_a_taxed_goal_is_not_satisfied_until_the_taxed_figure_is_funded() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, crate::rate::BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &NewGoal {
                name: "Roth IRA".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
        setting::set(&db, Gate::Roth.key(), id).unwrap();

        assert_eq!(remaining(&db, Gate::Roth).unwrap(), Cents(6_500));
    }
```

If `NaiveDate` or `setting`/`key` are not already in scope in that test module, add the imports the compiler asks for.

- [ ] **Step 7: Move `NO_TAX_RATE` down**

In `src/tui/goal_form.rs`, delete the `pub const NO_TAX_RATE` definition and re-export the one that now lives below it, so both existing importers keep compiling:

```rust
pub use crate::goal::NO_TAX_RATE;
```

`src/tui/app.rs`'s `use super::goal_form::{..., NO_TAX_RATE};` is unchanged by that.

- [ ] **Step 8: Run the whole suite**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 9: Update the root `CLAUDE.md`**

Two edits.

In the Architecture table, insert a row after the `src/fund.rs` row:

```markdown
| `src/goal.rs` | Reads the `goal` table and the sales tax rate out of `db`, feeds `calc::tax`. The one place a goal's stored base becomes the target every screen funds it to, and the one reader of `setting::key::TAX_RATE`. |
```

In "Invariants worth knowing before editing", add a bullet immediately after the "A goal ends; it never becomes its successor" bullet:

```markdown
- **A goal's target is derived, never stored.** The table holds `goal.base_cents` and `goal.taxed`;
  `goal::target` is `base_cents` for an untaxed goal and `calc::tax` of it for a taxed one, the way
  `calc::fund` turns an age rule into a percentage. A rate that changes must not leave a stored
  figure behind quoting the old one. **The derived figure is the target everywhere** — the shortfall
  behind a Planning gate, the percentage complete, `$/Pay`, whether the payday plug still counts the
  goal as short, whether the Savings screen draws it as overdue. The base is shown as itself in
  exactly two places, the goal form and the recurring-goal form, and both say what it comes to
  beside it. A goal that funds to its base comes up short at the register, which is the whole
  reason the column is split in two.
  - **A taxed goal with no rate on record is a loud error naming `key::TAX_RATE`**, not a silent
    fallback to the base. An unset key normally means a feature is off, but the flag on the row says
    tax is wanted, and quietly targeting the base would move the waterfall's plug on the strength of
    a missing setting — the same reasoning that makes a dangling gate key an error. The goal form
    refuses to *write* that row for the same reason, with the same sentence: `goal::NO_TAX_RATE`.
  - **`shortfall` lives in `src/goal.rs`, not in `db::goal`.** It is a target reader and the rate
    cannot be reached from `db`; a second one left behind in `db` would let `plan::remaining` go on
    gating the waterfall against a base. `db::goal::balance` stays, because a sum is not a target.
```

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(goal): derive a taxed goal's target from its stored base

src/goal.rs is the policy over db::goal, the shape src/fund.rs is over
db::fund: it reads the table and key::TAX_RATE and feeds calc::tax.
shortfall moves there, so the Planning gates stop gating on a base.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: The payday plug's set

`transfer::spread_goals` is the set the plug is spread over: the goals no line claims that are *still short*. A taxed goal sitting at its base is short, and this is the test that would fail if any reader kept using the base.

**Files:**
- Modify: `src/transfer.rs:127-150` (`unclaimed_with_balances`, `shares_of`), plus one test
- Test: `src/transfer.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::goal::{Funding, all_with_balances}` from Task 2.
- Produces: `shares_of(&[goal_engine::Funding]) -> Vec<Goal>` — the same return type as before, so `spread_containers`, `spread_container`, `wiring` and `unclaimed_by_container` are untouched.

- [ ] **Step 1: Write the failing test**

Add to `src/transfer.rs`'s `mod tests`. Look at the existing tests around line 1290 for how that module seeds a database, and reuse its helpers rather than inventing new ones; the shape below is what the test must assert.

```rust
    /// A taxed goal sitting at its base is not funded -- it is short by the
    /// tax. The plug's set is the goals that are still short, so it has to be
    /// in it, and it has to be offered a share. This is the test that fails if
    /// any reader on this path goes back to the base.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_in_the_plugs_set() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let taxed = goal::insert(
            &db,
            &NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            taxed,
            day(2026, 1, 1),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();

        let spread = spread_goals(&db).unwrap();

        assert_eq!(
            spread.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![taxed],
            "a goal short by its tax must still be offered a share"
        );
    }

    /// The other half of the same rule: once the *taxed* figure is funded the
    /// goal needs nothing, so it drops out of the set -- and, because the same
    /// set decides where the plug lands, it stops pulling the spread into its
    /// container too.
    #[test]
    fn a_taxed_goal_funded_to_its_taxed_figure_drops_out_of_the_plugs_set() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let taxed = goal::insert(
            &db,
            &NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        let short = goal::insert(
            &db,
            &NewGoal {
                name: "Rug".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 1,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, taxed, day(2026, 1, 1), Cents(106_500), None, None).unwrap();

        let spread = spread_goals(&db).unwrap();

        assert_eq!(
            spread.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![short],
            "a goal at its taxed figure needs nothing"
        );
    }
```

- [ ] **Step 2: Run the tests to verify the first one fails**

Run: `cargo test --lib transfer::tests::a_taxed_goal`
Expected: `a_taxed_goal_funded_to_its_base_is_still_in_the_plugs_set` FAILS — `spread_goals` returns an empty set, because `shares_of` compares the balance against `base_cents` and finds the goal met. The second test passes already, for the wrong reason.

- [ ] **Step 3: Filter on the target**

In `src/transfer.rs`, add the import:

```rust
use crate::goal as goal_engine;
```

and change the two functions:

```rust
/// Every open goal `claimed` does not name, with its balance and its target.
fn unclaimed_with_balances(db: &Db, claimed: &[GoalId]) -> Result<Vec<goal_engine::Funding>> {
    Ok(goal_engine::all_with_balances(db)?
        .into_iter()
        .filter(|f| !claimed.contains(&f.goal.id))
        .collect())
}

/// The filter itself, over a set the claims have already been taken out of.
///
/// Split out because the claim list reaching it differs by caller: [`plan`]
/// reads claims strictly and refuses a dangling key, while [`wiring`] has to
/// report one and draw the screen anyway. Which goals the plug spreads over
/// is the same question either way, and this is the one answer to it.
///
/// Short means short of the **target**, so a taxed goal sitting at its base is
/// still in the set: what it needs is the tax.
fn shares_of(unclaimed: &[goal_engine::Funding]) -> Vec<Goal> {
    let short: Vec<Goal> = unclaimed
        .iter()
        .filter(|f| f.current < f.target)
        .map(|f| f.goal.clone())
        .collect();
    if !short.is_empty() {
        return short;
    }
    unclaimed.iter().map(|f| f.goal.clone()).collect()
}
```

The one other reader of `unclaimed_with_balances`, in `wiring` at ~line 350, already binds through the `goal` field (`with_balances.iter().map(|g| g.goal.clone())`) and needs no change.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib transfer::`
Expected: PASS.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

```bash
git add -A
git commit -m "fix(transfer): the plug's set is short of the target, not of the base

A taxed goal sitting at its base needs the tax, so it stays in the set
the payday plug is spread over -- and, since the same set decides where
the plug lands, it goes on holding its container in play.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The screens read the target

`$/Pay`, the percentage complete, and the overdue mark are all target questions. This task moves the three screen readers onto `Funding` and carries the base and the flag onto the Savings row, so `e` can prefill the form from the row it already has — the reason that row already carries `interest_eligible`.

**Files:**
- Modify: `src/tui/mod.rs:37,106-118` (`paycheck_ask`)
- Modify: `src/tui/savings.rs` (`Row`, `set_goals`, the `goal` test helper)
- Modify: `src/tui/app.rs` (`reload_savings` ~1538, `new_worksheet` ~2073, `spread_asks` ~585)
- Test: `src/tui/savings.rs` `mod tests`, `src/tui/app.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::goal::{Funding, target, all_with_balances, list_with_balances}` from Task 2.
- Produces:
  ```rust
  // src/tui/mod.rs
  pub fn paycheck_ask(goal: &crate::goal::Funding, today: NaiveDate, period_days: i64)
      -> Result<Option<Cents>>;

  // src/tui/savings.rs
  pub struct Row {
      pub goal_id: GoalId,
      pub container: super::Account,
      pub name: String,
      pub current: Cents,
      pub goal: Cents,      // the target
      pub base: Cents,      // what the table holds, for the `e` prefill
      pub taxed: bool,      // ditto
      pub percent: Option<Percent>,
      pub goal_date: Option<NaiveDate>,
      pub expired: bool,
      pub per_paycheck: Option<Cents>,
      pub interest_eligible: bool,
      pub favorite: bool,
  }
  pub fn set_goals(&mut self, goals: Vec<crate::goal::Funding>) -> Result<()>;
  ```
  Task 5 reads `Row::base` and `Row::taxed` in `App::open_goal_edit`.

- [ ] **Step 1: Write the failing tests**

In `src/tui/savings.rs`'s `mod tests`, first change the `goal` helper to build a `Funding` (it currently builds a `GoalWithBalance`) and add a taxed sibling beside it:

```rust
    /// `id` doubles as the sort key, so goals arrive in the order
    /// `all_with_balances` would return them.
    fn goal(
        id: i64,
        container: i64,
        name: &str,
        current: i64,
        target: i64,
        date: Option<NaiveDate>,
    ) -> Funding {
        Funding {
            goal: Goal {
                id: GoalId(id),
                name: name.to_string(),
                container_account_id: AccountId(container),
                base_cents: Cents(target),
                goal_date: date,
                recurring_goal_id: None,
                interest_eligible: true,
                closed: false,
                sort: id,
                favorite: false,
                taxed: false,
            },
            current: Cents(current),
            target: Cents(target),
        }
    }

    /// The same goal with the base and the target pulled apart, which is the
    /// only thing a taxed goal is: the screen must read the second one.
    fn taxed(mut g: Funding, base: i64, target: i64) -> Funding {
        g.goal.base_cents = Cents(base);
        g.goal.taxed = true;
        g.target = Cents(target);
        g
    }
```

Then add the two tests:

```rust
    /// A goal funded to its base is 94% of the way to what the item costs, not
    /// 100% of the way to its sticker price. The screen's percentage is a
    /// target question.
    #[test]
    fn a_taxed_goals_percentage_is_measured_against_its_taxed_target() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![taxed(
                goal(1, 1, "Couch", 100_000, 100_000, None),
                100_000,
                106_500,
            )])
            .unwrap();

        let row = savings.rows()[0];
        assert_eq!(row.goal, Cents(106_500), "the column shows the target");
        assert_eq!(row.base, Cents(100_000), "the base is carried for `e`");
        assert!(row.taxed);
        assert_eq!(row.percent, Some(Percent(94)));
    }

    /// Past its date and still short of the *taxed* figure is overdue. Reading
    /// the base would clear the mark on a goal that cannot yet buy the thing.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_expired_past_its_date() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![taxed(
                goal(1, 1, "Couch", 100_000, 100_000, Some(day(2026, 1, 1))),
                100_000,
                106_500,
            )])
            .unwrap();

        assert!(savings.rows()[0].expired);
    }

    /// `$/Pay` is the third target question on this screen, and the payday
    /// worksheet prefills with the same figure through the same function. A
    /// goal dated one pay period out asks for the whole of what it lacks, and
    /// what a taxed one lacks includes the tax.
    #[test]
    fn a_taxed_goals_paycheck_ask_is_measured_against_its_taxed_target() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![taxed(
                // today() is 2026-08-12 and the period is 14 days, so this is
                // exactly one paycheck away: one period to divide by.
                goal(1, 1, "Couch", 100_000, 100_000, Some(day(2026, 8, 26))),
                100_000,
                106_500,
            )])
            .unwrap();

        assert_eq!(savings.rows()[0].per_paycheck, Some(Cents(6_500)));
    }
```

`Savings::rows` returns `Vec<&Row>`, so `savings.rows()[0]` is already a `&Row`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib savings::tests::a_taxed 2>&1 | head -30`
Expected: FAIL to compile — `Funding` is not in scope and `set_goals` takes `Vec<GoalWithBalance>`.

- [ ] **Step 3: Move `paycheck_ask` onto `Funding`**

In `src/tui/mod.rs`, replace `use crate::db::goal::GoalWithBalance;` with `use crate::goal::Funding;` and change the signature and the one read:

```rust
pub fn paycheck_ask(
    goal: &Funding,
    today: NaiveDate,
    period_days: i64,
) -> Result<Option<Cents>> {
    crate::calc::per_paycheck(
        goal.current,
        goal.target,
        goal.goal.goal_date,
        today,
        period_days,
    )
}
```

Add one sentence to its doc comment, after the existing "`None` for an undated goal" paragraph:

```rust
/// The target, not the base: a goal due next paycheck asks for all it lacks,
/// and what a taxed one lacks includes the tax.
```

- [ ] **Step 4: Move the Savings screen onto `Funding`**

In `src/tui/savings.rs`, replace `use crate::db::goal::GoalWithBalance;` with:

```rust
use crate::db::goal::Goal;
use crate::goal::Funding;
```

Add the two fields to `Row`, after `goal`:

```rust
    /// The target, which is what the `Goal` column shows.
    pub goal: Cents,
    /// What the table holds. Equal to `goal` unless the goal is taxed, and
    /// carried so `e` can prefill the form from the row rather than
    /// re-querying -- the reason `interest_eligible` is here too.
    pub base: Cents,
    pub taxed: bool,
```

and rewrite `set_goals`:

```rust
    /// Take every open goal, in `goal::all_with_balances` order, and build the
    /// derived columns once.
    ///
    /// Every column that asks "how far along is this goal" reads `target`, so
    /// a taxed goal is measured against what the item costs at the register.
    pub fn set_goals(&mut self, goals: Vec<Funding>) -> Result<()> {
        let mut rows = Vec::with_capacity(goals.len());
        for g in goals {
            let per_paycheck = super::paycheck_ask(&g, self.today, self.period_days)?;
            rows.push(Row {
                goal_id: g.goal.id,
                container: super::Account::named(&self.accounts, g.goal.container_account_id),
                percent: percent_complete(g.current, g.target),
                expired: g.goal.goal_date.is_some_and(|d| d < self.today)
                    && g.current < g.target,
                name: g.goal.name,
                current: g.current,
                goal: g.target,
                base: g.goal.base_cents,
                taxed: g.goal.taxed,
                goal_date: g.goal.goal_date,
                per_paycheck,
                interest_eligible: g.goal.interest_eligible,
                favorite: g.goal.favorite,
            });
        }
        self.all = rows;
        self.rebuild_months();
        self.refilter();
        Ok(())
    }
```

In `percent_complete`'s doc comment, `goal_cents` is now the derived target — change the sentence to read "`None` for a non-positive target: the base is user-editable and reaches a divisor, which is the case `div_ceil` exists to refuse."

Fix the `favorited` test helper's two `GoalWithBalance` mentions to `Funding`.

- [ ] **Step 5: Feed the screens from the policy module**

In `src/tui/app.rs`, add the import beside the existing `use crate::fund as fund_engine;`:

```rust
use crate::goal as goal_engine;
```

Three call sites:

1. `reload_savings` (~line 1538):
```rust
        self.savings
            .set_goals(goal_engine::all_with_balances(&self.db)?)?;
```

2. `new_worksheet` (~line 2073):
```rust
        let mut prefill = Vec::new();
        for g in goal_engine::list_with_balances(&self.db, container)? {
            let ask = super::paycheck_ask(&g, self.today, self.period_days)?;
            prefill.push((g.goal.id, g.goal.name, ask.unwrap_or(Cents::ZERO)));
        }
```

3. `spread_asks` (~line 585) — the plug's goals arrive as bare `Goal`s from `transfer::spread_goals`, so this is the one caller that assembles a `Funding` itself. The rate is read once, outside the loop:

```rust
    fn spread_asks(&self) -> Result<Vec<(goal::Goal, Cents)>> {
        let rate = setting::get(&self.db, key::TAX_RATE)?;
        let mut out = Vec::new();
        for g in transfer::spread_goals(&self.db)? {
            let funding = goal_engine::Funding {
                current: goal::balance(&self.db, g.id)?,
                target: goal_engine::target(&g, rate)?,
                goal: g,
            };
            let ask = super::paycheck_ask(&funding, self.today, self.period_days)?;
            out.push((funding.goal, ask.unwrap_or(Cents::ZERO)));
        }
        Ok(out)
    }
```

The other four `goal::list_with_balances` callers in `app.rs` — the close-out's sibling list (~1932), the pending-worksheet prefill (~2045), and the interest worksheet's two (~2104) — want names, ids and balances rather than targets, so they keep using `db::goal` untouched.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib savings:: && cargo test --lib app::`
Expected: PASS. Fix any test in `app.rs` that constructed a `GoalWithBalance` for `paycheck_ask`.

- [ ] **Step 7: Run the whole suite and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

```bash
git add -A
git commit -m "feat(tui): the Savings columns and the payday ask read the target

percent_complete, the overdue mark and $/Pay are all target questions,
so all three now come off goal::Funding. The row carries the base and
the flag so \`e\` can prefill the form from what it already has.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The goal form stores the flag instead of spending it

`Add Tax` is currently an instruction that rewrites the figure at commit; it becomes `Taxed`, a property of the goal, matching the Recurring Goals form. The selector, the note, the form's `rate` and `NO_TAX_RATE` all survive as built.

**Files:**
- Modify: `src/tui/goal_form.rs` (`GoalField`, `GoalForm::new`, `commit`, doc comments, tests)
- Modify: `src/tui/app.rs` (`open_goal_edit`, `commit_goal`)
- Modify: `src/tui/CLAUDE.md`
- Test: `src/tui/goal_form.rs` `mod tests`

**Interfaces:**
- Consumes: `db::goal::{GoalEdit, NewGoal}` with `base_cents`/`taxed` (Task 1); `savings::Row::{base, taxed}` (Task 4).
- Produces:
  ```rust
  pub enum GoalField { Name, Target, Date, Taxed, Interest }
  impl GoalForm {
      pub fn new(
          goal_id: GoalId,
          name: &str,
          base: Cents,
          date: Option<NaiveDate>,
          interest_eligible: bool,
          taxed: bool,
          rate: Option<BasisPoints>,
          today: NaiveDate,
      ) -> GoalForm;
      pub fn commit(&self) -> Result<GoalEdit>;  // GoalEdit.base_cents is the typed figure; GoalEdit.taxed is the flag
  }
  ```

- [ ] **Step 1: Write the failing tests**

In `src/tui/goal_form.rs`'s `mod tests`, the `taxable` helper gains the new parameter:

```rust
    fn taxable(name: &str, base: Cents) -> GoalForm {
        GoalForm::new(
            GoalId(7),
            name,
            base,
            None,
            true,
            false,
            Some(BasisPoints(625)),
            today(),
        )
    }
```

Replace `a_taxed_goal_commits_what_the_tax_lambda_makes_of_its_target` with:

```rust
    /// The flag is stored and the figure is not rewritten: what the table
    /// holds is the base, and every reader derives the target from it. A
    /// commit that taxed the figure here would tax it again on the next edit.
    #[test]
    fn a_taxed_goal_commits_its_base_and_the_flag() {
        let mut form = taxable("Couch", Cents(100_000));
        while form.focus != GoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.display(GoalField::Taxed).plain_text(), "yes");
        let edit = form.commit().unwrap();
        assert_eq!(edit.base_cents, Cents(100_000));
        assert!(edit.taxed);
    }
```

and add:

```rust
    /// The flag is a field of the goal now, so the form opens on whatever the
    /// goal holds. Opening a taxed goal with the selector at `no` would make
    /// every edit of it silently untax it.
    #[test]
    fn a_form_opened_on_a_taxed_goal_opens_with_the_selector_on() {
        let form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            None,
            true,
            true,
            Some(BasisPoints(625)),
            today(),
        );

        assert_eq!(form.display(GoalField::Taxed).plain_text(), "yes");
        assert_eq!(
            form.display(GoalField::Target).plain_text(),
            "1,000.00",
            "the field holds the base"
        );
        assert_eq!(form.tax_note(), "(1,065 w/ tax)");
        let edit = form.commit().unwrap();
        assert_eq!(edit.base_cents, Cents(100_000));
        assert!(edit.taxed, "the flag round-trips");
    }
```

Rename the remaining `GoalField::Tax` occurrences in that module to `GoalField::Taxed`, and update the two `.goal_cents` assertions in `a_goal_opens_untaxed_and_commits_the_target_untouched` and `typing_on_the_tax_field_changes_nothing` to `.base_cents`. In `a_taxed_goal_with_no_rate_on_record_is_refused_rather_than_saved_untaxed`, the `GoalForm::new` call gains `false` for the new `taxed` parameter before its `None` rate.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib goal_form:: 2>&1 | head -30`
Expected: FAIL to compile — `no variant named Taxed`, and `GoalForm::new` takes seven arguments, not eight.

- [ ] **Step 3: Rename the field and store the flag**

In `src/tui/goal_form.rs`:

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalField {
    Name,
    Target,
    Date,
    Taxed,
    Interest,
}

impl GoalField {
    pub const ORDER: [GoalField; 5] = [
        GoalField::Name,
        GoalField::Target,
        GoalField::Date,
        GoalField::Taxed,
        GoalField::Interest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GoalField::Name => "Name",
            GoalField::Target => "Target",
            GoalField::Date => "Goal Date",
            GoalField::Taxed => "Taxed",
            GoalField::Interest => "Interest",
        }
    }
}
```

`GoalForm::new` takes the flag and prefills from it:

```rust
    pub fn new(
        goal_id: GoalId,
        name: &str,
        base: Cents,
        date: Option<NaiveDate>,
        interest_eligible: bool,
        taxed: bool,
        rate: Option<BasisPoints>,
        today: NaiveDate,
    ) -> GoalForm {
        GoalForm {
            subject: Subject::Existing(goal_id),
            focus: GoalField::Name,
            name: Field::given(name),
            // The base, which is what the table holds. What it comes to is
            // beside it, in `tax_note`.
            target: Field::given(base.to_string()),
            date: DateField::given(today, date),
            taxed,
            eligible: interest_eligible,
            rate,
        }
    }
```

`commit` stores rather than rewrites:

```rust
    pub fn commit(&self) -> Result<GoalEdit> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        let base_cents = parse_whole_amount(self.target.value())?;
        // Refused even though nothing here needs the rate any more: letting it
        // through would write precisely the row the read side calls corrupt,
        // and this is the one place that can ask for the rate before there is
        // a goal to be broken by its absence.
        if self.taxed {
            self.rate.context(NO_TAX_RATE)?;
        }
        Ok(GoalEdit {
            name,
            base_cents,
            // An empty date field is an undated goal -- rows 6-26 of the sheet.
            goal_date: self.date.parse_opt()?,
            interest_eligible: self.eligible,
            taxed: self.taxed,
        })
    }
```

Rename the three remaining `GoalField::Tax` arms — in `display`, `choice`, `type_char` and `backspace` — to `GoalField::Taxed`.

Rewrite the two doc comments that describe the old design. On the struct:

```rust
/// The two flags are `bool`s rather than `Field`s: they are selectors, so a
/// keystroke cannot leave one saying something that is neither yes nor no.
///
/// `Taxed` is a field of the goal, stored and derived from rather than spent
/// at commit: the Target field holds the **base**, and what the goal is
/// actually funded to is [`GoalForm::tax_note`], beside it.
```

On `tax_note`, replace "what the price in the field comes to once the tax lambda has had it" with "what the base in the field comes to once the tax lambda has had it", and drop the sentence claiming it is the only place the figure about to be saved is on screen — the figure about to be saved is now the base in the field. It becomes:

```rust
    /// The note beside the Target: what the base in the field comes to once
    /// the tax lambda has had it -- the figure every reader will derive.
    ///
    /// Empty whenever there is nothing to say -- the flag is off, the field is
    /// not a whole figure yet, or no rate is on record -- rather than a guess
    /// at one of the three.
```

- [ ] **Step 4: Rewire the two call sites in `app.rs`**

`open_goal_edit` reads the base and the flag off the row:

```rust
        self.modal = Some(Modal::Goal(GoalForm::new(
            row.goal_id,
            &row.name,
            row.base,
            row.goal_date,
            row.interest_eligible,
            row.taxed,
            setting::get(&self.db, key::TAX_RATE)?,
            self.today,
        )));
```

`commit_goal`'s `NewGoal` literal carries both across:

```rust
                        base_cents: edit.base_cents,
                        goal_date: edit.goal_date,
                        // A free-form goal answers to no recurring entry.
                        recurring_goal_id: None,
                        interest_eligible: edit.interest_eligible,
                        sort: goal::next_sort(&self.db, container)?,
                        taxed: edit.taxed,
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib goal_form:: && cargo test --lib app::`
Expected: PASS.

- [ ] **Step 6: Update `src/tui/CLAUDE.md`**

Replace the whole `**Add Tax** is an instruction, not a field of the goal.` bullet and its two sub-bullets with:

```markdown
- **`Taxed` is a field of the goal, and the Target field holds the base.** The flag is stored in
  `goal.taxed` and `crate::goal::target` derives the figure from it on every read, so a goal saved
  at `1,000` taxed reopens holding `1,000` with the selector still on — flipping it off is how it
  is untaxed, and nothing can tax a taxed figure twice. What makes the derived figure visible
  before Enter rather than after is the **note beside the Target**: `GoalForm::tax_note` puts
  `(1,065 w/ tax)` past the caret — the same place, and for the same reason, that the allocation
  form shows what a `/N` share comes to. The Recurring Goals form's `Base` field carries the same
  note, so the two forms answer the same question the same way.
  - **The Target field's label stays `Target`, not `Base`.** `Base` would match the other form
    exactly, but this field is the goal's target for every goal that is not taxed, which is most of
    them, and the note is what disambiguates the ones that are.
  - **The rate is read when the form opens, not when it commits.** The note has to be drawable on
    every keystroke. `None` — a database no `Constants` sheet has reached — still opens the form and
    draws no note, and the commit still **refuses a taxed goal** with `goal::NO_TAX_RATE`, even
    though it no longer needs the rate to compute anything: letting it through would write precisely
    the row the read side calls corrupt, and the form is the one place that can ask for the rate
    before there is a goal to be broken by its absence.
  - **The note is empty wherever there is nothing to say**, and never a guess: the flag is off, the
    Target is not a whole figure yet, or no rate is on record. It is drawn through
    `demo::whole_figure`, so `--demo` blocks it like every other absolute figure on a form.
```

In the bullet above it, `**The goal form's two `bool`s are selectors, not typed fields.**`, change `` `Add Tax` and `Interest` `` to `` `Taxed` and `Interest` ``.

- [ ] **Step 7: Run the whole suite and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

```bash
git add -A
git commit -m "feat(tui): the goal form stores Taxed rather than spending it

Add Tax was an instruction that rewrote the figure at commit, so a goal
could not be untaxed and reopening one showed no sign it had been taxed.
It is a property of the goal now: the field holds the base, the note says
what it comes to, and the commit still refuses a taxed goal with no rate.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: The recurring-goal handoff

A goal created from a taxed entry must be indistinguishable from one the owner marked taxed by hand. The flag survives into the goal instead of being spent at creation.

**Files:**
- Modify: `src/tui/app.rs` (`commit_picker`, ~line 2243)
- Test: `src/tui/app.rs` `mod tests`

**Interfaces:**
- Consumes: `db::recurring_goal::Entry::{base_cents, taxed}` (unchanged); `db::goal::NewGoal::{base_cents, taxed}` (Task 1).
- Produces: nothing new.

- [ ] **Step 1: Write the failing test**

`src/tui/app.rs`'s `mod tests` already has `only_a_taxed_catalog_entry_goes_through_the_tax_lambda` (~line 4741), which seeds a taxed `Rolex` and an untaxed `Dropbox`, presses `7 s Enter`, and asserts the two targets on the Savings screen. Those two assertions stay true after this task — the derivation puts the same figure back — so extend that test rather than writing a second one. Add, after the existing asserts:

```rust
        // ...and what the *table* holds is the base and the flag, handed
        // across rather than spent: the lambda runs on read, so a goal made
        // from a taxed entry is indistinguishable from one the owner marked
        // taxed by hand, and nothing can tax the taxed figure a second time.
        let stored = |name: &str| {
            let g = goal::all_with_balances(&app.db)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no goal named {name}"));
            (g.goal.base_cents, g.goal.taxed)
        };
        assert_eq!(stored("Rolex"), (Cents::from_dollars(9_000), true));
        assert_eq!(stored("Dropbox"), (Cents::from_dollars(9_000), false));
```

and add a paragraph to the test's doc comment:

```rust
    /// The picker hands the flag across rather than spending it, so the screen
    /// shows the derived target while the table holds the base.
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib app::tests::only_a_taxed_catalog_entry`
Expected: FAIL on the `Rolex` line of the new block — `base_cents` comes back as `Cents(956_500)` and `taxed` as `false`, because the picker applied the lambda at creation. The two target assertions above it still pass.

- [ ] **Step 3: Stop taxing at creation**

In `src/tui/app.rs`'s `commit_picker`, delete the `rate` binding and the `goal_cents` match, and write both fields straight across:

```rust
    fn commit_picker(&mut self) -> Result<()> {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return Ok(());
        };
        let container = picker.container();
        let chosen: Vec<Entry> = picker.chosen().into_iter().cloned().collect();
        ensure!(!chosen.is_empty(), NOTHING_SELECTED);
        // Every goal created here is dated, so each takes its place in the
        // container's dated block by deadline rather than landing at the end.
        // `sort` still runs in the order the picker showed them -- the ticked
        // group first, since that is the order the picker sorted itself into --
        // but among dated goals it decides only which of two falling on the
        // same day comes first.
        let first_sort = goal::next_sort(&self.db, container)?;
        let mut new_goals = Vec::with_capacity(chosen.len());
        for (offset, entry) in chosen.iter().enumerate() {
            let has_goal_this_year =
                goal::has_goal_dated_in_year(&self.db, entry.id, self.today.year())?;
            new_goals.push(goal::NewGoal {
                name: entry.name.clone(),
                container_account_id: container,
                // The entry's base and its flag, handed across rather than
                // spent: a goal made from a taxed entry is indistinguishable
                // from one the owner marked taxed by hand, and the lambda runs
                // once, on read.
                base_cents: entry.base_cents,
                goal_date: Some(picker::goal_date(entry, has_goal_this_year, self.today)?),
                recurring_goal_id: Some(entry.id),
                interest_eligible: true,
                sort: first_sort + offset as i64,
                taxed: entry.taxed,
            });
        }
        goal::insert_all(&self.db, &new_goals)?;
        self.status = format!("created {} goals", new_goals.len());
        self.close_modal();
        self.reload()
    }
```

Remove `NO_TAX_RATE` from `use super::goal_form::{...}` if the compiler reports it unused, and the same for `use crate::calc;` and `use anyhow::Context` — check each with `cargo clippy --all-targets -- -D warnings` rather than deleting on sight, since `app.rs` has other users of both.

Note the behavior this removes: the picker used to refuse to create *any* goal when something ticked was taxed and no rate was on record. It no longer needs to — nothing here computes a taxed figure — and the read side refuses instead, naming the key. Search for a test asserting that refusal (`grep -n "NO_TAX_RATE\|tax rate" src/tui/app.rs`); if one exists, delete it and say so in the commit message.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib app::`
Expected: PASS.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

```bash
git add -A
git commit -m "feat(tui): the picker hands the taxed flag across, not the taxed figure

A goal created from a taxed recurring entry now stores the entry's base
and its flag, so it is indistinguishable from one marked taxed by hand
and the lambda runs once, on read, instead of being baked in at creation.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The recurring-goal form's note

The two forms hold a base and show what it comes to, in the same words. This is the only change to that screen; its `Base` and `Taxed` columns already say what they say.

**Files:**
- Modify: `src/tui/recurring_goal.rs` (`RecurringGoalForm::{add, edit}`, `tax_note`, `render_form`)
- Modify: `src/tui/app.rs` (`open_recurring_goal_add`, `open_recurring_goal_edit`, and the key dispatch that calls the first)
- Modify: `src/tui/CLAUDE.md`
- Test: `src/tui/recurring_goal.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::calc::tax`, `crate::rate::BasisPoints`, `super::form::field_line_noted`.
- Produces:
  ```rust
  impl RecurringGoalForm {
      pub fn add(rate: Option<BasisPoints>) -> RecurringGoalForm;
      pub fn edit(entry: &Entry, rate: Option<BasisPoints>) -> RecurringGoalForm;
      pub fn tax_note(&self) -> String;
  }
  ```

- [ ] **Step 1: Write the failing test**

In `src/tui/recurring_goal.rs`'s `mod tests`:

```rust
    /// The same question the goal form answers, in the same words: the field
    /// holds the base, and the note says what the goal made from it will
    /// actually be funded to.
    #[test]
    fn the_note_beside_the_base_says_what_it_comes_to_with_tax() {
        let mut form = RecurringGoalForm::add(Some(BasisPoints(625)));
        assert_eq!(form.tax_note(), "", "nothing to say while the flag is off");

        while form.focus != RecurringGoalField::Amount {
            form.next_field();
        }
        for c in "1000".chars() {
            form.type_char(c);
        }
        while form.focus != RecurringGoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "(1,065 w/ tax)");
        assert_eq!(
            form.display(RecurringGoalField::Amount).plain_text(),
            "1000",
            "the field itself still holds the base"
        );
    }

    /// No rate on record is a database nobody has imported `Constants` into.
    /// The form still opens and simply says nothing, the way the goal form's
    /// note does -- this screen writes no goal, so there is nothing to refuse.
    #[test]
    fn the_note_is_empty_with_no_rate_on_record() {
        let mut form = RecurringGoalForm::add(None);
        while form.focus != RecurringGoalField::Amount {
            form.next_field();
        }
        for c in "1000".chars() {
            form.type_char(c);
        }
        while form.focus != RecurringGoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
    }
```

Add `false`-equivalent updates to every other `RecurringGoalForm::add()` / `::edit(&entry)` call in that test module: `add(None)` and `edit(&entry, None)`, except where a test is about the note.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib recurring_goal:: 2>&1 | head -20`
Expected: FAIL to compile — `add` takes no arguments and there is no `tax_note`.

- [ ] **Step 3: Carry the rate and draw the note**

In `src/tui/recurring_goal.rs`, add to the imports:

```rust
use crate::rate::BasisPoints;
```
and add `field_line_noted` and `parse_whole_amount` to the `use super::form::{...}` list (check what is already there; `parse_whole_amount` is imported already for `commit`).

On the struct, after `cadence`:

```rust
    /// The sales tax rate the `Taxed` note applies, as it stood when the form
    /// opened. `None` is a database no `Constants` sheet has been imported
    /// into: the note simply says nothing. Unlike the goal form, this one
    /// refuses nothing for it -- an entry writes no goal, and the goal made
    /// from it is where the rate is actually wanted.
    rate: Option<BasisPoints>,
```

`add` and `edit` take it and store it. Then:

```rust
    /// The note beside the Base: what it comes to once the tax lambda has had
    /// it -- the same sentence, in the same place, that the goal form's Target
    /// carries, so the two forms answer the same question the same way.
    ///
    /// Empty whenever there is nothing to say: the flag is off, the field is
    /// not a whole figure yet, or no rate is on record.
    pub fn tax_note(&self) -> String {
        match self.taxed_base() {
            Some(cents) => format!("({} w/ tax)", crate::demo::whole_figure(cents)),
            None => String::new(),
        }
    }

    fn taxed_base(&self) -> Option<Cents> {
        if !self.taxed {
            return None;
        }
        let base = parse_whole_amount(self.amount.value()).ok()?;
        crate::calc::tax(base, self.rate?).ok()
    }
```

and `render_form`:

```rust
pub fn render_form(frame: &mut Frame, form: &RecurringGoalForm) {
    let note = form.tax_note();
    let lines: Vec<TextLine> = RecurringGoalField::ORDER
        .iter()
        .map(|f| {
            // The Base field holds the pre-tax figure, so `Taxed` says what it
            // comes to beside the figure it applies to rather than beside
            // itself -- the goal form's Target does the same.
            let note = if *f == RecurringGoalField::Amount {
                note.as_str()
            } else {
                ""
            };
            field_line_noted(f.label(), form.display(*f), form.focus == *f, note)
        })
        .collect();
    render_fields(frame, form.title(), lines);
}
```

Drop `field_line` from the `use super::form::{...}` list if nothing else in the file uses it.

- [ ] **Step 4: Rewire `app.rs`**

```rust
    fn open_recurring_goal_add(&mut self) -> Result<()> {
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::add(
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }

    fn open_recurring_goal_edit(&mut self) -> Result<()> {
        let Some(row) = self.recurring_goal.selected() else {
            return self.nothing_selected();
        };
        let found = recurring_goal::get(&self.db, row.recurring_goal_id)?;
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::edit(
            &found,
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }
```

`open_recurring_goal_add` now returns `Result<()>`, so its caller in the key dispatch (~line 1017, `KeyCode::Char('a') => self.open_recurring_goal_add(),`) becomes `self.open_recurring_goal_add()?,`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib recurring_goal:: && cargo test --lib app::`
Expected: PASS. The render test at ~line 864 that checks the `Base` column header alignment is about the *table*, not the form, and should be unaffected — if it moved, the note is being drawn on the wrong screen.

- [ ] **Step 6: Note it in `src/tui/CLAUDE.md`**

Find the Recurring Goals screen's section and add a bullet there:

```markdown
- **The `Base` field carries the same note the goal form's `Target` does.** `RecurringGoalForm::tax_note`
  puts `(1,065 w/ tax)` past the caret whenever the `Taxed` selector is on and the field holds a whole
  figure, so both forms that edit a base answer the same question in the same words. This form refuses
  nothing when no rate is on record — an entry writes no goal, and the goal made from it is where the
  rate is actually wanted.
```

- [ ] **Step 7: Run the whole suite and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`

```bash
git add -A
git commit -m "feat(tui): the recurring-goal form says what its base comes to with tax

Both forms that edit a base now carry the same note in the same place, so
the two answer the same question the same way.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: The workbook oracle, and the docs read back

Nothing about the importer changed, but the oracle tests are the only ones that run the whole import against real data, and they have been skipping quietly through seven tasks unless run deliberately. This is the task that runs them and reads the docs back against the code.

**Files:**
- Modify: `CLAUDE.md`, `src/tui/CLAUDE.md` (only if the read-back finds a drift)
- Test: `tests/` (the workbook oracle), run rather than written

- [ ] **Step 1: Run the oracle tests against the real workbook**

The path and the account codes are the owner's and are not in this repository, so ask them for the two values rather than guessing, then:

```bash
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<workbook> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test
```

Expected: PASS. `MM_REQUIRE_WORKBOOK=1` turns a missing workbook into a hard failure rather than a quiet skip — without it, the tests that actually exercise the importer and the waterfall no-op. If the owner cannot supply the workbook, say so explicitly in the handoff rather than reporting a green suite: `cargo test` alone does not cover this path.

- [ ] **Step 2: Migrate a copy of the real database**

The arm renames a column in a database that has real rows in it. Against a **copy**, never the original:

```bash
cp ~/.local/share/mistermanager/mm.db /tmp/mm-migration-check.db   # confirm the real path with the owner first
cargo run --bin mm -- --db /tmp/mm-migration-check.db
```

Expected: the TUI opens, the Savings screen draws every goal at the figure it drew before, and Planning computes. Then `q`, and:

```bash
sqlite3 /tmp/mm-migration-check.db "PRAGMA user_version; SELECT COUNT(*) FROM goal WHERE taxed = 1;"
```
Expected: `4`, and `0` — no existing goal is taxed, which is the reading the arm's missing data half depends on.

Delete the copy afterwards.

- [ ] **Step 3: Read the docs back against the code**

Re-read the three sections written across Tasks 2, 5 and 7 and check each claim against what the code now does:

- Root `CLAUDE.md`'s new invariant: does `src/goal.rs` still have exactly the five public items it names? Is `db::goal::shortfall` really gone (`grep -rn "goal::shortfall" src`)? Is `db::goal::balance` still there?
- Root `CLAUDE.md`'s architecture table row: is `src/goal.rs` still the only reader of `key::TAX_RATE` (`grep -rn "TAX_RATE" src`)? It should appear in `src/goal.rs`, in `src/tui/app.rs` (three form-opening sites and `spread_asks`), in `src/db/setting.rs`, and in `src/import/constants.rs`. If it appears anywhere else, either the reader belongs behind `goal::target` or the table row needs narrowing to "the one place a *goal's* target is derived from it".
- `src/tui/CLAUDE.md`'s replaced bullet: does the goal form really reopen a taxed goal with the selector on, and does its commit really still refuse with `goal::NO_TAX_RATE`?

Fix whichever half is wrong — the doc if the code is right, the code if the doc describes what was agreed.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```
Expected: PASS, no output from `fmt --check`.

- [ ] **Step 5: Commit whatever the read-back moved**

```bash
git add -A
git commit -m "docs: read the taxed-goal invariants back against the code

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

If nothing moved, skip the commit and say so.

---

## Notes for the executor

- **`crate::goal` and `crate::db::goal` are two modules.** Wherever both are in scope, the db one keeps the plain name and the policy one is aliased `goal_engine`, matching `crate::fund as fund_engine` and `crate::recurring_txn as recurring_engine` in `src/tui/app.rs`.
- **`src/calc/CLAUDE.md` needs nothing.** `calc::tax` is untouched — it is already the workbook's lambda with the workbook's own cells as its test cases.
- **`--replace` is unaffected.** `goal` is still in `IMPORTED_TABLES`, so taxed goals are cleared with the rest, and `every_table_the_schema_creates_is_either_cleared_or_deliberately_kept` still holds — no table was added.
- **The import is unaffected.** `Savings!C` holds whatever the owner typed, tax included where they applied it, and the sheet carries no flag beside it. Imported goals arrive `taxed = 0` holding their target, which is true. `recurring_goal.taxed` likewise goes on arriving `false` from `O:Q` and being set on the Recurring Goals screen.
- **Nothing marks a taxed goal in the Savings list.** The `Goal` column goes on showing one figure and that figure is the target, which is what it already showed. The base is a keystroke away on the form.
