# Guaranteeing an account is colored wherever it is displayed

## The problem

`style::account_color` is the one decision about what an account looks like, and
`tui::account_cell` carries it into a table cell. Every table that names an account already
goes through it. Two shapes do not:

| Shape | Sites |
|---|---|
| Form field values | `TxnForm::Account`, `TransferForm::From`/`To`, `RecurringTxnForm::Account` |
| Titles | `Savings::title`, `Ledger::title`, `Picker::title`, `Worksheet::title`, `GoalForm`'s "New goal in X", the Reconcile modal's `Target · X` |

The cause is structural rather than oversight. All three build a `String`, and a `String` cannot
carry a tint: `account_name` and `account_code` return `&str`, so `format!` swallows the account
and the color is gone before any render function is reached. Every uncolored site is written the
same way — `format!("{} — {}", a.code, a.name)`, `format!("Target · {}", ledger.account_name(id))` —
which is why adding the missing tints by hand fixes today's screens and guarantees nothing about
tomorrow's.

## Scope

Table cells, form field values, and every screen or modal title that names an account or a
container. `App::status` messages and the Savings reconciliation footer stay plain text: they are
transient prose rather than something a reader scans to identify an account.

## 1. `tui::Account` — one value where three fields are today

```rust
pub struct Account {
    id: AccountId,
    text: String,
    color: Option<AccountColor>,
}
```

Private fields, and **no `Display`, no `Deref`, no `as_str`**. The only exit is the render half.
This is what makes the failure mode unwritable: a title cannot `format!` an account into itself.

Three constructors, one caller each:

```rust
impl Account {
    /// `Everyday` — the ledgers, Savings, Overview, Accounts.
    pub fn named(accounts: &[account::Account], id: AccountId) -> Account;
    /// `CHK` — Recurring Transactions, whose other columns pin the row down already.
    pub fn coded(accounts: &[account::Account], id: AccountId) -> Account;
    /// `CHK — Everyday` — the three form selectors, which show both halves.
    pub fn labelled(account: &account::Account) -> Account;

    pub fn id(&self) -> AccountId;
}
```

`named` and `coded` keep the `?` fallback for an id with no account: a corrupt row is not a reason
to stop drawing a screen, and `style::account_color` still gives it the shade its id derives.

It replaces the trio `account_code` / `account_name` / `account_color_of`, and collapses the
parallel fields two screens have already grown — `savings::Row { account_name, container,
account_color }` and `recurring_txn::Row { account_code, account_id, account_color }` each become
one `Account`. Those clumps are three values that must agree, kept in step by hand.

### The database's `Account` and this one

`db::account::Account` keeps its name; the six `src/tui/` modules that import it switch to the
module-qualified `account::Account`. Two layers each have a right name for the thing they hold,
and qualification says which layer a signature is in — the same resolution as
`ratatui::text::Line as TextLine` already in the tree.

## 2. `tui::Label` — a tinted string the view state may hold

View-state types hold no ratatui, so a title that carries a tint needs a text type that is not
`Line`:

```rust
pub struct Label(Vec<Segment>);

enum Segment {
    Plain(String),
    Account(Account),
}
```

`plain` opens a `Label` with some text and `text` appends more of it; `account` appends the one
segment that gets a tint. Built in reading order, so a title's source reads like the title:

```rust
Label::plain("Savings · ").account(container).text(" · Aug 2026")
```

Unit-testable without a terminal, like everything else here: `plain_text()` for the assertions
that check wording, `accounts()` for the ones that check which account a title names.

## 3. Where the guarantee is discharged

`Account`, `Label`, and their two render functions live in one module, `src/tui/label.rs`:

```rust
pub(super) fn account_cell(account: &Account) -> Cell<'static>;
pub(super) fn label_line(label: &Label) -> TextLine<'static>;
```

Both tint through `style::account_color`. Because the `text` field has no accessor outside this
module, these two functions are the only path from an account to the screen, and both color it.
A `#[cfg(test)] pub fn text(&self)` exists for assertions and for nothing else.

Not `src/tui/account.rs`: one character from `accounts.rs`, which is the Accounts screen.

### Signatures that change

- `title()` on `Savings`, `Ledger`, `Picker`, `Worksheet` and `GoalForm` returns `Label`.
- `display(field)` on the forms returns `Label`. Uniformly, including the fields that name no
  account — one shape per form rather than two.
- `render_fields`'s title and `field_line`'s value take `Label`. Titles naming nothing pass
  `Label::plain("…")`.

`field_line_tinted` stays exactly as it is, for the Accounts screen's `Color` field. Its tint says
"this is what `Teal` looks like", not "this is which account"; folding it into the account tint
would make one mechanism carry two meanings.

## 4. Closing the field hole

`tui::Account` guarantees that *what goes through it* is colored. It cannot stop a screen holding
an `account::Account` from reaching past it. So in `db::account`:

```rust
pub struct AccountName(String);
pub struct AccountCode(String);
```

**Neither implements `Display`.** That is the entire content of the change, and it is enough:
`format!("{} — {}", a.code, a.name)` is verbatim how three of today's uncolored sites are written,
and it stops compiling everywhere in the crate.

- `PartialEq<&str>` so the existing assertions read unchanged.
- `FromSql`/`ToSql` so `from_row` and every query are untouched.
- `insert` and `set_name` keep taking `&str`. The write path never had this problem.

## 5. Where the guarantee ends

One reader remains, `as_str()`, and no Rust type erases it. What the design buys is that an
uncolored account display becomes something a person deliberately typed `as_str()` to get, rather
than something `format!` did silently.

Roughly six uses are legitimate and are not displays of an account:

- `form::refresh_payment_description` — `"{code} Payment"` prefills a description.
- The Savings and destination search filters, lowercasing a name to match against.
- `AccountForm::edit`, seeding the editable `Field` with the current name.
- `transfer::diagnose`'s text report, outside `src/tui/` entirely.
- The `App::status` messages this design scopes out.

A guard test over the `src/tui/` sources pins that list, so a seventh is a reviewed act rather than
a quiet one. This is the one part of the design that is a source-grep test rather than a type, and
it is severable: without it the newtypes still hold, and the residual risk is a deliberate
`as_str()` passing review unnoticed.

## 6. Testing

Existing coverage carries most of the weight — the width tests still pass unchanged, since a
`Label` renders the same glyphs, and the column-tint tests in `savings.rs`, `planning.rs` and
`recurring_txn.rs` keep asserting what they assert. New tests:

- `label_line` tints exactly the account segments and leaves the plain ones alone, asserted on
  spans rather than on a buffer.
- Every `title()` that names an account carries it as an `Account` segment rather than as text —
  `assert!(!savings.title().accounts().is_empty())`, one per screen.
- An id with no account still yields a colored `Account` showing `?`.
- The three form selectors' values arrive as a single `Account` segment reading `CHK — Everyday`.

## 7. Order of work

Four stages, each compiling with the suite green.

1. **`AccountName` and `AccountCode`**, and `as_str()` at the read sites. Nothing changes on
   screen. The compiler errors from this stage are the complete inventory of every account display
   in the crate — a better list than any grep, and worth having before stage 3.
2. **`tui::Account`**, plus `account_cell(&Account)`. Collapse the three-field rows. Tables render
   identically.
3. **`Label` and `label_line`.** Convert the titles and the form values. This is the stage where
   the screens change.
4. **Docs.** The color section of `src/tui/CLAUDE.md`, and the root `CLAUDE.md` invariant about an
   account being one color everywhere.
