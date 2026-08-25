# import — the workbook's shape

`calamine` is named only in here. This module is the one place that knows where anything lives in
`Money.xlsx`; everything downstream sees `db` rows and `setting` keys.

That containment is what puts the whole module behind the non-default `import` Cargo feature: one
module naming the dependency is one `cfg` to put it behind, so `calamine` is `optional` and a
default build carries neither the parser nor the `mm import` subcommand. Anything added here is
reachable only under `--features import` — including its `mod tests`, and every binary in
`tests/`, which carry `#![cfg(feature = "import")]` for the same reason.

The whole import runs inside one SQL transaction opened by `import_all`, in dependency order:
`Constants` (accounts and settings) → `Planning` (settings, the bill table and the fund table) → the ledgers →
`Savings`. A failure partway through — an unknown account code, half a bill row — leaves the
database exactly as it was, not half-populated. Nothing here may call `Db::transaction` again; it
is not reentrant.

**`import_all` is self-resolving, and that is why it stops early sometimes.** `Savings` names its
two blocks by *position* and carries no account code beside either, so which account each block
belongs to is the owner's to say on the Accounts screen and cannot be read here. `savings::containers`
is that reading: unset it returns `None`, and `import_all` writes the accounts, returns
`Report::AccountsOnly`, and stops with a message naming the screen. Set and resolving, the whole
import runs in one pass. Set and *dangling* is a corrupt database and a loud error naming the key —
never a silent return to "not configured", which would import a whole sheet into a container the
owner never chose.

Only the first import against an empty database is ever two steps. The mapping is read **before**
`clear_imported_data` and written back by `savings::set_containers` after, because `setting` is an
imported table and `account` is not: the rows the keys name outlive a `--replace`, so the mapping
is still true and losing it would make every replace a two-step.

Import is **not additive**. `import_all` refuses to run against a database already holding
transactions or goals, because re-running would double every row with no signal beyond a healthy
exit code. `--replace` clears the imported tables first — but not `account` and not
`recurring_txn`, which are the owner's rather than the sheet's. See the root `CLAUDE.md`.

A `Constants`-only pass writes neither a transaction nor a goal, so `has_imported_data` stays false
and the second pass needs no flag.

## Sheet map

### `Constants` → accounts and settings

```text
  A             C               E               G                   H                  J      K
1 Cash Accounts Credit Accounts Sales Tax Rate  Annual Pay Periods  Pay Period Length  Today  Birth Date
2 CHK           CC1             0.0625          26                 14                 …      …
3 SAV           CC2
4 BKR           CHK
5               CC3
```

A code can appear in both columns — a checking account and a card at the same bank. That is why
accounts are keyed by `(code, kind)` and never by code alone; `account::by_code` takes both, and
folds the code's case — a code the owner typed on the Accounts screen has to meet the one the sheet
spells, or this pass writes a second row for an account that is already here.

The codes are all the sheet carries, so that is all the import writes: each new code becomes an
account **named by itself**, in its kind's `default_group`, with a `sort` appending to whatever
that kind already holds, and no color at all. The longer name, the color, the band and the order are
set on the Accounts screen and survive a `--replace` — `account` is not an imported table. A code already present is skipped, so
re-running the pass is a no-op rather than an `account_code_kind` failure.

`E2` is a fraction in the sheet and becomes `BasisPoints` (×10,000). The Planning split percentages
are also fractions in the sheet but become `Percent` (×100). Two scalings, two types, so a value
cannot be read at one scale and used at the other.

**`H2` is read by nothing.** The pay cadence is one fact and the sheet states it twice — a count in
`G2` and a length in `H2` — so only the count is imported, and `calc::period_days` derives the
length from it. Two cells can disagree, and a database holding `26` beside `15` would count every
goal's runway in a cadence the owner never worked: `26 / 14` is the pairing, and nothing in the
sheet enforces it. `tests/import_constants.rs` asserts the derivation against `H2` all the same,
which is what would catch a workbook where the two have come apart.

### `Cash` and `Credit` → `txn`

`A=Date, B=Amount, C=Acct, D=Description`, header in row 1. One shape, two sign conventions: cash
rows are signed naturally, credit rows are signed as debt. Nothing is normalized on the way in — the
sign convention is per-ledger all the way through the app.

Opening balances arrive as ordinary rows: `End of Year` entries dated 12/31 of the prior year.
Future-dated rows (pre-entered bills, paychecks, scheduled card payments) arrive as ordinary rows
too, which is exactly what makes projection and to-date the same `SUM(cents) WHERE date <= X` query.

A row whose date won't parse is only reported as skipped when it carries an amount, an account code,
or a description. Trailing blank rows inside the used range are normal and silent; a real row with a
bad date must never be dropped quietly.

### `Savings` → goals, buckets, and the recurring goals

Independent blocks in one sheet, scanned from row 6:

| Columns | Contents | `savings_block::Block` |
|---|---|---|
| `A:E` | Goals — `A` name, `B` current, `C` goal, `E` goal date | `Goals` |
| `I:K` | Buckets — `I` name, `J` current, `K` goal | `Buckets` |
| `O:Q` | The recurring goals — `O` name, `P` date, `Q` amount | — |

**Which account each block belongs to is not in the sheet.** The blocks are told apart by position
alone, so `Block` carries a `Key<AccountId>` per block and the Accounts screen is where they are
set. `savings::import` takes the pair as an argument rather than reading it, so the read happens
once, before a `--replace` can clear it.

**The two blocks are scanned by different rules, on purpose.** The goal block has a gap in it, so
its loop runs the full sheet height and skips rows missing a name, a current, or a goal. The bucket
block is contiguous, so `bucket_rows` stops at the first blank name in column I. Running the goal
rule over column I would import any stray text further down that column as a phantom bucket carrying
a real allocation, silently corrupting the container reconciliation. Stopping at the first blank
still lets the block grow another bucket. There is a unit test against a hand-built range for this,
because the live workbook doesn't exhibit it.

That contiguity cuts the other way too: **half a bucket row is a hard error, not a skip.** The
scan has already ended at the first blank name, so a *named* row with a blank `J` or `K` is a bucket
with a missing figure rather than a heading or a footer. Dropping it would leave its balance out of
the container's allocations and make the unallocated remainder wrong by that much — the way half a
bill and half a fund are errors. The goal block cannot be strict for the same reason it is scanned
differently: text in `A` with nothing beside it is a heading or a stray note there, and nothing
distinguishes that from a goal with a blank cell.

Each imported balance becomes **one opening allocation** in a single `Import` batch — `Current` is
derived from the allocation ledger, never stored. That batch is the one `U` will never undo; it
holds every opening balance in the database.

Goal-block goals get `interest_eligible = true`. A bucket is ineligible when its name contains
`"Down Payment"`, reproducing `Planning!J7`'s forced-zero weight. That is the *opening* value, not
a permanent one: the goal form's `Interest` field edits it, and a `--replace` reruns the rule over
a rebuilt `goal` table, so a hand-set flag is lost along with every other edit a replace discards. That substring is deliberately a
separate constant from `Line::FutureHousing`'s, even though the text matches today: one answers
"does interest allocate here", the other "which bucket does the future housing line watch".
Collapsing them couples the day either needs to change.

**Nothing here writes `account.interest_policy`.** How a container divides an interest posting is a
judgment about that account rather than a fact the sheet carries, so it is typed on the Accounts
screen and left alone by every import after the row's first insert. `NULL` reads as `manual`.

The recurring goals' cadence comes from header rows in column O, not from a per-row column. **The
workbook's "Biannual" means every two years**, so it maps to `Cadence::Biennial` — entries in that
group carry recurring-goal months two years ahead of their goal dates.

### `Planning` → settings and `bill`

| Cell | Becomes |
|---|---|
| `D1` | `key::PLANNING_TARGET` |
| `D3` | `key::PINNED_EXCESS` — the sheet's hand-typed `Excess (Fixed)`; refused by `plan::check_pinned_excess` if it is negative or carries cents |
| `J11` | `key::PLANNING_BUFFER` |
| `E19` / `F19` | `key::BILL_PAYMENT_CAP` / `key::BILL_PAYMENT_PCT` |
| `E20` | `key::MOM_AND_DAD_ANNUAL` |
| `E24` | `key::GOALS_FLOOR` |
| `F25:F27` | the split percentages — refused by `plan::check_splits` if any is outside `0..=100` or the three total over 100% |
| `C7:D12` | the `bill` table — `C7:C8` Housing, `C9:C12` Other |
| `I2:M<n>` | the `fund` table — `I` name, `J` the cached target percentage, `M` the value |

`C6` is the housing *subtotal*, not a bill. It is recomputed by `calc::planning`, not read, or the
housing figure would be counted twice.

**Half a bill row is a hard error, not a skip.** A label with no amount or an amount with no label
means a bill has been dropped, which inflates the excess the waterfall has left to allocate and
skews every downstream transfer instruction. Blank in both columns is just the end of the block.
Labels are indented in the sheet (`"  Mortgage"`); `cell::as_text` trims.

**The fund block's kind comes from the value, not the formula.** `calamine` hands back cached
values, and parsing `(DATEDIF(Dates[Birth Date],Dates[Today],"y")-30)/100` would be brittle. So the
first row is the age row when its cached `J` matches `(age − 30)/100` to within a basis point, and
every other row is a share of what that row leaves — `J / (1 − age target)`, rounded to the nearest
basis point, which recovers `0.36/0.90` as exactly 40%. The age used is the one the
*sheet* computed with: the birth date against `Constants!J2`, not against the day the import runs,
because `J` is what Excel cached on that date. `K` and `L` are never read — they are derived, and
recomputing them is what the screen is for.

**The block is bounded by `I` *and* `J` together, not `M`.** The real sheet follows its last fund
with a totals row (`M5`) that leaves `I5` and `J5` both blank beside a real `M`. Pairing the
end-of-block check with `M` the way a bill row pairs label and amount would misread that row as a
fund with a dropped name and refuse the whole import — but bounding on the name column alone gives
up too much: a genuine fund that lost its name would still carry its `J` formula, and reading a
blank name alone as the end of the block would let that row vanish silently and shrink the
portfolio, exactly the failure half-a-row errors exist to catch. So blank `I` *and* blank `J`
together is the sheet's own total, and ends the scan; blank `I` with `J` still present is a fund
with a dropped label, and is a hard error naming the row. A *named* row with no value is still a
hard error too, for the bill's reason in miniature: a dropped fund shrinks the portfolio and moves
every percentage beside it. A share row with a zero remainder is an error too — its share is
undefined. A missing block is silently zero rows.

The margin below the block is one blank row. `I6`/`J6` carry an interest-posting label and amount
(`Planning!I6:K9`), and `J11` is `key::PLANNING_BUFFER` — the same two columns the scan reads. Row
5's blank `I` and `J` are what end the scan before it reaches them; if another fund ever landed
where the totals row sits, the scan would keep going into `I6`, and the failure would surface as the
loud, transactional `"half a fund: … has no value"` rather than a silent misread — the right
failure, but the margin that produces it is exactly one row.

## Goal matching happens here, once

Goal names are not unique — several names repeat in the live goal block, one of them three times —
so nothing downstream may key a goal by name. Name matching therefore happens exactly once,
at import: `GoalMatch` offers each freshly inserted goal to a `(Key<GoalId>, substring)` pair — a
gate's, via `GoalMatch::gate`, or a `Line`'s own, via `Line::owned_goal` — and the winner's **id**
goes into `setting` under that key. Readers resolve by id.

Each key is offered goals from one block only. From the goal block: Roth (a gate), Bill Payments,
and Housing (`Line::Bills`, `Line::CurrentHousing`). From the bucket block: Emergency Fund (a gate),
Down Payment, and Mom & Dad (`Line::FutureHousing`, `Line::MomAndDad`). A future goal-block goal
named "Emergency Dental" cannot hijack the bucket side, because it is never offered to it.

Matching is by substring, not equality, because the sheet's names are "Roth IRA", "Emergency
Savings", "Home Down Payment", "Bill Payments", "Housing", "Mom & Dad". A **second** match is a hard
error rather than resolved by a rule like "first wins": choosing wrongly sends a whole Planning
line's money to the wrong destination, silently, which is the exact failure this indirection exists
to remove. **No** match writes nothing, which reads downstream as "not configured" — a gate off, or
a `Line` paying out as a plain withdrawal.

## Cell coercion

`cell.rs` is the only place Excel's types are interpreted, and `cell::at` is the one way a cell is
reached: it reads everything past the sheet's used range as `Data::Empty`, so a block scan can run
to `range.height()` without asking whether each column reaches that far.

Excel stores money as `f64`; every real amount in this workbook is exact to the cent, so rounding
at 2dp is lossless — that rounding is the last float in the pipeline and everything after it is
`Cents`.

`as_percent` and `as_i64` accept `Int` as well as `Float`, because a cell hand-edited to a whole
number round-trips as an integer. Reading only `Float` would return `None`, which every caller with
a default reads as "not configured" — a silent revert to a default is worse than a loud parse
error.

`as_date` uses `to_ymd_hms_milli` rather than `as_datetime`, which would require calamine's
`chrono` feature that this crate does not enable.

## Parity is checked by tests, not a command

There is no `reconcile` command — the owner checks figures by eye. The integration tests in
`tests/` are what verify the import: they assert every imported balance, the goal rows, both
container reconciliations, and the whole waterfall against the workbook's **own cached cell
values**, never hardcoded literals. They need `MM_ACCOUNTS` as well as the workbook — the three
accounts no cell names, by code — and `tests/common/mod.rs` is what reads it, configures a database
the way the Accounts screen would, and runs the two-pass first import. Run them with
`MM_REQUIRE_WORKBOOK=1 ... --features import` when changing anything in here: a missing fixture
turns the whole check into a silent skip, and so does a missing feature, since every binary in
`tests/` is `#![cfg(feature = "import")]` and compiles to nothing without it -- leaving
`MM_REQUIRE_WORKBOOK=1` no test left to fail.
