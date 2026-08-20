# Account Color Guarantee Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it impossible to put an account on screen without its color, rather than adding the tints that are missing today.

**Architecture:** Every uncolored account on screen is written the same way — a `format!` that turns the account into a `String` before any render function sees it. Three changes remove that move. `AccountName` and `AccountCode` drop `Display`, so the `format!` stops compiling crate-wide. `tui::Account` carries an account's id, display text and color as one value, with no reader for the text outside the module that tints it. `Label` is a ratatui-free string of plain and account segments, so a view-state type can return a title that still carries a tint.

**Tech Stack:** Rust, `rusqlite` (confined to `src/db/`), `ratatui`/`crossterm` (confined to `src/tui/`), `anyhow`.

**Spec:** `docs/superpowers/specs/2026-08-20-account-color-guarantee-design.md`

## Global Constraints

- **No real data in any tracked file.** No real balance, institution, account code, or goal name traceable to a person. Fixtures use the vocabulary in the root `CLAUDE.md`: cash `CHK`/`SAV`/`BKR`/`NST` named `Everyday`/`Rainy Day`/`Brokerage`/`Nest Egg`; credit `CC1`/`CC2`/`CC3`/`CHK` named `Card One`/`Card Two`/`Card Three`/`Everyday Card`.
- **`rusqlite` is named only inside `src/db/`. `ratatui`/`crossterm` only inside `src/tui/`.**
- **View-state types hold no ratatui.** Render functions only draw. This is why `Label` exists.
- **`ratatui::style::Color` is decided in `src/tui/style.rs` and nowhere else.**
- **Test names are full sentences** describing the scenario, not `test_foo_3`.
- **Unit tests live in `mod tests` at the bottom of the file under test** and run against `db::open_in_memory`.
- **Documentation describes the code as it is**, never how it got there. No "changed to…", "previously…", "kept for backwards compatibility".
- Every task ends green on: `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.
- Never commit to `main`. Work happens on the current feature branch.

## Known residuals

Two things this plan deliberately does not close, recorded so a reviewer does not read them as oversights:

1. **`AccountName::as_str` exists.** No Rust type erases it. Task 5's guard test pins the sanctioned call sites so a new one is a reviewed act rather than a quiet one.
2. **`transfer::Container` keeps `name: String`.** It is the pre-existing carrier of exactly `tui::Account`'s three fields — id, name, color — and the Planning screen does tint what it hands over, through `planning::Tint`. Task 7 unifies the two and is optional; without it Planning stays correct but keeps a second mechanism for the same idea.

---

### Task 1: `AccountName` and `AccountCode`

The stage that changes nothing on screen. Its compiler errors are the complete inventory of every account display in the crate — a better list than any grep, and the reason this task comes first.

**Files:**
- Modify: `src/db/account.rs` (add the types above `pub struct Account`, around line 273)
- Modify: `src/transfer.rs:301`, `:506`, `:607`, `:1017`
- Modify: `src/tui/accounts.rs:221`, `:530`, `:923`
- Modify: `src/tui/form.rs:324`, `:646`, `:657`, `:681`, `:1068`, `:1077`, `:1091`, `:1100`
- Modify: `src/tui/recurring_txn.rs:248`, `:462`, `:471`
- Modify: `src/tui/ledger.rs:525`, `:534`
- Modify: `src/tui/overview.rs:138`
- Modify: `src/tui/planning.rs:325`
- Modify: `src/tui/savings.rs:440`, `:449`
- Modify: `src/tui/app.rs:1240-1241`
- Modify: `tests/import_ledger.rs:144`, `:149`
- Modify: `tests/import_constants.rs:44`, `:50`, `:53`, `:152`, `:156`
- Test: `src/db/account.rs` `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `db::account::AccountName` and `db::account::AccountCode`. Both `Clone + PartialEq + Eq + PartialOrd + Ord + Hash + Debug`, both with `fn as_str(&self) -> &str`, `impl From<&str>`, `impl From<String>`, `impl PartialEq<&str>`, `ToSql`, `FromSql`, and **no `Display`**. `Account.name` becomes `AccountName`; `Account.code` becomes `AccountCode`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` at the bottom of `src/db/account.rs`:

```rust
/// The names go to SQLite as text and come back as the same text. A `ToSql`
/// that stored the debug form would put `AccountName("Rainy Day")` in the
/// column, and every `by_code` lookup and every screen would read it back.
#[test]
fn an_accounts_name_and_code_survive_a_round_trip_through_the_database() {
    let db = db::open_in_memory().unwrap();
    let id = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
    let account = get(&db, id).unwrap();
    assert_eq!(account.code, "SAV");
    assert_eq!(account.name, "Rainy Day");
    assert_eq!(account.name.as_str(), "Rainy Day");
}
```

The other half of the task is a property no assertion can state, because it is the *absence* of an impl: a name must not be interpolatable. Pin it with a ```compile_fail``` doc-test, which is the mechanism `src/db/id.rs` already uses for its own negative property — that `container_excess` refuses a `GoalId`. It goes in the doc comment Step 3 writes, not in `mod tests`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib an_accounts_name_and_code_survive`
Expected: FAIL to compile — `no method named as_str found for struct String`.

- [ ] **Step 3: Add the two types**

In `src/db/account.rs`, widen the existing rusqlite import:

```rust
use rusqlite::types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, Result as SqlResult, Row, ToSql, params};
```

and add above `pub struct Account`:

```rust
/// The two pieces of text an account is named by, each its own type.
///
/// Neither implements `Display`, and that absence is the whole point. Every
/// account that reaches a screen with no color on it gets there through a
/// `format!` -- a `String` cannot carry a tint, so an account that becomes
/// one has lost its color before any render function can see it. With no
/// `Display`, that does not compile, and the only route from an account to a
/// glyph is `tui::Account`, which colors what it draws.
///
/// `as_str` is the deliberate escape, for the handful of uses that are not
/// displays of an account: a description prefill, a search filter folding
/// case, a form seeding its editable field. It is named plainly rather than
/// hidden, so reaching for it is visible in a diff.
///
/// `PartialEq<&str>` so an assertion reads as it always did, and
/// `ToSql`/`FromSql` so `from_row` and every query are untouched.
macro_rules! account_text {
    ($name:ident, $what:literal) => {
        #[doc = concat!("An account's ", $what, ".")]
        ///
        /// Text the database stores, and deliberately **not** something
        /// `format!` will take:
        ///
        /// ```compile_fail
        /// use mistermanager::db::{self, account::Kind};
        /// let db = db::open_in_memory().unwrap();
        /// let id = db::account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        /// let account = db::account::get(&db, id).unwrap();
        /// println!("{}", account.name);
        /// ```
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(String);

        impl $name {
            /// The text, with no color on it.
            ///
            /// Not for drawing an account -- `tui::Account` draws one, and it
            /// colors what it draws. This is for the uses that are not
            /// displays at all.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> $name {
                $name(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> $name {
                $name(s)
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> SqlResult<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<$name> {
                String::column_result(value).map($name)
            }
        }
    };
}

account_text!(
    AccountCode,
    "short code, as the workbook's `Constants` sheet carries it"
);
account_text!(
    AccountName,
    "name, which is the owner's rather than the workbook's"
);
```

Then change the two fields:

```rust
pub struct Account {
    pub id: AccountId,
    pub code: AccountCode,
    pub name: AccountName,
    // kind, sort, group, color unchanged
}
```

`from_row` needs no edit: `row.get(1)?` and `row.get(2)?` resolve through the new `FromSql`. `insert` and `set_name` keep taking `&str` — the write path never had this problem.

- [ ] **Step 4: Build to get the inventory**

Run: `cargo build --all-targets 2>&1 | grep -E '^error' -A4 | head -120`
Expected: a list of every site that reads a name or code. That list is the work of Step 5. Keep it — it is the authoritative answer to "where is an account displayed".

- [ ] **Step 5: Fix every site**

Each falls into one of four shapes. Apply the matching fix; do not invent a fifth.

**(a) A fixture literal — construction, not display.** `.into()`:

```rust
// src/tui/ledger.rs:525, :534, src/tui/savings.rs:440, :449,
// src/tui/recurring_txn.rs:462, :471, src/tui/form.rs:1068, :1077, :1091, :1100
Account {
    id: AccountId(1),
    code: "CHK".into(),
    name: "Everyday".into(),
    kind: Kind::Cash,
    sort: 0,
    group: Group::Savings,
    color: None,
}
```

and in the two fixture helpers `src/tui/accounts.rs:530` and `:923`:

```rust
code: "CHK".into(),
name: name.into(),
```

**(b) Already reads `.as_str()` — no edit at all.** `src/tui/mod.rs:139`, `src/tui/mod.rs:149` and `src/tui/ledger.rs:363` call `.as_str()` on what is now an `AccountCode`/`AccountName`, which resolves to the new inherent method. Leave them; Task 2 deletes the first two and Task 3 replaces the third.

**(c) A genuine non-display use — `as_str()`, with a comment saying which.** These are the sanctioned sites Task 5 pins:

```rust
// src/tui/form.rs:646 -- prefills a description, not a display of an account
            self.description.fill(format!("{} Payment", card.code.as_str()));

// src/tui/form.rs:681 -- names the source in an error about a transfer to itself
            from.code.as_str()

// src/tui/accounts.rs:221 -- seeds the form's editable Name field
            name: Field::given(account.name.as_str().to_string()),

// src/db/account.rs:430 -- the "which account is Checking" error names the offenders
            .map(|a| a.name.as_str())

// src/transfer.rs:506 -- diagnose() writes a text report, outside src/tui/ entirely
        out.push(format!(
            "  {}: {count}{listed}",
            account::get(db, id)?.name.as_str()
        ));
```

**(d) A display that a later task replaces — `as_str().to_string()`, with a `FIXME` naming the task.** Every one of these disappears later in this plan. The `FIXME` is what stops a half-finished conversion looking finished:

```rust
// src/tui/form.rs:324, :657 and src/tui/recurring_txn.rs:248
// FIXME(task 4): a Label with one Account segment, so the selector is tinted.
                .map(|a| format!("{} — {}", a.code.as_str(), a.name.as_str()))

// src/tui/overview.rs:138
// FIXME(task 2): tui::Account carries the id, the text and the color together.
                        label: account.name.as_str().to_string(),

// src/tui/app.rs:1240-1241 -- builds an accounts::Row
// FIXME(task 2): as above.
                code: account.code.as_str().to_string(),
                name: account.name.as_str().to_string(),

// src/tui/planning.rs:325 -- tinted today through planning::Tint; see "Known residuals"
                (account.name.as_str().to_string(), String::new())

// src/transfer.rs:301, :607, :1017 -- Container is (id, name, color); see "Known residuals"
            name: a.name.as_str().to_string(),
```

In `tests/`: `tests/import_ledger.rs:144` and `:149` pass `&cash.code` where `by_code` wants `&str`, so they become `cash.code.as_str()`. `tests/import_constants.rs:44`, `:152` and `:156` collect names and codes — `.as_str().to_string()`. `:50` interpolates both into an assertion message and `:53` interpolates the code — `.as_str()` on each.

- [ ] **Step 6: Run the full suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS. Nothing renders differently — this task adds a type and changes no behavior.

- [ ] **Step 7: Verify the negative property actually holds**

Run: `cargo test --doc account`
Expected: PASS, meaning the ```compile_fail``` block did fail to compile. The failure mode here is a doc-test that never ran, so confirm the output names at least one doc-test rather than zero.

- [ ] **Step 8: Commit**

```bash
git add src/db/account.rs src/transfer.rs src/tui tests
git commit -m "Give an account's name and code types that cannot be interpolated

Every account that reached a screen uncolored got there through a format!,
because a String cannot carry a tint and nothing stopped an account
becoming one. AccountName and AccountCode have no Display, so those stop
compiling; as_str is the named escape for the uses that are not displays.

Nothing renders differently. The FIXMEs mark the displays that the
tui::Account and Label conversions replace.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `tui::Account`, and the tables

**Files:**
- Create: `src/tui/label.rs`
- Modify: `src/tui/mod.rs` (declare the module; delete `account_code`, `account_name`, `account_color_of`, and the old `account_cell`)
- Modify: `src/tui/savings.rs` (`Row`, `set_goals`, `render`, tests)
- Modify: `src/tui/recurring_txn.rs` (`Row`, `set_recurring_txns`, `render`, tests)
- Modify: `src/tui/ledger.rs` (delete `Ledger::account_color`, add `Ledger::accounts`, `render`)
- Modify: `src/tui/overview.rs` (`Line`, `table_row` and its call sites)
- Modify: `src/tui/accounts.rs` (`Row`, `render`)
- Modify: `src/tui/app.rs:1240-1241`
- Test: `src/tui/label.rs` `mod tests`

**Interfaces:**
- Consumes: `db::account::{Account, AccountName, AccountCode}` from Task 1.
- Produces:
  - `tui::label::Account`, `Clone + Debug + PartialEq + Eq`, with `Account::named(&[account::Account], AccountId) -> Account`, `Account::coded(&[account::Account], AccountId) -> Account`, `Account::labelled(&account::Account) -> Account`, `pub fn id(&self) -> AccountId`, and `#[cfg(test)] pub fn text(&self) -> &str`.
  - `tui::label::account_cell(&Account) -> Cell<'static>`, `pub(super)`.
  - Both reached from screens as `super::Account` and `account_cell`, via `use label::{Account, account_cell};` in `src/tui/mod.rs`.

- [ ] **Step 1: Write the failing test**

Create `src/tui/label.rs` containing only this `mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{Group, Kind};

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "CHK".into(),
                name: "Everyday".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Checking,
                color: None,
            },
            account::Account {
                id: AccountId(2),
                code: "NST".into(),
                name: "Nest Egg".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: Some(AccountColor::Teal),
            },
        ]
    }

    /// Which half of an account a screen shows is the screen's business --
    /// Recurring Transactions has room for a code and no more -- but the id
    /// and the color travel with it either way, which is what makes the same
    /// account one color across all of them.
    #[test]
    fn an_account_carries_its_id_and_color_whichever_half_it_shows() {
        let all = accounts();
        let named = Account::named(&all, AccountId(2));
        let coded = Account::coded(&all, AccountId(2));
        assert_eq!(named.text(), "Nest Egg");
        assert_eq!(coded.text(), "NST");
        assert_eq!(named.id(), AccountId(2));
        assert_eq!(cell_color(&named), cell_color(&coded));
    }

    /// The three form selectors show both halves in one cell, so both halves
    /// are one segment and take one color -- splitting them would leave the
    /// code reading as chrome in front of a colored name.
    #[test]
    fn a_selectors_label_shows_the_code_and_the_name_as_one_account() {
        assert_eq!(Account::labelled(&accounts()[0]).text(), "CHK — Everyday");
    }

    /// An id with no account is a corrupt row, not a reason to stop drawing
    /// the screen -- so it renders, and it still gets a color.
    #[test]
    fn an_id_with_no_account_still_draws_and_still_has_a_color() {
        let missing = Account::named(&accounts(), AccountId(99));
        assert_eq!(missing.text(), "?");
        assert_eq!(
            cell_color(&missing),
            Some(style::account_color(AccountId(99), None))
        );
    }

    /// The whole guarantee, at the one place it is discharged: the cell an
    /// account draws into is colored, always, with no way for a caller to ask
    /// for an uncolored one.
    #[test]
    fn every_account_cell_is_colored() {
        let all = accounts();
        for id in [AccountId(1), AccountId(2), AccountId(99)] {
            assert!(cell_color(&Account::named(&all, id)).is_some(), "{id}");
        }
    }

    /// The owner's choice outranks the derived shade here exactly as it does
    /// in `style::account_color` -- this is the plumbing, not a second
    /// decision about what an account looks like.
    #[test]
    fn a_chosen_color_reaches_the_cell() {
        assert_eq!(
            cell_color(&Account::named(&accounts(), AccountId(2))),
            Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
        );
    }

    /// Reads the foreground the cell's first glyph draws in.
    ///
    /// Through a rendered buffer rather than off the `Cell`, because a
    /// `Cell`'s spans are private -- and because the buffer is what the
    /// terminal actually shows, which is the thing under test. The same trick
    /// `savings.rs` and `recurring_txn.rs` already use to read a column's
    /// color back.
    fn cell_color(account: &Account) -> Option<style::Color> {
        use ratatui::layout::{Constraint, Rect};
        use ratatui::widgets::{Row, Table, Widget};
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        Table::new(
            vec![Row::new(vec![account_cell(account)])],
            [Constraint::Length(20)],
        )
        .render(area, &mut buffer);
        buffer[(0, 0)].style().fg
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib label`
Expected: FAIL to compile — `cannot find struct Account in this scope`.

- [ ] **Step 3: Write `src/tui/label.rs`**

Above the `mod tests` just written:

```rust
//! An account on its way to the screen, and the one place it gets its color.
//!
//! `style::account_color` decides what an account looks like; this module is
//! what makes that decision unavoidable. [`Account`] holds an account's id,
//! the text a screen shows for it, and the owner's color, with **no reader
//! for the text outside this file**. So the only route from an account to a
//! glyph is [`account_cell`], which colors what it draws, and a screen that
//! wants an uncolored account has no way to ask for one.
//!
//! That is a stronger arrangement than a helper every screen is supposed to
//! remember. The tables all did remember; the titles and the form selectors
//! did not, and every one of them went wrong the same way -- a `format!` that
//! turned the account into a `String` before any render function could see
//! it. `db::account::AccountName` has no `Display` for the same reason, one
//! layer down.

use crate::db::AccountId;
use crate::db::account::{self, AccountColor};
use ratatui::text::Line as TextLine;
use ratatui::widgets::Cell;

use super::style;

/// An account as a screen shows it: which account, what text, what color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    id: AccountId,
    text: String,
    color: Option<AccountColor>,
}

impl Account {
    /// `Everyday` -- the ledgers, Savings, Overview and the Accounts screen.
    ///
    /// An id with no account draws as `?` and still takes a color: a corrupt
    /// row is not a reason to stop drawing a screen, and
    /// [`style::account_color`] has a shade for every id.
    pub fn named(accounts: &[account::Account], id: AccountId) -> Account {
        Account::look_up(accounts, id, |a| a.name.as_str().to_string())
    }

    /// `CHK` -- Recurring Transactions, whose other columns pin a row down
    /// already, so the code alone says which account and reads better tight
    /// than padded. Also the ledger title, whose account is a filter term.
    pub fn coded(accounts: &[account::Account], id: AccountId) -> Account {
        Account::look_up(accounts, id, |a| a.code.as_str().to_string())
    }

    /// `CHK — Everyday` -- the three form selectors, which have a whole
    /// field's width where a column has none.
    ///
    /// One segment rather than two: both halves name the same account, and
    /// splitting them would leave the code reading as chrome in front of a
    /// colored name.
    pub fn labelled(account: &account::Account) -> Account {
        Account {
            id: account.id,
            text: format!("{} — {}", account.code.as_str(), account.name.as_str()),
            color: account.color,
        }
    }

    pub fn id(&self) -> AccountId {
        self.id
    }

    /// The text, private to this file.
    ///
    /// Every caller is one of this module's own. A reader outside it would be
    /// a way to draw an account without its color, which is the whole of what
    /// this type prevents.
    fn as_text(&self) -> &str {
        &self.text
    }

    /// The text, for assertions.
    ///
    /// `pub` only in a test build, so it is not a route to an uncolored
    /// account in one that ships.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        self.as_text()
    }

    /// The color this account draws in, wherever it draws.
    fn color(&self) -> style::Color {
        style::account_color(self.id, self.color)
    }

    fn look_up(
        accounts: &[account::Account],
        id: AccountId,
        text: impl Fn(&account::Account) -> String,
    ) -> Account {
        match accounts.iter().find(|a| a.id == id) {
            Some(a) => Account {
                id,
                text: text(a),
                color: a.color,
            },
            None => Account {
                id,
                text: "?".to_string(),
                color: None,
            },
        }
    }
}

/// A cell naming an account, colored by [`style::account_color`].
///
/// One of this module's two exits, and it offers no way to skip the color.
pub(super) fn account_cell(account: &Account) -> Cell<'static> {
    super::tinted(
        TextLine::from(account.as_text().to_string()),
        Some(account.color()),
    )
}
```

- [ ] **Step 4: Wire it into `src/tui/mod.rs`**

Add `mod label;` beside the other module declarations and `use label::{Account, account_cell};` beside the existing `use` block. Delete `account_code` (`src/tui/mod.rs:135-140`), `account_name` (`:145-150`), `account_color_of` (`:298-300`) and the old `account_cell` (`:286-288`). `tinted` stays exactly as it is — `label.rs` calls it, and so do `fund.rs`, `planning.rs` and `savings.rs`.

- [ ] **Step 5: Convert the five table screens**

`src/tui/savings.rs` — `Row` loses two fields and gains one:

```rust
pub struct Row {
    pub goal_id: GoalId,
    /// The container this goal sits in, as the Account column shows it. One
    /// value rather than an id, a name and a color kept in step by hand.
    pub container: super::Account,
    pub name: String,
    // current, goal, percent, goal_date, expired, per_paycheck,
    // interest_eligible all unchanged
}
```

In `set_goals` the three lines collapse to one:

```rust
                container: super::Account::named(&self.accounts, g.goal.container_account_id),
```

and in `render`:

```rust
                account_cell(&r.container),
```

Every `r.container` used as an `AccountId` becomes `r.container.id()`. `Savings::account_name` loses the `super::account_name` this task deletes, so give it a body of its own — the reconciliation footer still needs a plain name:

```rust
    /// The container's name as text, for the reconciliation footer.
    ///
    /// The footer is a status strip rather than a place a reader looks to
    /// identify an account, so it is deliberately not tinted. Everything else
    /// on this screen names a container through `Account`.
    pub fn account_name(&self, id: AccountId) -> &str {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map_or("?", |a| a.name.as_str())
    }
```

`src/tui/recurring_txn.rs` — the same shape:

```rust
pub struct Row {
    pub recurring_txn_id: RecurringTxnId,
    pub description: String,
    /// The account this row belongs to, as the Account column shows it: its
    /// code, since the other columns pin the row down already.
    pub account: super::Account,
    // cents, cadence, anchor_date, last_owned, is_paycheck, owned unchanged
}
```

```rust
                account: super::Account::coded(&self.accounts, txn.account_id),
```

```rust
                account_cell(&r.account),
```

`r.account_id` becomes `r.account.id()`. `RecurringTxns::account_code` keeps its two test callers, reading `self.accounts` directly the way `Savings::account_name` now does.

`src/tui/ledger.rs` — delete `Ledger::account_color` (`:352-354`), add the accessor beside `Ledger::account_name`:

```rust
    pub(super) fn accounts(&self) -> &[account::Account] {
        &self.accounts
    }
```

and change `render`:

```rust
                account_cell(&super::Account::named(ledger.accounts(), t.account_id)),
```

`src/tui/overview.rs` — `Line` loses `color`, and its label becomes an optional account:

```rust
pub struct Line {
    /// The account this row is, or `None` for a subtotal, which names a band
    /// rather than an account and takes no tint.
    pub account: Option<super::Account>,
    /// What a subtotal row is labelled. Empty when `account` is set.
    pub label: String,
    pub balances: Balances,
}
```

In the builder (`:136-138`):

```rust
                    Line {
                        account: Some(super::Account::named(&accounts, account.id)),
                        label: String::new(),
                        balances: /* the existing match on account.kind */,
                    }
```

`table_row` takes the line rather than three arguments:

```rust
/// A data row. An account row is tinted by its account; a subtotal names a
/// band rather than an account, so it takes the plain foreground.
fn table_row(line: &Line, balances: Balances, style: Style) -> Row<'static> {
    let label = match &line.account {
        Some(account) => account_cell(account),
        None => Cell::from(line.label.clone()),
    };
    Row::new(vec![
        label,
        amount(balances.to_date),
        amount(balances.adhoc),
        amount(balances.month_end),
    ])
    .style(style)
}
```

The subtotal call sites build a `Line` that names a band:

```rust
    table_row(
        &Line {
            account: None,
            label: band.group.label().to_string(),
            balances: band.total,
        },
        band.total,
        bold,
    )
```

and the `Net` row the same way, with `"Net"` as its label.

`src/tui/accounts.rs` — its `Row` already carries `account_id` and `color`; collapse them into `pub account: super::Account`, built in `src/tui/app.rs:1240-1241` as `account: super::Account::named(&accounts, account.id)` and rendered at `:439` as `account_cell(&r.account)`. The `Code` column at `:438` keeps `Cell::from(r.code.clone())`: it is the second column of a row whose first column already names the account in color, and coloring both would say the same thing twice.

- [ ] **Step 6: Run the suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS. Fix the tests reading the deleted fields — `savings.rs:868` and `app.rs:7163` read `r.account_name`, which becomes `r.container.text()`; `recurring_txn.rs:548` and `:561` read `r.account_code`, which becomes `r.account.text()`.

- [ ] **Step 7: Confirm nothing changed on screen**

Run: `cargo test`
Expected: PASS. This task touches no importer or waterfall code, so the workbook oracle is not required here; say in the commit or the handoff which suite you ran.

- [ ] **Step 8: Commit**

```bash
git add src/tui
git commit -m "Carry an account to the screen as one value that knows its color

tui::Account holds an account's id, the text a screen shows for it and the
owner's color, with no reader for the text outside the module that tints
it -- so account_cell is the only route to a glyph and it colors what it
draws.

It also replaces the three parallel fields two screens had grown, where a
name, an id and a color had to be kept in step by hand.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: `Label`, and the screen titles

**Files:**
- Modify: `src/tui/label.rs` (add `Label`, `Segment`, `label_line`)
- Modify: `src/tui/mod.rs` (extend the re-export to `use label::{Account, Label, account_cell, label_line};`)
- Modify: `src/tui/savings.rs` (`title`, render)
- Modify: `src/tui/ledger.rs` (`title`, `title_line`)
- Modify: `src/tui/picker.rs` (`container_name` becomes `container: Account`; `title`, render)
- Modify: `src/tui/worksheet.rs` (same substitution; `title`, render)
- Modify: `src/tui/goal_form.rs` (`Subject::New`, `GoalForm::title`)
- Modify: `src/tui/form.rs` (`ValueForm::new`/`label`, `render_fields`)
- Modify: `src/tui/app.rs` (the Reconcile modal's label, and the call sites building the above)
- Test: `src/tui/label.rs` `mod tests`, plus a title pair in `savings.rs`, `picker.rs`, `worksheet.rs`, `goal_form.rs` and `ledger.rs`

**Interfaces:**
- Consumes: `tui::label::Account` from Task 2.
- Produces:
  - `tui::label::Label`, `Clone + Debug + Default + PartialEq + Eq`, with `Label::plain(impl Into<String>) -> Label`, `fn text(self, impl Into<String>) -> Label`, `fn account(self, Account) -> Label`, `pub fn plain_text(&self) -> String`, `pub fn accounts(&self) -> Vec<&Account>`, `impl From<&str>`, `impl From<String>`.
  - `tui::label::label_line(&Label) -> TextLine<'static>`, `pub(super)`.
  - `form::render_fields(&mut Frame, impl Into<Label>, Vec<TextLine<'static>>) -> Rect`.

- [ ] **Step 1: Write the failing tests**

In `src/tui/label.rs`'s `mod tests`:

```rust
/// The point of the type: a title that names an account carries it as an
/// account, so the render half can tint it. A title already flattened to a
/// String could not be tinted by anything.
#[test]
fn a_label_keeps_its_accounts_apart_from_its_text() {
    let all = accounts();
    let label = Label::plain("Savings · ")
        .account(Account::named(&all, AccountId(2)))
        .text(" · Aug 2026");
    assert_eq!(label.plain_text(), "Savings · Nest Egg · Aug 2026");
    assert_eq!(label.accounts().len(), 1);
    assert_eq!(label.accounts()[0].id(), AccountId(2));
}

/// Exactly the account segments are colored. A title is mostly chrome, and
/// chrome in an account's color would say the whole line is about that
/// account rather than the two words that are.
#[test]
fn only_the_account_segments_of_a_label_are_colored() {
    let all = accounts();
    let label = Label::plain("Savings · ").account(Account::named(&all, AccountId(2)));
    let colors: Vec<Option<style::Color>> = label_line(&label)
        .spans
        .iter()
        .map(|s| s.style.fg)
        .collect();
    assert_eq!(
        colors,
        vec![
            None,
            Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
        ]
    );
}

/// A title naming nothing is still a Label, so one function draws every
/// title rather than one for the plain ones and one for the rest.
#[test]
fn a_label_with_no_account_draws_as_plain_text() {
    let line = label_line(&Label::from("Edit goal"));
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].style.fg, None);
}
```

In `src/tui/savings.rs` — the container filter is set by loading the containers and stepping onto one, since `Savings` has `set_containers` and `next_container` rather than a direct setter:

```rust
/// The container in the title is the same account the Account column names,
/// so it is the same color there too -- a title is where a reader looks to
/// find out which container they are in.
#[test]
fn the_savings_title_names_its_container_as_an_account() {
    let mut savings = Savings::new(accounts(), today(), 14);
    savings.set_containers(vec![AccountId(1), AccountId(2)]);
    savings.next_container();
    let title = savings.title();
    assert_eq!(title.plain_text(), "Savings · Everyday");
    assert_eq!(title.accounts().len(), 1);
    assert_eq!(title.accounts()[0].id(), AccountId(1));
}

/// `All` is not an account and takes no color: coloring it would make the
/// unfiltered screen look like it was filtered to something.
#[test]
fn the_unfiltered_savings_title_names_no_account() {
    let savings = Savings::new(accounts(), today(), 14);
    assert_eq!(savings.title().plain_text(), "Savings · All");
    assert!(savings.title().accounts().is_empty());
}
```

In `src/tui/picker.rs`:

```rust
/// The picker's title is the only thing on screen naming the container its
/// goals will be created in, which is why the title carries a container at
/// all -- so it is the one word on the line worth a color.
#[test]
fn the_picker_title_names_the_container_it_creates_in() {
    let picker = Picker::new(entries(), Account::named(&accounts(), AccountId(2)));
    let title = picker.title();
    assert!(
        title.plain_text().contains("creates in Nest Egg"),
        "{}",
        title.plain_text()
    );
    assert_eq!(title.accounts().len(), 1);
    assert_eq!(title.accounts()[0].id(), AccountId(2));
}
```

In `src/tui/ledger.rs` — the ledger title names its account by **code**, because the title is a chain of filter terms rather than a heading:

```rust
/// The ledger's account filter names the account by code, and takes the same
/// color the Account column below it does -- the title is what says which
/// account the rows have been narrowed to.
#[test]
fn a_filtered_ledger_title_names_its_account_by_code() {
    let ledger = filtered_ledger(AccountId(1));
    let title = ledger.title();
    assert!(title.plain_text().contains("CHK"), "{}", title.plain_text());
    assert_eq!(title.accounts().len(), 1);
    assert_eq!(title.accounts()[0].id(), AccountId(1));
}

/// An unfiltered ledger is `All`, which is not an account.
#[test]
fn an_unfiltered_ledger_title_names_no_account() {
    assert!(unfiltered_ledger().title().accounts().is_empty());
}
```

Build `filtered_ledger` and `unfiltered_ledger` from whatever `Ledger` fixture and account-filter method that module's tests already use; the assertions are what matter.

`src/tui/worksheet.rs` and `src/tui/goal_form.rs` take the same pair, against `"post $200.00 to Nest Egg"` and `"New goal in Nest Egg"`. Both constructors take the container by name today; change that parameter to `Account` and pass `Account::named(&accounts, id)` at the `app.rs` call site.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib label && cargo test --lib title`
Expected: FAIL to compile — `cannot find struct Label in this scope`.

- [ ] **Step 3: Add `Label` to `src/tui/label.rs`**

```rust
/// A string a view-state type may return that still knows which of its words
/// are accounts.
///
/// A title cannot be a `String` and be tinted: `format!` flattens the account
/// into text and the color is gone before any render function is reached,
/// which is how every uncolored title on this app got that way. It cannot be
/// a ratatui `Line` either -- view-state types hold no ratatui, so
/// `Savings::title` could not return one. So it is neither: a sequence of
/// plain runs and [`Account`]s, which [`label_line`] turns into spans and
/// which a test can read without a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label(Vec<Segment>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Plain(String),
    Account(Account),
}

impl Label {
    /// Opens a label with some text. [`Label::text`] appends more of it, and
    /// [`Label::account`] appends the one kind of segment that takes a color.
    pub fn plain(text: impl Into<String>) -> Label {
        Label(vec![Segment::Plain(text.into())])
    }

    pub fn text(mut self, text: impl Into<String>) -> Label {
        self.0.push(Segment::Plain(text.into()));
        self
    }

    pub fn account(mut self, account: Account) -> Label {
        self.0.push(Segment::Account(account));
        self
    }

    /// The whole label as text, for the assertions that check wording.
    ///
    /// Not a `Display` impl: this type exists to stop an account being
    /// flattened by accident, and a `Display` on the thing that holds one
    /// would put the flattening back within reach of a `format!`.
    pub fn plain_text(&self) -> String {
        self.0
            .iter()
            .map(|s| match s {
                Segment::Plain(text) => text.as_str(),
                Segment::Account(account) => account.as_text(),
            })
            .collect()
    }

    /// The accounts this label names, for the assertions that check a title
    /// names one *as an account* rather than as text.
    pub fn accounts(&self) -> Vec<&Account> {
        self.0
            .iter()
            .filter_map(|s| match s {
                Segment::Account(account) => Some(account),
                Segment::Plain(_) => None,
            })
            .collect()
    }
}

impl From<&str> for Label {
    fn from(text: &str) -> Label {
        Label::plain(text)
    }
}

impl From<String> for Label {
    fn from(text: String) -> Label {
        Label::plain(text)
    }
}

/// A label as a line of spans, with every account segment in its own color
/// and nothing else colored at all.
///
/// The second of this module's two exits, beside [`account_cell`], and the
/// same guarantee: an account that reaches a screen through here is colored.
pub(super) fn label_line(label: &Label) -> TextLine<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    TextLine::from(
        label
            .0
            .iter()
            .map(|segment| match segment {
                Segment::Plain(text) => Span::raw(text.clone()),
                Segment::Account(account) => Span::styled(
                    account.as_text().to_string(),
                    Style::default().fg(account.color()),
                ),
            })
            .collect::<Vec<Span<'static>>>(),
    )
}
```

Both `plain_text` and `label_line` reach the text through `Account::as_text`, the private reader Task 2 defined. No new accessor is needed and none may be added.

- [ ] **Step 4: Convert the titles**

`src/tui/savings.rs`:

```rust
    pub fn title(&self) -> Label {
        let mut title = match self.container {
            None => Label::plain("Savings · All"),
            Some(id) => Label::plain("Savings · ").account(Account::named(&self.accounts, id)),
        };
        if let Some(month) = self.month.selected() {
            title = title.text(format!(" · {}", month.label()));
        }
        title
    }
```

Its render becomes `Block::bordered().title(label_line(&savings.title()))`. The footer at `:411` stays plain text and keeps calling `Savings::account_name`.

`src/tui/picker.rs` — `Picker` holds `container_name: String`; change it to `container: Account`, built at the `app.rs` call site with `Account::named(&accounts, id)`:

```rust
    pub fn title(&self) -> Label {
        Label::plain(format!(
            "Recurring goals — {} selected · Space toggles · Enter creates in ",
            self.selected_count()
        ))
        .account(self.container.clone())
        .text(" · Esc cancel")
    }
```

`src/tui/worksheet.rs` — the same substitution for its `container_name`:

```rust
    pub fn title(&self) -> Label {
        let kind = match self.kind {
            BatchKind::Paycheck => "Payday",
            BatchKind::Interest => "Interest",
            BatchKind::Adhoc => "Allocate",
            BatchKind::Import => "Import",
        };
        Label::plain(format!("{kind} — post {} to ", self.amount))
            .account(self.container.clone())
            .text(" · Tab field · Enter commit · Esc cancel")
    }
```

`src/tui/goal_form.rs` — `Subject::New { container_name, .. }` becomes `Subject::New { container: Account, .. }`:

```rust
    pub fn title(&self) -> Label {
        match &self.subject {
            Subject::Existing(_) => {
                Label::from("Edit goal — Tab field · Enter save · Esc cancel")
            }
            Subject::New { container, .. } => Label::plain("New goal in ")
                .account(container.clone())
                .text(" — Tab field · Enter save · Esc cancel"),
        }
    }
```

`src/tui/ledger.rs` — the filter chain, naming its account by code:

```rust
    pub fn title(&self) -> Label {
        let kind = match self.kind {
            Kind::Cash => "Cash",
            Kind::Credit => "Credit",
        };
        let mut title = Label::plain(format!("{kind} · {} · ", self.window.label()));
        title = match self.account {
            None => title.text("All"),
            Some(i) => title.account(Account::coded(&self.accounts, self.accounts[i].id)),
        };
        if !self.search().is_empty() {
            title = title.text(format!(" · /{}", self.search()));
        }
        title
    }
```

and `title_line` composes it with the balance it already appends:

```rust
fn title_line(ledger: &Ledger) -> TextLine<'static> {
    let mut spans = label_line(&ledger.title()).spans;
    spans.push(Span::raw(" · Today "));
    spans.push(money_span(ledger.total()));
    if let (Some(target), Some(delta)) = (ledger.target(), ledger.delta()) {
        spans.push(Span::raw(" · Target "));
        spans.push(money_span(target));
        spans.push(Span::raw(" · Δ "));
        spans.push(delta_span(delta));
    }
    TextLine::from(spans)
}
```

`src/tui/app.rs` — the Reconcile modal's label:

```rust
        let label = Label::plain("Target · ")
            .account(Account::named(self.ledger().accounts(), id));
```

`ValueForm::new`'s `label` parameter becomes `impl Into<Label>` and the field becomes a `Label`, with `ValueForm::label()` returning `&Label`. Its other caller passes a `&str`, which `From<&str>` handles.

- [ ] **Step 5: Make `render_fields` take a `Label`**

```rust
pub(super) fn render_fields(
    frame: &mut Frame,
    title: impl Into<Label>,
    lines: Vec<TextLine<'static>>,
) -> Rect {
    let area = centered(frame.area(), FORM_WIDTH, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(label_line(&title.into()))),
        area,
    );
    area
}
```

Every existing caller passing a `&str` or a `String` keeps compiling through `From`. The converted titles pass a `Label`.

- [ ] **Step 6: Run the suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS. Existing title assertions become `plain_text()` reads — `destination.rs:511` and `:514`, and `goal_form.rs:920-921`.

- [ ] **Step 7: Check the widths still hold**

Run: `cargo test --lib width`
Expected: PASS. A `Label` renders the same glyphs as the `String` it replaces, so `MIN_WIDTH` is unaffected — this step confirms rather than fixes.

- [ ] **Step 8: Commit**

```bash
git add src/tui
git commit -m "Give titles a string that still knows which words are accounts

A title cannot be a String and be tinted, and it cannot be a ratatui Line
either, because view-state types hold no ratatui. Label is neither: plain
runs and accounts, which label_line turns into spans and a test reads
without a terminal.

The Savings, Ledger, Picker, Worksheet, new-goal and Reconcile titles name
their account in the same color the Account column does. The ledger names
its by code, the way its other filter terms read.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The three form selectors

**Files:**
- Modify: `src/tui/form.rs` (`TxnForm::display`, `TransferForm::display`, `field_line`, `field_line_noted`, `field_line_tinted`, `field_line_parts`, the `render_*` functions)
- Modify: `src/tui/recurring_txn.rs` (`RecurringTxnForm::display`)
- Modify: `src/tui/accounts.rs:281`, `src/tui/fund.rs:245`, `src/tui/goal_form.rs:121`, `:308`, `:459`, `src/tui/planning.rs:912`, `src/tui/recurring_goal.rs:231`
- Test: `src/tui/form.rs` `mod tests`, `src/tui/recurring_txn.rs` `mod tests`

**Interfaces:**
- Consumes: `Label`, `Account`, `label_line` from Task 3.
- Produces: every `display(field)` returns `Label`. `form::field_line(&str, Label, bool) -> TextLine<'static>` and `field_line_noted(&str, Label, bool, &str) -> TextLine<'static>`. `field_line_tinted` keeps its `(&str, String, bool, Color)` signature.

- [ ] **Step 1: Write the failing tests**

In `src/tui/form.rs`'s `mod tests` — its `accounts()` fixture is `CHK`/`Everyday` (id 1) and `SAV`/`Rainy Day` (id 2), and `all_accounts()` adds two cards:

```rust
/// A transaction's account is the same account the ledger's Account column
/// names behind the form, so it is the same color. The selector shows a code
/// and a name, and both are that account.
#[test]
fn the_account_selector_shows_one_colored_account() {
    let form = TxnForm::add(accounts(), today(), None).unwrap();
    let value = form.display(TxnField::Account);
    assert_eq!(value.plain_text(), "CHK — Everyday");
    assert_eq!(value.accounts().len(), 1);
    assert_eq!(value.accounts()[0].id(), AccountId(1));
}

/// A date is not an account and takes no color. The uniform `Label` return is
/// about having one shape per form, not about tinting every field.
#[test]
fn a_forms_ordinary_fields_name_no_account() {
    let form = TxnForm::add(accounts(), today(), None).unwrap();
    assert!(form.display(TxnField::Date).accounts().is_empty());
    assert!(form.display(TxnField::Amount).accounts().is_empty());
    assert!(form.display(TxnField::Description).accounts().is_empty());
}

/// Both ends of a transfer, so money moving between two containers is
/// readable at a glance rather than by reading two codes.
#[test]
fn both_ends_of_a_transfer_name_their_own_account() {
    let form = TransferForm::transfer(all_accounts(), today()).unwrap();
    let from = form.display(TransferField::From);
    let to = form.display(TransferField::To);
    assert_eq!(from.accounts().len(), 1);
    assert_eq!(to.accounts().len(), 1);
}
```

In `src/tui/recurring_txn.rs`'s `mod tests`:

```rust
/// The form's selector names the same account its Account column does, in
/// the same color -- the column shows a code and the form shows both halves,
/// but they are one account.
#[test]
fn the_recurring_transaction_selector_shows_one_colored_account() {
    let form = RecurringTxnForm::add(accounts(), today()).unwrap();
    let value = form.display(RecurringTxnField::Account);
    assert_eq!(value.plain_text(), "CHK — Everyday");
    assert_eq!(value.accounts().len(), 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib selector`
Expected: FAIL — `no method named plain_text found for struct String`.

- [ ] **Step 3: Convert the three account-bearing forms**

`src/tui/form.rs`:

```rust
    pub fn display(&self, field: TxnField) -> Label {
        match field {
            TxnField::Date => Label::from(self.date.value()),
            TxnField::Amount => Label::from(self.amount.value()),
            TxnField::Description => Label::from(self.description.value()),
            TxnField::Account => match self.accounts.get(self.account) {
                Some(a) => Label::default().account(Account::labelled(a)),
                None => Label::default(),
            },
        }
    }
```

`Label::default()` is the empty label, which is what a selector with nothing selected draws as — and `Label::default().account(..)` is a one-segment label.

```rust
    pub fn display(&self, field: TransferField) -> Label {
        let account = |list: &[account::Account], i: usize| match list.get(i) {
            Some(a) => Label::default().account(Account::labelled(a)),
            None => Label::default(),
        };
        match field {
            TransferField::Date => Label::from(self.date.value()),
            TransferField::Amount => Label::from(self.amount.value()),
            TransferField::Description => Label::from(self.description.value()),
            TransferField::From => account(&self.from_accounts, self.from),
            TransferField::To => account(&self.to_accounts, self.to),
        }
    }
```

`src/tui/recurring_txn.rs`:

```rust
    pub fn display(&self, field: RecurringTxnField) -> Label {
        match field {
            RecurringTxnField::Description => Label::from(self.description.value()),
            RecurringTxnField::Amount => Label::from(self.amount.value()),
            RecurringTxnField::Anchor => Label::from(self.anchor.value()),
            RecurringTxnField::Horizon => Label::from(self.horizon.value()),
            RecurringTxnField::Account => match self.accounts.get(self.account) {
                Some(a) => Label::default().account(super::Account::labelled(a)),
                None => Label::default(),
            },
            RecurringTxnField::Cadence => Label::from(Cadence::ALL[self.cadence].as_str()),
        }
    }
```

Delete the three `FIXME(task 4)` comments Task 1 left.

- [ ] **Step 4: Convert the seven account-free forms**

One shape per form rather than two, so these change their return type and wrap the existing match. `src/tui/accounts.rs:281`:

```rust
    pub fn display(&self, field: AccountField) -> Label {
        Label::plain(match field {
            AccountField::Name => self.name.value().to_string(),
            AccountField::Color => match color_choices()[self.color] {
                None => "—".to_string(),
                Some(color) => color.label().to_string(),
            },
            AccountField::Band => Group::bands(self.kind)[self.band].label().to_string(),
            AccountField::Order => format!("{} of {}", self.position + 1, self.of_kind),
            AccountField::Interest => InterestPolicy::ALL[self.policy].label().to_string(),
            AccountField::Savings => match savings_choices()[self.block] {
                None => "—".to_string(),
                Some(block) => block.label().to_string(),
            },
        })
    }
```

Do the same for `fund.rs:245`, `goal_form.rs:121`, `:308`, `:459`, `planning.rs:912` and `recurring_goal.rs:231` — wrap the whole `match` in `Label::plain(...)`, change the return type, no other edit.

The Accounts screen's `Color` field keeps going through `field_line_tinted` rather than through the `Label`. Its tint says what `Teal` looks like, not which account this is; folding it into the account tint would make one mechanism carry two meanings. That call site therefore reads `form.display(AccountField::Color).plain_text()` to get its `String`.

- [ ] **Step 5: Make `field_line` take a `Label`**

```rust
/// One labelled input line; the focused one carries a caret.
pub(super) fn field_line(label: &str, value: Label, focused: bool) -> TextLine<'static> {
    field_line_noted(label, value, focused, "")
}

/// The same, with a note past the caret -- what the field comes to, where its
/// text is an expression rather than the figure itself. An empty note draws
/// nothing, trailing space included.
pub(super) fn field_line_noted(
    label: &str,
    value: Label,
    focused: bool,
    note: &str,
) -> TextLine<'static> {
    let mut spans = vec![Span::raw(format!("{label:>12}  "))];
    spans.extend(label_line(&value).spans);
    spans.push(Span::raw(trailer(focused, note)));
    TextLine::from(spans)
}

/// The same, with the *value* drawn in a color -- the one field whose text is
/// a name for something the form cannot otherwise show. The Accounts screen's
/// `Color` selector cycles eight names, and a name is not a color: drawing
/// `Teal` in teal is what makes the choice answerable without saving it and
/// looking.
///
/// Only the value is tinted. The label and the caret are chrome and belong to
/// the form rather than to the field's content.
pub(super) fn field_line_tinted(
    label: &str,
    value: String,
    focused: bool,
    color: Color,
) -> TextLine<'static> {
    TextLine::from(vec![
        Span::raw(format!("{label:>12}  ")),
        Span::styled(value, Style::default().fg(color)),
        Span::raw(trailer(focused, "")),
    ])
}

/// The caret and the note that follow every field's value.
fn trailer(focused: bool, note: &str) -> String {
    let caret = if focused { "▌" } else { "" };
    if note.is_empty() {
        caret.to_string()
    } else {
        format!("{caret}  {note}")
    }
}
```

`field_line_parts` goes away: with `field_line_tinted` no longer sharing the value handling, the only thing the two have in common is the trailer, which `trailer` now states once.

- [ ] **Step 6: Run the suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Confirm the form is still the right width**

Run: `cargo test --lib width`
Expected: PASS. `FORM_WIDTH` is unchanged and the selector shows the same `CHK — Everyday` text it showed before.

- [ ] **Step 8: Commit**

```bash
git add src/tui
git commit -m "Tint the account in the three form selectors

Every display() returns a Label, so a form has one shape rather than one
for the fields that name an account and one for the rest. The code and the
name are one segment and take one color: both halves name the same
account, and splitting them would leave the code reading as chrome in
front of a colored name.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Pin the escape hatch

`as_str` exists and no type erases it. What this task buys is that a new one is a reviewed act rather than a quiet one.

**Files:**
- Modify: `src/tui/label.rs` `mod tests` (the guard test lives with the guarantee it guards)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing other code uses.

- [ ] **Step 1: Write the guard test**

In `src/tui/label.rs`'s `mod tests`:

```rust
/// `AccountName::as_str` is the one way past `Account`, and it exists for the
/// uses that are not displays of an account -- a description prefill, a
/// search filter folding case, a form seeding its editable field. Every one
/// of those is listed here, so a new one has to be added deliberately and is
/// visible in the diff that adds it.
///
/// A source scan rather than a type, because the property is "nobody reached
/// for the escape hatch", which no signature can state. It is the weakest
/// link in the guarantee and says so.
#[test]
fn nothing_in_the_screens_reads_an_account_name_as_bare_text() {
    let sanctioned = [
        // Prefills a description with the card's code. Not a display of an
        // account: it seeds an editable field the owner then owns.
        ("form.rs", "{} Payment"),
        // Names the source in an error about a transfer to itself. Errors are
        // prose, and the status line is uncolored.
        ("form.rs", "from.code.as_str()"),
        // Seeds the Accounts form's editable Name field.
        ("accounts.rs", "Field::given(account.name.as_str()"),
        // The reconciliation footer, a status strip rather than a place a
        // reader looks to identify an account.
        ("savings.rs", "map_or(\"?\", |a| a.name.as_str())"),
        // Its twin on Recurring Transactions.
        ("recurring_txn.rs", "map_or(\"?\", |a| a.code.as_str())"),
    ];

    let mut found: Vec<String> = Vec::new();
    for (file, source) in tui_sources() {
        for (number, line) in source.lines().enumerate() {
            if !line.contains(".name.as_str()") && !line.contains(".code.as_str()") {
                continue;
            }
            if sanctioned
                .iter()
                .any(|(f, needle)| *f == file && line.contains(needle))
            {
                continue;
            }
            found.push(format!("{file}:{}: {}", number + 1, line.trim()));
        }
    }

    assert!(
        found.is_empty(),
        "an account's text is read as a bare string here, which is how one \
         reaches a screen with no color on it. Draw it through `Account` \
         instead, or add the site to `sanctioned` with a comment saying why \
         it is not a display:\n{}",
        found.join("\n")
    );
}

/// Every `src/tui/*.rs`, as (file name, contents).
fn tui_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tui");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src/tui is readable") {
        let path = entry.expect("a readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a named file")
            .to_string();
        out.push((name, std::fs::read_to_string(&path).expect("a readable file")));
    }
    out
}
```

The list is keyed by file name and a distinctive fragment of the line, so a site that moves within its file stays sanctioned and one that moves to another file does not. `label.rs` itself is exempt by construction: `Account::as_text` is not `as_str`, so the scan never sees it.

- [ ] **Step 2: Run the test**

Run: `cargo test --lib nothing_in_the_screens`
Expected: PASS, if Tasks 1-4 left no unsanctioned site. That is a real result — but a guard test that has never failed is not yet known to work, which is what Step 3 is for.

- [ ] **Step 3: Verify the test can actually fail**

Add a throwaway line to any `src/tui/*.rs` function body:

```rust
        let _leak = self.accounts[0].name.as_str();
```

Run: `cargo test --lib nothing_in_the_screens`
Expected: FAIL, naming that file and line. Delete the line and confirm it passes again. A guard test that cannot fail is worse than no guard test, because it reads as coverage.

- [ ] **Step 4: Run the suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tui/label.rs
git commit -m "Pin the sanctioned readers of an account's bare text

as_str is the one way past Account and no type erases it, so the sites
that use it are listed with a reason each. A new one now has to be added
deliberately and shows up in the diff that adds it.

A source scan rather than a type, because the property is 'nobody reached
for the escape hatch', which no signature can state.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Documentation

**Files:**
- Modify: `src/tui/CLAUDE.md` (the account-color section, around line 248)
- Modify: `CLAUDE.md` (the architecture table's `src/tui/` row; the account-color invariant)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Rewrite the color section of `src/tui/CLAUDE.md`**

Replace the bullet at `src/tui/CLAUDE.md:248` with one stating the arrangement as it now is. Describe the code, not the change — no "used to", no "now".

```markdown
- **An account cannot reach a screen without its color, and that is a property of the types
  rather than a rule to remember.** `label.rs` holds `Account` — an account's id, the text a
  screen shows for it, and the owner's color — with **no reader for the text outside that file**.
  Its two exits are `account_cell` and `label_line`, and both color what they draw, so a screen
  that wants an uncolored account has no way to ask for one. One layer down,
  `db::account::AccountName` and `AccountCode` have no `Display`, so an account cannot become a
  `String` on the way to a `format!` either.
  - **`Account::named`, `Account::coded` and `Account::labelled` are one per caller.** The
    ledgers' rows, Savings, Overview and Accounts show a name; Recurring Transactions and the
    ledger *title* show a code, the one because its other columns pin the row down already and
    the other because a title is a chain of filter terms; the three form selectors show
    `CHK — Everyday` as one segment, because both halves name the same account and splitting
    them would leave the code reading as chrome in front of a colored name.
  - **`Label` is what lets a title carry a tint.** A title cannot be a `String` and be colored,
    and it cannot be a ratatui `Line` because view-state types hold no ratatui. So it is a
    sequence of plain runs and `Account`s: `Savings::title`, `Ledger::title`, `Picker::title`,
    `Worksheet::title`, the new-goal title and the Reconcile modal's label all return one, and
    `label_line` turns one into spans.
  - **Every `display(field)` returns a `Label`**, including the fields that name no account. One
    shape per form rather than one for the account fields and one for the rest. The Accounts
    screen's `Color` field is the exception and still goes through `field_line_tinted`: its tint
    says what `Teal` looks like, not which account this is.
  - **The status line and the Savings reconciliation footer are deliberately uncolored.** They
    are transient prose rather than places a reader looks to identify an account, and
    `Savings::account_name` exists for the footer alone.
  - **`as_str` is the escape, and it is pinned.** `AccountName::as_str` serves the uses that are
    not displays — a description prefill, a search filter folding case, a form seeding its
    editable field — and `nothing_in_the_screens_reads_an_account_name_as_bare_text` lists them
    with a reason each. A source scan rather than a type, because the property is "nobody reached
    for the escape hatch", which no signature can state.
```

Leave the existing bullets about `style::palette`, `AccountColor::derived` and the `Color` field on the Accounts form exactly as they are — they describe decisions this work does not touch.

- [ ] **Step 2: Update the root `CLAUDE.md`**

In the architecture table, the `src/tui/` row gains a clause after "`ratatui`/`crossterm` are named only here": `An account reaches a screen only as `label::Account`, which colors it.`

And append to the invariant beginning "**An account's color is a name, and having none is a supported state**":

```markdown
  Which screens honour that is not left to each screen: `tui::label::Account` is the only way an
  account reaches a glyph and it colors what it draws, and `AccountName`/`AccountCode` have no
  `Display`, so an account cannot be flattened into a `String` on the way.
```

- [ ] **Step 3: Check the docs against the code**

Run: `grep -n "account_color_of\|account_cell\|account_code" src/tui/CLAUDE.md CLAUDE.md`
Expected: no hit naming a function this plan deleted. `account_cell` survives with a new signature and may legitimately appear.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md src/tui/CLAUDE.md
git commit -m "Document where an account's color is made unavoidable

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7 (optional): Fold `planning::Tint` into `Label`

**Reject this task freely.** The Planning screen is correct without it. What it buys is one mechanism instead of two for "this cell names an account"; what it costs is touching the screen with the most delicate color rule in the app — a tone outranks a tint, because red says the plan will not run and amber says there is a gap worth filling, and neither may be displaced by a tint that only says which account.

**Files:**
- Modify: `src/tui/label.rs` (add `label_line_toned`, `Account::of_container`)
- Modify: `src/tui/planning.rs` (delete `Tint`, `Tint::of`, `Tint::color_in`, `Row::account`; the three cells become `Label`)
- Modify: `src/transfer.rs` (`Container.name` becomes `AccountName`)
- Test: `src/tui/label.rs` `mod tests`, and the three existing Planning tint tests

**Interfaces:**
- Consumes: `Label`, `Account` from Task 3.
- Produces: `tui::label::label_line_toned(&Label, style::Tone) -> TextLine<'static>` and `Account::of_container(&transfer::Container) -> Account`.

- [ ] **Step 1: Write the failing test**

```rust
/// A tone outranks a tint. Red says this plan will not run and amber says
/// there is a gap worth filling; a tint only says which account, and a cell
/// carrying both must say the more urgent thing.
#[test]
fn a_toned_cell_ignores_the_account_it_names() {
    let label = Label::default().account(Account::named(&accounts(), AccountId(2)));
    let line = label_line_toned(&label, style::Tone::Negative);
    assert_eq!(line.spans[0].style.fg, Some(style::NEGATIVE));
}

/// With no tone, the account's own color is what draws -- the precedence is
/// a precedence, not a replacement.
#[test]
fn an_untoned_cell_keeps_its_accounts_color() {
    let label = Label::default().account(Account::named(&accounts(), AccountId(2)));
    let line = label_line_toned(&label, style::Tone::Plain);
    assert_eq!(
        line.spans[0].style.fg,
        Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
    );
}
```

Keep the three existing Planning tint tests at `planning.rs:1637`, `:1681` and `:1788`, converted to read `Label`s.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib toned`
Expected: FAIL — `cannot find function label_line_toned`.

- [ ] **Step 3: Add `label_line_toned` and `Account::of_container`**

```rust
/// A label as a line, with a tone that outranks every account's own color.
///
/// The Planning screen's value column is heterogeneous and its tones carry
/// *instructions* -- red says the plan will not run, amber says there is a
/// gap worth filling -- where a tint only says which account. So the tone is
/// read first, and an account named in a toned cell draws in the tone.
pub(super) fn label_line_toned(label: &Label, tone: style::Tone) -> TextLine<'static> {
    let Some(tone) = style::tone_color(tone) else {
        return label_line(label);
    };
    let mut line = label_line(label);
    for span in line.spans.iter_mut() {
        span.style = ratatui::style::Style::default().fg(tone);
    }
    line
}
```

and, beside the other three constructors:

```rust
    /// The account a [`crate::transfer::Container`] names.
    ///
    /// `Container` carries an id, a name and a color, which is this type's
    /// three fields -- it is what the plan layer hands the Planning screen so
    /// that screen can tint what it draws.
    pub fn of_container(container: &crate::transfer::Container) -> Account {
        Account {
            id: container.id,
            text: container.name.as_str().to_string(),
            color: container.color,
        }
    }
```

Make `transfer::Container.name` an `AccountName`, which lets `transfer.rs:301`, `:607` and `:1017` drop the `.as_str().to_string()` Task 1 left there.

- [ ] **Step 4: Convert `planning::Row`**

`Row`'s `label`, `value` and `extra` become `Label`, and `account: Option<Tint>` goes away entirely — a `Label` says structurally which cell names an account, which is all `Tint`'s `column` field was for. `destination` builds them directly:

```rust
    fn destination(w: &Wiring) -> Row {
        let (value, extra) = match &w.landing {
            Landing::Goal { goal, container } => (
                Label::from(goal.as_str()),
                Label::default().account(Account::of_container(container)),
            ),
            Landing::Account { account } => (
                Label::default().account(Account::of_container(account)),
                Label::default(),
            ),
            Landing::Spread { container } => (
                Label::from("spread"),
                Label::default().account(Account::of_container(container)),
            ),
            Landing::Ambiguous { containers } => (
                Label::from("ambiguous"),
                Label::from(match containers.len() {
                    0..=2 => containers.join(", "),
                    n => format!("{n} containers"),
                }),
            ),
            // the remaining arms wrap their existing String in Label::from
            // and pass Label::default() where they name nothing
        };
        // the rest of the function unchanged
    }
```

and `render` reads:

```rust
            let row = TableRow::new(vec![
                Cell::from(label_line(&r.label)),
                Cell::from(label_line_toned(&r.value, r.tone).right_aligned()),
                Cell::from(label_line(&r.extra).right_aligned()),
            ]);
```

- [ ] **Step 5: Run the suite**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: PASS, including the three converted Planning tint tests.

- [ ] **Step 6: Run against the workbook**

Run: `MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<workbook> MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test`
Expected: PASS. This task touches `src/transfer.rs`, which the waterfall reaches, so the workbook oracle is not optional here. Ask the owner for the three values if they are not already in the environment.

- [ ] **Step 7: Update the docs and commit**

Remove the `src/tui/CLAUDE.md` bullet describing `Tint` and its `Column`, and extend the Task 6 bullet to say Planning's cells are `Label`s whose tone outranks the account color. Then:

```bash
git add src/tui src/transfer.rs src/tui/CLAUDE.md
git commit -m "Say which Planning cell names an account structurally

A Label already knows which of its segments is an account, so the parallel
Tint -- a column, an id and a color beside three String cells -- has
nothing left to say. The tone still outranks the account color: red says
the plan will not run and amber says there is a gap worth filling, and
neither may be displaced by a tint that only says which account.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```
