# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build
cargo test
cargo test shortfall                       # single test / substring filter
cargo test --test planning_from_workbook   # one integration test binary
cargo test --lib                           # unit tests only (no workbook needed)
cargo fmt                                  # pre-commit hook runs `cargo fmt --check`
cargo clippy --all-targets -- -D warnings  # CI treats warnings as errors

cargo run --bin mm -- import <workbook>
cargo run --bin mm -- --db /tmp/scratch.db --today 2026-08-12 import --replace <workbook>

# The workbook-oracle tests need the workbook *and* the three accounts no cell
# names: the current account, then the two `Savings` block containers, by code.
# Neither the path nor the codes may live in this repository, so both are read
# from the environment.
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=<workbook> \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test
```

`pre-commit install` wires up `.pre-commit-config.yaml`. CI (`.github/workflows/ci.yml`) delegates to
shared reusable workflows: `rust-ci.yml` runs build, test, `cargo fmt --check`, and clippy with
`-D warnings`; `terraform-ci.yml` runs `terraform fmt -check -recursive`, then
`terraform init -backend=false` and `terraform validate`, matching the `terraform_fmt`/`terraform_validate` pre-commit hooks.
Terraform is never *applied* by CI.

## The workbook is the test oracle

The app replaces a `Money.xlsx` workbook. Integration tests in `tests/` open that real workbook and
assert against **its own cached cell values**, not hardcoded balances — the owner keeps editing it,
so literals would rot. The workbook is personal financial data and must never be committed.

`MM_WORKBOOK` says where it is, and there is deliberately no default: where the owner keeps their
finances is the same kind of fact as an account code, and a fallback path would put it back in the
repository the one place nobody would think to grep. `tests/common/mod.rs` is what reads it.

Tests skip loudly when it is unset or the file is absent, so a clean checkout passes.
`MM_REQUIRE_WORKBOOK=1` turns either into a hard failure instead. When changing anything the
importer or the waterfall touches, run with that set — otherwise the tests that actually exercise
it silently no-op.

The workbook's own layout and formulas are documented next to the code that reproduces them:
`src/import/CLAUDE.md` maps every sheet, block, and cell reference the importer reads;
`src/calc/CLAUDE.md` carries the formula derivations, the rounding policy, and the Planning
waterfall's ordering and gates. Read the relevant one before touching either module. The golden
values themselves stay in the tests, where they are asserted against the workbook rather than
restated.

`src/tui/CLAUDE.md` sits beside those two and answers a different set of questions: what a key is
allowed to mean, what each screen owns, and how much width it may spend. The app is driven entirely
by single keystrokes, so the same action takes the same key on every screen that offers it. Read it
before touching anything under `src/tui/` — nothing about the screens is documented here.

## No real data in the repository

The repository is public; the owner's finances are not. **Nothing committed here may carry a real
figure, a real institution, or a name that identifies a real person.** Four categories, banned
outright from every tracked file — source, tests, fixtures, docs, `README.md`, commit messages and
PR text alike:

- **Balances and amounts.** Every money literal in the crate is invented. A figure copied off the
  workbook leaks even as a test fixture, because a fixture that reproduces an asserted result *is*
  the balance it was taken from.
- **Institution names.** No bank, brokerage, or card issuer the owner actually holds — not in a
  fixture, not in a doc example, not in a comment explaining where a number came from.
- **Account codes.** The short codes the workbook's `Constants` sheet carries name real accounts at
  real institutions. They are data, not schema, and they never appear in a tracked file. The
  workbook's own *path* is the same kind of fact — it names where the owner keeps their money — so
  it is read from `MM_WORKBOOK` rather than written down.
- **Goal names traceable to a real person.** A goal named for a family member, an address, or an
  employer. A generic name a real goal happens to share is fine; what is banned is the name that
  identifies someone.

Real data lives in exactly two places, and neither is tracked: the owner's SQLite database, and the
`Money.xlsx` workbook `MM_WORKBOOK` points at. That is what makes this a boundary rather than a ban
on test data.
The workbook-oracle tests in `tests/` still assert against real values — they read them out of the
untracked file at run time instead of restating them. A test that needs to *name* something in the
workbook resolves it structurally instead: through the `setting` key that records the id, or
through a property such as "some goal names repeat" rather than the two names that do.

Invented fixtures use one vocabulary, so a new test copies it rather than inventing a second scheme:

| Kind | Codes | Names |
|---|---|---|
| Cash | `CHK`, `SAV`, `BKR`, `NST` | `Everyday`, `Rainy Day`, `Brokerage`, `Nest Egg` |
| Credit | `CC1`, `CC2`, `CC3`, and `CHK` again | `Card One`, `Card Two`, `Card Three`, `Everyday Card` |

`CHK` appears under both kinds deliberately: one code naming two accounts is exactly what
`UNIQUE (code, kind)` exists for, and a fixture set without that property would stop exercising it.
No name here is `Checking`, `Savings`, `Cash` or `Credit`, which are what `Group::label` and
`Kind::label` already print — an account sharing a band's label makes an Overview row ambiguous to
assert against.
Amounts are round invented numbers chosen to make an assertion readable, never lifted from a real
balance — and where several of them feed one asserted result, the result is recomputed from the
invented inputs rather than carried over.

## Architecture

Layered, and the layering is enforced by module privacy rather than convention:

| Path | Responsibility |
|---|---|
| `src/money.rs` | `Cents(i64)` — the only money type. No floats anywhere in the crate. |
| `src/rate.rs` | `Percent` (/100) and `BasisPoints` (/10,000) — the two scalings, as distinct types. |
| `src/gate.rs` | `Gate` — the Planning gates, each owning its setting key and goal-name substring. |
| `src/savings_block.rs` | `Block` — the two blocks of the `Savings` sheet, each owning the setting key naming its container account. |
| `src/config.rs` | The TOML config file. `serde` and `toml` are named here, and both again in `src/backup/state.rs`, whose `State` derives `Serialize` as well as `Deserialize`. |
| `src/plan_line.rs` | Every Planning line: its label, the amount it moves, and the setting key that says where it lands. |
| `src/calc/` | Pure formulas: `tax`, `biweekly`, `per_paycheck`, `pro_rata`, the Planning waterfall, `fund` (the target/actual/delta derivation), `schedule` (when a recurring thing happens). No database. |
| `src/db/` | Schema and queries — one module per aggregate. |
| `src/db/migration.rs` | The frozen v1 baseline, the chain of arms above it, and the runner that applies whichever of them a database is missing. |
| `src/db/date.rs` | The stored date format, in one place: `iso` writes it, `parse`/`parse_opt` read it back for a `from_row`. |
| `src/db/bill.rs` | The monthly bill block, labelled — the `Planning!C6:E12` rows. |
| `src/db/fund.rs` | The `fund` table — the asset-allocation block, `Planning!I1:M5`. `Target` is the age rule or a share of what it leaves. |
| `src/db/recurring_txn.rs` | The `recurring_txn` table — rows whose amount and date are known in advance. CRUD plus the queries regeneration needs. |
| `src/import/` | Reads `Money.xlsx` via `calamine`. |
| `src/plan.rs` | Reads settings and balances out of `db`, feeds `calc::planning::compute`. |
| `src/fund.rs` | Reads the `fund` table and the birth date out of `db`, feeds `calc::fund`. The one place `db::fund::Target` becomes `calc::fund::Rule`. |
| `src/transfer.rs` | The policy over `db::txn`: resolving lines to destinations, grouping, and writing a payday atomically. `wiring` and `diagnose` are the same rules read rather than enforced, for the screen that has to draw a database `plan` would refuse. |
| `src/recurring_txn.rs` | The policy over `db::recurring_txn`: horizons, adoption order, what a cadence *is*, and regeneration. |
| `src/projection.rs` | The dates every balance is quoted at: to-date, ad-hoc, month-end. |
| `src/backup/` | The schedule, the snapshot, and the upload. `aws_config`, `aws_sdk_s3` and `tokio` are named only in `s3.rs`. |
| `src/tui/` | The screens. `ratatui`/`crossterm` are named only here. An account reaches a screen through `tui::label::Account`, which colors it, everywhere but a short, named list of residuals in `src/tui/CLAUDE.md`'s account-color section. View-state types hold no ratatui; render functions only draw, and what every screen shares lives in `mod.rs` rather than in whichever screen needed it first. Which module is which screen, what a key may mean, and how wide a screen is laid out for are all in `src/tui/CLAUDE.md`. |
| `src/bin/mm.rs` | clap CLI. No subcommand launches the TUI; `import` and `backup` are the two subcommands. |

**`rusqlite` is named only inside `src/db/`.** `Db` holds a private `Connection` and deliberately does
not `Deref` to it — handing out a `&Connection` would put every rusqlite method back within reach of
`import` and `plan`. Everything outside `db` reaches the database through
`db::{account, bill, goal, recurring_goal, recurring_txn, setting, txn}`. Likewise `calamine` is named only inside `src/import/`,
which re-exports `CellData` and `SheetRange` for callers (including integration tests, which compile
as separate crates).

Every query module states the columns its `from_row` reads in a `select_*!` macro beside it, and
builds every `SELECT` of that row from it — `concat!` at compile time, so the queries stay
`&'static str`. A query wanting those columns in another shape — `select_goal!`'s `with_balance`
arm, table-qualified for a join and carrying the allocation sum — takes an arm of the same macro
rather than a list of its own, so what a reader checks the `row.get` indices against stays in one
place per table. A new query module follows the same shape; so does a new date column, which goes
through `db::date` rather than restating `%Y-%m-%d`.

**A `get` by id errors when the row is missing; it does not return `None`.** An id held by a caller
came off another row, so a dangling one is a corrupt database rather than an ordinary absence, and
every such reader would otherwise write the same `.context("… is gone")` back. `goal::get` is the
one exception and says so at its definition: `transfer::wiring` must *draw* a dangling setting key
instead of refusing, and the destination picker opens on a line's goal even once it is closed — both
tell "gone" from "corrupt" using context only they hold. A new one follows `account::get`.

Multi-statement writes go through `Db::transaction`, which commits only if its closure returns `Ok`.
It issues a bare `BEGIN` and is **not reentrant** — anything reachable from inside another
`transaction` must not call it. `db::clear_imported_data` is the standing example: it relies on its
one caller already being inside the import's transaction. `txn::write_transfer` is the second: it
writes both legs of a transfer with no transaction of its own, so several calls compose into one
atomic payday under a single caller-owned `transaction`. `txn::insert_transfer` is the wrapper that
opens that transaction for a single transfer.

**The schema migrates forward, and `schema.sql` is a frozen baseline rather than the whole truth.**
`schema.sql` states version 1 and is never edited again; every change since is an arm in
`db::migration::MIGRATIONS`. A fresh database takes the baseline and then the whole chain — the same
SQL, in the same order, that an existing database takes the tail of. **One code path is the point:**
there is no second statement of the schema for the chain to drift from, and since every test builds
its database through `db::open_in_memory`, the chain is replayed from version 1 on every
`cargo test`. An arm broken by a later arm fails the suite rather than waiting to fail on the one
database nobody has migrated yet.

Adding a schema change means appending an arm and nothing else. The head version is one plus the
number of arms, so there is no constant to bump and no way to forget to — which is what retires the
old hazard of a stale database failing on a missing column instead of saying so on open. A database
above the head is one a later build wrote, and `db::open` refuses it rather than opening it
best-effort. Zero is the one version that is not "some other schema": it is an empty file, and
filling it is the whole job.

Three rules bind an arm, all of them enforced by the chain being replayed from version 1:

- **An arm names the schema as it stood when the arm was written, never as it stands today.** An arm
  two versions before a table is renamed still calls it by its old name.
- **An arm must survive replaying against an empty database**, because that is what a fresh install
  is. A data half that finds nothing to move returns `Ok` rather than erroring.
- **A change of *meaning* is an arm too** — a data half with no SQL. A column whose type does not
  change but whose interpretation does leaves an existing database holding values under the old
  reading, and no `CHECK` can catch that. The old version 2 was exactly this case: `account` stopped
  being an imported table, so its name, band and order became the owner's.

What this costs is a `schema.sql` that stops describing the database as it actually is once the
chain is long. The remedy is to **squash**, and it is a periodic operation rather than a one-off:
with the owner's database at the head version, dump the schema it actually has into `schema.sql` as
the new version 1, empty `MIGRATIONS`, and delete-and-reimport once. That is affordable because the
workbook is the source of truth for every figure in the database; what it does not carry — the
recurring transactions, and the naming and ordering on the Accounts screen — is quick to re-enter.
Never squashing is what would turn "frozen" into a trap.

Anything the schema constrains has a Rust type that says the same thing, so the
`CHECK` is a backstop rather than the only guard: `account::Kind`, `account::Group`,
`recurring_goal::Cadence`, `goal::BatchKind`, `fund::Target`, and one id type per table
(`db::AccountId`, `db::GoalId`, …, `db::FundId`, in `src/db/id.rs`, which carry their own
`ToSql`/`FromSql`). When you add a table or a constrained column, add the type
too — and when you add an enum variant, check the schema's `CHECK` list still
matches, since nothing but a test ties them together.

## Invariants worth knowing before editing

- **Goal names are not unique.** "Lego" names several goals in the workbook, "Dropbox" more than
  one. Nothing downstream may key a goal by name. Name matching happens exactly once, at import,
  which records ids into `setting` under the keys `gate::Gate` owns; readers resolve by id. Add a
  gate by adding a `Gate` variant — never by writing its key or its substring at a call site, since
  the whole point of the type is that the two halves cannot be paired wrongly. `plan_line::Line`
  owns such keys of its own — one per line matched at import — built the same way and for the same
  reason.
- **An account's name, color, order, and Overview band are the owner's, not the workbook's.** The
  sheet carries a short code per account in its own column order and nothing else — no longer name,
  no band, no order, no color. So the import writes a row per code and stops: `name = code`, the
  kind's `default_group`, a `sort` appending to whatever that kind already holds, and no color at
  all. The Accounts screen is where all four are then set, and `account` is **not** in
  `IMPORTED_TABLES`, so what is set there outlives every `mm import --replace`. A code the sheet grows later is not an error: it
  arrives the same way, named after itself, and sorts past the accounts already placed.
  `account::reorder` takes a *position* rather than a raw `sort` and renumbers the whole kind,
  because `sort` is only ever read through an `ORDER BY` that breaks ties by code — "put it third"
  is an instruction whose result does not depend on rows the caller never saw, and "set sort to 2"
  is.
- **An account's color is a name, and having none is a supported state rather than a gap.**
  `account.color` holds an `account::AccountColor` as `TEXT` with the schema's `CHECK` behind it —
  the construction `kind`, `grp` and `interest_policy` already use — because an index into a
  palette would leave the database holding a number whose meaning lived in one array in `src/tui/`,
  and reordering that array would silently repaint every account. `NULL` is what every row starts
  at and what the Accounts screen's `—` writes back: `tui::style::account_color` falls back to
  `AccountColor::derived`, so a freshly imported database is already distinguishable and the field
  is an *override* rather than a step the owner has to complete first. The derivation lives on the
  enum because it is a fact about *it* — which variant an id lands on — while what a variant looks
  like is `tui::style::palette`'s to say and nothing else's, which is what keeps colour decided in
  one module.
  Which screens honour that is not left to each screen: an account reaches a glyph through
  `tui::label::Account`, which colors what it draws, everywhere but the residual list in
  `src/tui/CLAUDE.md`'s account-color section, and `AccountName`/`AccountCode` have no `Display`,
  so an account cannot be flattened into a `String` on the way.
- **Three things about the owner's accounts are in no cell of the workbook, and all three are
  configured on the Accounts screen.** The `Savings` sheet names its two blocks by *position* --
  `A:E` and `I:K` -- with no account code beside either, and nothing anywhere says which account is
  the current one. So:
  - `savings_block::Block` owns a `Key<AccountId>` per block, the same construction as
    `gate::Gate`: one value carries the key and what it means, because a block recorded under the
    other block's key sends a whole sheet's goals to the wrong container and every balance, gate and
    destination below follows it there.
  - **The current account is the one in the `Checking` band**, through `account::checking` — the one
    read, so that the waterfall's `Excess (Actual)` and `transfer::source` cannot come to mean
    different accounts. None is a database nobody has finished configuring; more than one is an
    ambiguity only the owner can settle. Both are errors saying what to do, and the Planning screen
    renders the message in place of the plan, exactly as it does for every other unresolvable plan.
- **`mm import` is self-resolving, not two commands.** With the block mapping unset it imports
  `Constants`, commits the accounts, and stops with `Report::AccountsOnly`; with it set it runs the
  whole import in one pass. Only the first import against an empty database is ever two steps: the
  mapping is read *before* `clear_imported_data` and written back after, so a `--replace` — which
  does clear `setting` — cannot reopen the two-step. A block key pointing at an account that is
  gone is a corrupt database and a loud error naming the key, never a silent return to "not
  configured", which would import a whole sheet into a container the owner never chose.
- **`account.grp` and `account.kind` say overlapping things, and `set_group` is what holds them
  together.** A `Group` subdivides exactly one `Kind`: cash splits into `Checking` and `Savings`,
  credit does not split, so `Group::Credit` *is* the kind. The schema constrains the two columns
  separately, so a cash row could claim the credit band and be subtotalled into the wrong half of
  Net; `account::set_group` refuses a group whose `kind()` disagrees, and `insert` only ever writes
  the kind's default. Those two are the only writers. (`grp`, not `group`: `GROUP` is a SQL
  keyword.)
- **Settings are the coupling between `import` and `plan`.** Stored as `TEXT` key/value, but never
  addressed by a bare string: each key is a `Key<T>` constant in `db::setting::key` (or, for the two
  gates, on `gate::Gate`), so `setting::get`/`set` infer the value type from the key. Add a setting by
  adding a constant — a literal at a call site defeats the point, because every reader has a fallback
  and a mistyped key reads as "not configured". An *unset* key means a feature is off; a key pointing
  at a row that no longer exists is a corrupt database and must be a loud error, never a silently
  disabled gate. Storing a new type means implementing `setting::Value` for it.
- **Bills are a table, not a setting.** `Planning!C6:E12` is `db::bill`, split into
  `Category::{Housing, Other}` because `calc::planning` reports `housing_biweekly` and
  `other_bills_biweekly` separately and only the first reaches `lines.current_housing`. `calc` still
  takes two bare `Vec<Cents>`; `plan::compute_from_db` fills them from `bill::amounts`. A half-filled
  row in the sheet is an import error, because a dropped bill inflates the excess the waterfall has
  left to allocate.
- **Funds are imported, so `--replace` overwrites hand-typed values.** `fund` is in `IMPORTED_TABLES`
  because the import writes it, which means a value typed on the Funds screen is replaced by the
  sheet's on the next `--replace`, exactly as every other imported figure is. `has_imported_data` is
  unchanged: transactions and goals still stand in for the whole set.
- **A fund's target percentage is derived, never stored.** The bond row's target is
  `(age − 30)` points and a birthday moves it with no write, so storing it would go stale in the
  night. What the table holds is the *rule* — `Target::AgeOver30`, or a share of what that rule
  leaves — and `calc::fund` turns it into a percentage on every read. The remainder those shares
  divide is `10,000 bp` minus every age row's target, clamped at zero; an age row with **no birth
  date on record** claims nothing, so the share rows divide the whole 100% rather than being told a
  zero that is really a question.
- **`lines.goals` in the waterfall is a plug**, clamped at zero. The clamp is what makes `checksum` a
  real check — unclamped, the term cancels algebraically and the checksum is zero for any input.
- **The plug's set and the plug's division are two separate questions, answered in two places.**
  `transfer::spread_goals` is the set: the goals no line claims that are *still short*. A goal
  sitting at its target needs nothing, so it is offered nothing — and, because the same set decides
  *where* the plug lands, it does not pull the spread into its container either. That second half is
  not an optimisation: a met goal alone in a second container used to make the plug ambiguous over
  money it could never have received, which is what a line switched to a withdrawal leaves behind.
  When *nothing* is short the set is all of them rather than none, so the plug still has a container;
  their asks are then all zero and the money ends up unallocated, which is the right answer when
  everything is funded.
  - **The set is not `unclaimed_goals`, and the two must not be collapsed.** A met goal is still a
    perfectly good destination for a line, so it stays suggestible: that is exactly what makes
    `Home Down Payment?` worth offering on a Future Housing row that no longer funds it.
  - The claim list reaching the set differs by caller — `plan` reads claims strictly and refuses a
    dangling key, `wiring` has to report one and draw the screen anyway — so `shares_of` takes an
    already-filtered set rather than reaching for the database itself.
- **A payday prefills what each goal asks, and leaves the rest unallocated on purpose.** The ask is
  `tui::paycheck_ask` — `calc::per_paycheck`, and the same figure the Savings screen shows in
  `$/Pay`. `calc::fit` puts the asks against the money there is: under-subscribed, every ask is met
  in full and the difference is **left over**, because that remainder is money the owner places by
  hand rather than money a prefill should find a home for; over-subscribed, `pro_rata` scales every
  goal to the same fraction of what it wanted. Weighting by the ask rather than by the raw shortfall
  is what makes a deadline count — a goal due next paycheck asks for all it lacks, one due in three
  years for a thirtieth of a larger number.
  - **Only the plug is priced this way.** A line that names a goal hands it the waterfall's own
    figure — Bills hands "Bill Payments" the whole of `lines.bills` — and a per-paycheck ask must never
    overwrite an amount computed for that goal specifically.
  - `tui::paycheck_ask` exists because several callers want that number: the `$/Pay` column, the `A`
    prefill, and this one. A figure a screen shows and a figure a prefill writes must be the same
    figure, and a copy of the unpacking in each is how they stop being.
- **An unset destination key and a dangling one mean opposite things, and must never be
  conflated.** `plan_line::Line::destination` and `gate::Gate::key` both resolve through `setting`,
  and for a destination key "unset" is a real, supported state: the money leaves the tracked system
  as a withdrawal, same as an owner who has not yet opened a Retirement account. A key pointing at a
  goal or account that no longer exists is a corrupt database instead, and `transfer::plan` reports
  it as a loud error naming the key — never as a silently reinterpreted withdrawal, which would move
  real money to the wrong place on the strength of a stale row. The Planning screen has to *show*
  both, so `transfer::wiring` is the same rules read rather than enforced: it reports a dangling key
  as `Landing::Dangling` instead of refusing, because a screen cannot decline to draw itself the way
  `plan` declines to move money. What the two states mean does not change — the block renders the
  unset one as the withdrawal it is, in no color at all, and the dangling one in the red it shares
  with a plan that cannot run.
- **A suggestion beside an unset destination is advisory, and is the one place a goal name is read
  after import.** `transfer::suggest` offers the goal a line's `Line::import_substring` names, and
  only when *exactly one* unclaimed open goal matches: "Lego" names several goals, and offering the
  first would be the pick-by-luck that confines name matching to import in the first place. Nothing
  resolves through it. What a human accepts is written as an **id**, which is what every later read
  uses, so the rule stands — names are matched once at import, and once more only to ask a question
  the owner answers.
- **`Line::FutureHousing`'s key is the down-payment/mortgage switch, and its stored string still says
  `down_payment` on purpose.** Set, the line's money lands in the bucket-block goal it names — a down
  payment. Unset, it leaves as a withdrawal — a mortgage payment made outside the tracked system. It
  is the key an existing database already holds that goal's id under, so renaming the stored string
  would silently disconnect every such database from the goal it names.
- **`Line::label` and a line's goal-name substring are different strings on purpose.** The label is
  what the screens show; the substring is what the sheet's goal name must contain at import. `Bills`
  is the screen label for the line that funds "Bill Payments" — matching substring to label would
  make the substring unreadable as a label and the label too broad to match safely.
- **Import is not additive.** `import_all` refuses to run against a database already holding
  transactions or goals; `--replace` clears the imported tables first. The whole import runs in one
  SQL transaction.
- **`--replace` clears every table but two, and the two it keeps are the two the import does not
  write.** `IMPORTED_TABLES` is the clear list and `PRESERVED_TABLES` names the exceptions;
  between them they must cover every table the schema creates — baseline and chain alike — which
  is what `every_table_the_schema_creates_is_either_cleared_or_deliberately_kept` holds up. `account` is
  kept because its naming, banding and ordering are the owner's. `recurring_txn` is kept because
  the only reason it was ever cleared was that its `NOT NULL REFERENCES account(id)` pointed at
  rows the replace rebuilt — with those rows surviving, so do the rules, and the `txn` rows they
  own come back out of the workbook for `g` to adopt. Losing the paycheck flag would revert
  Paycheck-Eve to today and move the Planning waterfall's excess, so keeping it is the point
  rather than a side effect.
- **Sign conventions differ per ledger.** Cash rows are signed naturally (positive is inflow); credit
  rows are signed as debt (positive is a charge). Balances are always `SUM(cents) WHERE date <= X`,
  with future-dated rows pre-entered — that is what makes projection and to-date the same query.
- **User-editable settings reach divisors.** `div_ceil` rejects a non-positive divisor as an error
  rather than a `debug_assert!`; callers clamp with `.max(1)` where a nonsense setting should not take
  down a whole screen. Don't reintroduce a bare divide.
- **A goal ends; it never becomes its successor.** `goal::move_value` is both endings — abandon
  (`to: None`, value back to unallocated) and close out (`to: Some(id)`, value to another goal in
  the **same container**) — and it closes the goal itself, in one transaction. The next round of a
  recurring goal is created from the `recurring_goal` table. Crossing containers is refused: no cash moved between
  the accounts, so allowing it would break both reconciliations at once.
- **`goal.favorite` is the owner's, and it is the one owner-set field the import can take away.**
  `f` on Savings toggles it and `goal::set_favorite` is the column's one writer — not a field on
  `GoalEdit`, for the reason `recurring_txn::set_paycheck` is not one on `update`: the goal form
  has no field for it, so an edit that wrote the whole row would clear a mark the owner never
  touched. The comparison worth drawing is `account.color`, which is also the owner's and also
  absent from the sheet — but `account` is in `PRESERVED_TABLES` and `goal` is not, so a
  `--replace` keeps a color and loses every favorite along with the goals themselves. Nothing can
  fix that: goal names are not unique, so there is no key to re-attach a mark by. What makes it
  affordable is that a favorite is a highlight and nothing else — it moves no money, gates
  nothing, and changes no figure on any screen, so losing one costs a keystroke rather than a
  reconciliation.
- **One worksheet commit is one `batch`.** `goal::insert_allocations` opens the batch itself, so a
  fumbled payday is one `delete_batch` rather than dozens of deletions. `U` undoes the most recent
  batch by insert order and **never an `Import` batch** — that one holds every opening balance in the
  database. A worksheet is scoped to one container: payday runs it twice.
- **The interest prefill is a property of the account, not a setting.** `account.interest_policy` is
  `pro_rata` or `manual` (which is also what `NULL` reads as), because a per-container setting key
  cannot be a `Key<T>` constant. It is set on the Accounts screen and no import ever writes it: which
  of the two a container wants is a judgment about that account, not a fact the sheet
  carries. `manual` prefills from the container's previous `Interest` batch, rescaled
  through `pro_rata`, and falls back to `pro_rata` when there is none. **The policy decides how the
  total is divided, never who it is divided among**: `goal.interest_eligible` is the membership, and
  `tui::worksheet::interest_prefill` filters the previous batch to it before that batch can weight
  anything. Without that filter the flag would be inert on exactly the container whose prefill is a
  copy — a goal made ineligible would keep drawing last month's share.
- **Regeneration is release, skip, adopt, insert — in that order.** `recurring_txn::regenerate` deletes the
  rows it owns from today forward that are still its own work (`recurring_txn_id = ? AND edited = 0 AND
  date >= ?`); whatever it still owns after that is a hand-correction, one per occurrence. Then, and
  only then, is the occurrence list known, so:
  - a date it owns inside `[today, horizon]` that is **not** an occurrence is *released*
    (`recurring_txn_id = NULL, edited = 0`, `recurring_txn::release_dates`) — never deleted, the
    same as `recurring_txn::delete`. Without this, moving a correction (edit the anchor or cadence, or move
    the row itself in the ledger) leaves it owned at a date the schedule no longer produces while
    the schedule's own date gets a fresh insert: four rows for three occurrences, every projected
    balance one payment out, stable from run two so nothing else catches it. Bound it to
    `[today, horizon]`, or shrinking the horizon sweeps up rows beyond it as a side effect.
  - an occurrence it **already owns is skipped**, not offered to `adopt` — whose
    `recurring_txn_id IS NULL` guard would refuse it anyway, and the refusal would be read as "nothing to
    adopt" and end in a duplicate insert. This step is the one the plan itself got wrong.
  - otherwise `adopt` claims one matching unclaimed row before `insert` writes a new one. Adoption
    is what makes `g` idempotent from the first press against an imported database, which already
    holds every future occurrence. An adopted row whose amount differs is flagged `edited`, so the
    next run never takes it back.

  Regenerating twice must produce identical rows — that is the property the tests pin. **What this
  does not do:** a moved correction plus the regenerated occurrence is still four rows where the
  schedule says three. Ownership is coherent and the released row is an ordinary ledger row the
  owner can keep or delete, but nothing decides that a moved row *consumes* its original
  occurrence — that needs the transaction↔occurrence link recorded (a `txn.occurrence_date` column),
  which is not there.
- **A recurring transaction's horizon is `min(recurring_txn.horizon,
  max(today + RECURRING_TXN_HORIZON_MONTHS, recurring_txn.generate_through))`**, the setting clamped
  `1..=120` months — the upper end matters too, since every occurrence inside the horizon is a row
  written in one transaction. Rows pre-entered beyond it stay unclaimed and untouched, because
  regeneration only ever touches rows it owns.
- **The two dates on a recurring transaction pull opposite ways.** `horizon` is a *cap* — the row
  stops existing there. `generate_through` is a *floor*, what `x` writes: how far past the rolling
  window this one has been asked to reach, clamped by the same ten years, and spent once the window
  overtakes it. Neither can stand in for the other, which is why they are two columns.
  `recurring_txn::update` writes neither the extension nor `is_paycheck`, for the same reason: the
  form that calls it has a field for neither, so `set_generate_through` and `set_paycheck` are each
  their column's one writer. `recurring_txn::extend` refuses rather than reporting an empty
  regeneration when the cap or the ten-year ceiling already binds — its `Extended` says which — and
  the date it reports on success is read back out of `horizon`, so what the status line quotes is
  where the rows stop rather than where the floor landed.
- **Deleting a recurring transaction releases its rows** (`recurring_txn_id = NULL`, `edited = 0`)
  rather than cascading. A
  delete must never move a balance.
- **At most one recurring transaction is `is_paycheck`**, enforced by
  `recurring_txn::set_paycheck` clearing every other one in the same transaction — by the write,
  not by the reader. `recurring_txn::update` deliberately does not
  touch the flag.
- **The ad-hoc projection date is derived**, by `projection::dates` through `recurring_txn::next_paycheck`,
  as the day before the next paycheck; it falls back to today when nothing carries the flag. The
  Overview scrub is pure view state and persists nothing. The one `Cadence → Step` match lives in
  `src/recurring_txn.rs`: `calc` never learns that `biweekly` is a string in a column.
- **The scrub reaches Planning, and `plan::compute_from_db` takes the ad-hoc date as an argument
  rather than re-deriving it.** `Excess (Actual)` *is* the checking balance at that date, so a
  waterfall that re-derived it would quote a different day than the Overview column the owner just
  moved. `App::adhoc` is the one scrubbed value, and all three readers take it: the Overview, the
  Planning screen, and `t`, which recomputes the plan to build its confirmation. `p` therefore pins
  the scrubbed figure — the number on the row it was pressed on — while `PINNED_AT` goes on storing
  today, since it records when the owner pinned and not which day the balance came from. Planning
  marks a scrubbed plan by naming the date in the `Excess (Actual)` extra column, the way the
  Overview marks its column header: this screen has no header to hang it off, and a screen quoting a
  hypothetical balance must say so.
- **An absent config file means backups are off; an unparseable one is an error.** The first is the
  same rule an unset `setting` key follows, and is what makes a clean checkout and an unconfigured
  machine both do nothing. The second is why `deny_unknown_fields` is on: a misspelled key that left
  `bucket` unset would read as "off", and a backup that silently stops running is the one failure
  nothing downstream ever notices.
- **`interval_days` is clamped before it reaches `TimeDelta::days`, the same rule as every
  user-editable setting that reaches a divisor.** `backup::interval` caps it at `MAX_INTERVAL_DAYS`
  (3653, ten years) because the value is read straight out of a hand-edited config file and
  `DateTime`'s addition panics rather than erroring once the sum leaves chrono's calendar — a nonsense
  setting must not take the run down, the same reasoning behind `div_ceil`'s `.max(1)` callers and the
  recurring transaction horizon's `1..=120`-month clamp.
- **The backup state file is advisory where a `setting` key is binding.** An unreadable
  `~/.local/state/mistermanager/backup.toml` warns and is treated as "never backed up", rather than
  refusing the way a dangling `setting` key does. The asymmetry is in the consequence: a dangling
  setting key moves real money to the wrong place, while a corrupt state file costs one redundant
  upload and is correct again as soon as it is rewritten.
- **The backup identity may only `PutObject`.** The key in the `mistermanager` profile is
  long-lived and unattended, so the policy is what bounds it: it cannot read a backup, delete one,
  or list the prefix, and restores are done by the owner under their own identity.
- **The bucket is this repository's own, and its name is composed rather than configured.**
  `mistermanager-<account id>-<region>-an`, built in `mistermanager.tf` from
  `aws_caller_identity` and `var.aws_region`. A name that had to be *chosen* would say where the
  owner's finances are backed up — the same kind of fact as the workbook path — and would have to
  reach Terraform out of band to stay unsaid; one derived from the profile is safe to commit and
  needs no such step. Owning the bucket is what makes the
  lifecycle rule declarable at all: `aws_s3_bucket_lifecycle_configuration` is a whole-bucket
  resource, so two repositories declaring one would revert each other on every apply. The rule
  expires an object at 365 days.
- **The scheduled check is the default database's, and `--db` opts out of it.** It runs after every
  arm but `backup` itself, and the state file it stamps records *when* an upload last happened
  rather than *what* was uploaded — so a scratch database backed up on the schedule would take the
  real one's turn as well as leaving an object nothing distinguishes from a real backup. An
  explicit `mm backup` is exempt, because being pointed somewhere is what it was asked for.
- **The due check reads the real clock, not `--today`.** `--today` simulates a financial date, and
  whether a file reached S3 is a fact about wall time — `mm --today 2027-01-01` must not fire an
  upload. `run_if_due` therefore takes `now` as `Utc::now()` from the CLI rather than the `today`
  the rest of the application is driven by.
- **The key prefix is a constant, not a setting, because one end of it is an IAM policy.**
  `mistermanager` appears in `backup::PREFIX` and in `mistermanager.tf`'s policy resource path, and
  nothing ties them together — but the policy is what the IAM user is scoped to, and only an AWS
  apply can change it, so a config knob could only ever be turned into `AccessDenied`. Moving the
  prefix means editing both, in that order. `Backup` carries no `prefix` field and
  `deny_unknown_fields` refuses one, so a config file asking for another prefix fails loudly
  instead of being silently ignored.

## Testing conventions

Test names are full sentences describing the scenario
(`shortfall_of_an_overfunded_goal_is_zero_not_negative`, not `test_shortfall_3`). Unit tests live in
`mod tests` at the bottom of the file under test and run against in-memory SQLite
(`db::open_in_memory`); tests that need the real workbook live in `tests/`.
