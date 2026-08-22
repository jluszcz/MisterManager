# MisterManager

A terminal application for tracking money, replacing a per-year spreadsheet.

## Usage

```bash
mm            # launch the application
mm --demo     # the same, with every dollar figure blocked out
mm report     # write the HTML report without opening the application
```

Screens are `1` Overview, `2` Cash, `3` Credit, `4` Savings, `5` Planning,
`6` Funds, `7` Recurring Goals, `8` Recurring Txns, `9` Accounts; `q` quits.
Accounts read by the name you gave them — `Everyday`, `Rainy Day` — everywhere
but Recurring Txns, whose columns leave room only for the code, and each one in
the color you gave it on screen `9`.
Tabs `7` and `8` are abbreviated because the bar is a row of shortcuts; each
screen's own title spells its name out. The screens are laid out for a terminal
at least 120 columns wide. Overview stacks the accounts in bands — checking, then
savings, then the cards — with a subtotal under each and a total under each
kind; its `←`/`→` scrub the Paycheck-Eve date against the baseline derived
from the paycheck transaction — `Shift+←`/`Shift+→` move it a week, as they do on
every date in the app — and
Planning quotes its excess at whatever that scrub leaves it at. Cash and Credit share one month: `[` and `]`
step both so the two always compare the same weeks, and `Esc` returns them to
the month around today. Each title ends with the balance of whatever `Tab`
narrows the screen to — `Cash · Aug 2026 · All · Today $42,000.00` — which is
the same to-date figure the Overview quotes, so neither the month on show nor a
`/` search moves it. A search matches a row's description or its amount, typed
without the separators the column draws — `1234` finds `$1,234.56` — and
`Enter` keeps it while `Esc` gives the whole list back. Narrowed to one account, `r` takes the balance a statement
says that account holds, and the title carries it with the difference after it —
`… · Today $1,160.00 · Target $1,200.00 · Δ -$40.00`, green above the target,
red below it, a dash when they match — so a typo or a missed row shows up while
the rows are still being entered. Nothing is written: quitting forgets every
target. Savings lists every open
goal with the container it belongs to:
`Tab` filters by container, `[` and `]` filter by goal date, `/` searches names,
balances and targets, `a`
allocates against the selected goal, `e` edits it, and `c` ends it — returning
its value to unallocated, or moving it to another goal in the same container.
Goals with no date lead the list, in an order you set with `K` and `J`; goals
with one follow, soonest first, since a deadline decides a goal's place for it.
`f` marks a goal, drawing its row as a band so it stands out among the rest;
that is all it does, so a marked goal keeps its place under every filter and
sort. The mark is stored on the goal, so unlike an account's color it does not
survive a `--replace`.
An allocation's amount takes `/N` for a fraction of the container's
unallocated remainder — `/2` is half of it, `/12` a twelfth — and the form
names the remainder it would divide and shows what the fraction comes to
before it is committed.
The month filter opens showing every goal; the first `[` or `]` narrows it to
the current month, and from there they step across the months the goal dates
span, the last wrapping back to the first. A goal with no date belongs to no
month, so it shows only unfiltered. `Esc` shows every goal again, clearing the
container filter along with the month — the screen narrows two ways, and one
key out of either saves working out which of them is hiding the goal you are
looking for.

`A` opens the allocation worksheet for the container — one amount, a line per
goal, and a live remaining counter; `Space` ticks the goals a posting funds and
`z` clears every other visible line to free the pot for them, `/N` divides the
amount onto the targeted lines, `s` spreads what is left across them by largest
remainder, `w` spreads it in the proportions they were prefilled with, and
`Enter` commits the whole thing as one batch. `i` posts interest from the container's
unallocated remainder, `n` opens a blank goal — a name, a target, a date, in
the container `Tab` names — and `U` undoes the last batch. (`n`, not `a`,
which this screen spends on the allocation it is mostly used for.)

`5` Planning leads with the transfer instructions — one per destination
account — and shows the waterfall that worked them out underneath: the
excess, the monthly bill block with its biweekly column, the gates, the
split, and where each line lands. `↑`/`↓` move between the editable constants
and skip everything computed; `e` edits the selected one, `E` opens a bill's
whole row — label, amount, and category — `a` adds a bill, `d` deletes the
selected bill, and `p` pins the excess so the plan stops moving underneath a
payday — pressing it again re-pins at whatever the excess reads now, and `P`
unpins. The excess is the checking balance at
Paycheck-Eve, so the Overview's `←`/`→` move the whole waterfall with it; a
scrubbed plan names its date beside `Excess (Actual)`, and `t` and `p` act on
the figures shown rather than on the derived date.

`6` Funds is the target/actual split across the funds — `Target %`,
`Actual %`, `Delta`, and the whole-dollar `Actual Value`, with a `Total` row
under them. One fund's target isn't a stored figure; it tracks the owner's
age directly — a percentage point past thirty for every year — and the rest
split whatever share of the target that leaves. Whichever row sits
furthest below its own target draws in bold: that's where the next
contribution belongs, and nothing is marked once every fund is at or above
its target. `a` adds a fund, `e` edits just the figure on the selected row —
the value it holds now — `E` opens the whole row, and `d` deletes it after a
confirmation; nothing here moves money, so a delete moves no balance either.
Entering the screen with an age-tracked fund and no birth date on record
opens a one-field form asking for it; `Esc` leaves that fund's target blank
rather than guessing, and the screen asks again the next time it has no
answer.

`8` Recurring Transactions holds the rows whose amount and date are known in
advance — the paycheck and the monthlies. `a` adds one, `e` edits, and
`d` deletes it and *releases* its ledger rows rather than deleting them. `g`
regenerates the selected one and `G` every one, and `P` marks the transaction
the Paycheck-Eve column is derived from. Regeneration adopts a matching
unclaimed row before inserting, so the first `g` after an import claims what
the workbook already held instead of duplicating it, and a row it owns on a
date the schedule no longer produces — the mortgage moved by hand from the
1st to the 5th — is
*released* back to the ledger rather than deleted. It reports
`removed / released / adopted / inserted` so all of that is visible.

`7` Recurring Goals is the table each round of goals is created from: `a`,
`e`, and `d`, with `d` refused while any goal still references the entry, and
`s`, which opens the picker — `Space` toggles an entry, `Enter` creates every
ticked one as a goal in the container the Savings screen's `Tab` names, all in
one transaction. It opens showing every entry; the first `[` or `]` narrows it
to the current month, and from there they step through the calendar — entries
carry a month of the year and no date, so December wraps to January. `Esc`
shows every entry again. Most months hold nothing, so an empty table there is
the answer rather than a fault.

The month filter is also what `s` ticks: whatever the screen is showing opens
already selected and sorted to the top, so the entries about to be created are
the ones the list opens on — a tick alone is easy to miss in a list dozens
long. Every entry is still listed below them, so the filter is a starting
point rather than a cage, and an entry that already has an open goal is left
unticked and sinks with the rest — `Space` still adds it, since a second open
goal against one entry is legitimate.

Every box that takes text — a form field, a `/` search, the worksheet's date —
edits the same way, with the readline keys: `Ctrl`+`A` and `Ctrl`+`E` jump to
the ends of the line, `Ctrl`+`B` and `Ctrl`+`F` move a character, `Ctrl`+`W`
deletes the word before the caret, `Ctrl`+`U` and `Ctrl`+`K` delete back to the
start or on to the end, and `Ctrl`+`D` takes the character the caret is on, as
`Delete` does. `←`/`→` move the caret too, in a field that holds text rather
than a date or a choice. `Ctrl` means editing text and nothing else anywhere in
the app; `Alt` is unused, since macOS sends `Option` as `Meta` only where the
terminal has been told to.

Every date the app asks for is typed as `YYYY-MM-DD` or as `M/D`, which takes
the next year that month comes round — typed in August, `9/10` is this
September and `3/4` is next March. The year turns on the month alone, so `8/1`
in August is the first of this August, which is what makes backdating a row a
fortnight a three-keystroke job. A field shows what was typed while the caret
is in it and the date it means once focus leaves. `←`/`→` nudge a date a day
wherever there is one, `Shift` with them a week, and a date field stays
typeable either way. A form editing a row opens on that row's own date; one
entering something new opens on today, bar a handful. The three that write a
ledger row — `a`, `t` and `p` — open on the date the last row added this
session was written for, since entering a statement is a run of rows landing on
the same few days; today, until a row is added, and restarting returns them
there. A new goal's opens on the first of the next month, since a goal date is
a deadline. `t`'s confirmation opens two business days out, dated for when the
transfers land rather than for when the plan was read — weekends skipped,
holidays not, which is part of why that date is editable. A recurring
transaction's horizon and the Funds birth-date prompt open blank, because
blank means something in both — a rule that does not end, and a date not on
record.

Each goal is dated for the year ahead: a year past the next occurrence of the
entry's month, so a September entry created in August 2026 is dated September
2027, and a March one — already past this year, and so next occurring in March
2027 — is dated March 2028. A biennial entry that already has a goal dated this
year steps two years instead, skipping the year between rather than filling it;
one that does not is due, and lands a year out like any other.

```bash
mm import path/to/Money.xlsx
```

Accepts `--db <path>` (default `~/.local/share/mistermanager/money.db`) and
`--today <YYYY-MM-DD>`.

**The first import against an empty database takes two runs of that same
command.** The `Savings` sheet identifies its two blocks by position and
carries no account code beside either, so nothing in the workbook says which
account holds which block. The first run therefore imports the accounts and
stops, printing what to do next: open the app, press `9`, and set the
`Savings` field on the two container accounts. Re-run the identical command
and the whole import completes in one pass. Only that first run is ever two
steps — the mapping is read before anything is cleared and written back
after, so `--replace` cannot reopen it.

The accounts arrive named after their codes, in the kind's default band. The
name, color, band, position, interest policy and `Savings` block are all yours,
set on screen `9`, and no import touches them again: `account` is deliberately
outside the tables a `--replace` clears. Neither is `recurring_txn` — the
rules you typed, and the paycheck flag among them, survive a re-import.

Import refuses to run against a database that already holds transactions or
goals -- re-running an import is not additive, so doing so unconditionally
would silently double every row on a second run. Pass `--replace` to clear
the previously imported data and import fresh:

```bash
mm import --replace path/to/Money.xlsx
```

The whole import runs in one SQL transaction, so a failure partway through
(a half-filled bill row, a missing sheet) leaves the database exactly
as it was before the run.

A database written by an older build is migrated on open: `schema.sql` is a
frozen baseline and every change since is an arm in a chain, so an existing
file takes the arms above its version and a new one takes the baseline and
then all of them. A database written by a *newer* build is refused instead,
naming both versions — the arms that produced it do not exist here. The way
out of that one is the same as it ever was: delete the file and re-import.
Every figure comes back out of the workbook; what does not — the recurring
transactions, and the naming, banding and ordering of the accounts — is quick
to re-enter.

## Demo mode

```bash
mm --demo
```

Draws the application exactly as an ordinary run does, with every absolute
dollar figure replaced by a run of blocks — for a screenshot, or for showing
the app to someone across a table. The same run of blocks whatever the figure:
the number of digits is itself a figure, so a mask that kept the shape of
`1,234.56` would hide the balance and publish its order of magnitude.

Percentages, dates, counts, and the names of accounts and goals are left alone
— a percentage is the shape of a plan rather than a sum, and a demonstration
with the Planning splits and the fund allocation blocked out would have nothing
left to show. Negatives keep their sign and their red.

Only the drawing changes. What is typed into a form still parses, so the app
can be driven rather than only watched, and the figures behind the mask are the
real ones: a form opened on a row commits what was already there. `mm import`
and `mm backup` take no such flag, because neither prints a figure. `mm report`
refuses it: the mask is installed by the screens, so a report written under the
flag would carry the real figures it exists to block.

## Report

Off until a config file switches it on:

```toml
# ~/.config/mistermanager/config.toml
[report]
dir = "~/Dropbox/money"   # required
```

`mm` writes a self-contained HTML page of the screens to `<dir>/Money.html`
every time it quits — one file, no separate stylesheet or
script, sized to read on a phone in light or dark. Pointing `dir` at a synced
folder is what turns "checked the balance on the laptop" into "checked it on
the phone a minute later."

The write happens after the TUI has already torn its screen down, the same
timing as the scheduled backup, and a failure prints to stderr rather than
making the run fail. `mm --demo` never writes a report: the file it would
overwrite is the one page nothing can regenerate without quitting an ordinary
session. `mm import` and `mm backup` don't write one either, since neither
launches the screens the page is drawn from.

The page carries six tabs, in the order the screens are numbered: Overview,
Cash, Credit, Savings, Planning, Funds. It opens on Overview, and the switch is
radio buttons and a stylesheet — no script, so it works on a phone with no
network and nothing to load. Each container's goals sit under its own heading,
the Planning tab shows the transfers over the waterfall that produced them, and
the footer says when the page was written.

Cash and Credit carry **every** transaction, not the month or two the screens
window to, with a dropdown that filters to one month at a time. They open on
the current month — or on all of them, in the first days of a month with
nothing entered in it yet. That dropdown is a `<details>` full of radio buttons
rather than a `<select>`: CSS cannot see which option of a `<select>` is
chosen, so a real one would render and do nothing.

To write one without opening the application:

```bash
mm report                    # into the configured dir
mm report --dir /tmp/export  # or anywhere else, config or no config
```

`--dir` is what makes the `[report]` section optional: leaving it out says "do
not write a page behind every quit", which is a different thing from "never
write me a page". Asked for outright, a failure is an error exit rather than a
line on stderr — the same way an explicit `mm backup` differs from the
scheduled one.

## Backups

`mm` uploads a copy of the database to S3 when the last upload is older than `interval_days`. The
check runs last, after the TUI has already torn its screen down, so a slow network holds up only
the shell prompt returning, and a failure prints to stderr rather than making the run fail.

The schedule belongs to the default database. A run given `--db` skips the check entirely: the one
state file records when a backup last ran, not which file it ran on, so uploading a scratch copy
would both leave a key indistinguishable from a real backup and hold the real database's next
upload off for a whole interval. `mm backup` still uploads whatever it is pointed at.

Backups are off until a config file switches them on:

```toml
# ~/.config/mistermanager/config.toml
[backup]
bucket        = "..."             # required
profile       = "mistermanager"   # default; a profile in ~/.aws/credentials
interval_days = 7                 # default
```

A key the file carries that `mm` does not read — a `[charts]` section, a misspelled
`interval_dayz` — is skipped rather than failing the run, so everything the build does understand
still takes effect. What that cannot hide is a misspelled `bucket`, or a misspelled `dir` in
`[report]`, because neither has a default: the one typo that would leave a feature silently
switched off is still an error.

The key prefix is not a setting. Objects go under `mistermanager/`, which is fixed in
`backup::PREFIX` because the IAM policy is scoped to that path and only a `terraform apply` can
widen it — a knob here could only ever be turned into `AccessDenied`. A `prefix` line in the config
file is one of those unread keys, and does nothing.

One-time setup: `terraform apply` creates the bucket, the IAM user and its access key, and its
outputs feed the `mistermanager` profile and the config file directly:

```bash
aws configure set aws_access_key_id "$(terraform output -raw mistermanager_access_key_id)" --profile mistermanager
aws configure set aws_secret_access_key "$(terraform output -raw mistermanager_access_key_secret)" --profile mistermanager
aws configure set region us-east-2 --profile mistermanager
terraform output -raw backup_bucket   # goes in config.toml's `bucket`
```

The third line is not optional. The region is read from the profile rather than from the config
file above — a second copy could only ever disagree with the first — so without it every upload
fails, and `AWS_REGION` is the only other place the SDK will look.

The bucket is created here too, as `mistermanager-<account id>-<region>-an`. The name is composed
from the caller's own account and region rather than chosen, which is what lets it sit in a public
repository: a name someone picked would say where the owner's finances are backed up, while a
derived one is legible only to whoever already holds the profile. Nothing about it is public —
public access is blocked four ways, and the only identity pointed at it may `PutObject` and nothing
else.

Its lifecycle rule deletes objects at 365 days.

The profile must carry static access keys: the AWS SDK is built here with `sso` and
`credentials-process` support left off along with the default HTTPS client, so an SSO profile or
one using `credential_process` will not authenticate.

`mm backup --status` prints the last upload and the next due date; `mm backup --force` uploads
regardless of the schedule.

To restore, quit `mm` first, then list the prefix and copy the object you want over the database:

```bash
aws s3 ls s3://<bucket>/mistermanager/
rm -f ~/.local/share/mistermanager/money.db-wal ~/.local/share/mistermanager/money.db-shm
aws s3 cp s3://<bucket>/mistermanager/money-20260820T140305Z.db ~/.local/share/mistermanager/money.db
```

The database runs in WAL mode, so a `-wal` file left over from a crash holds writes of its own; if
it is still there, SQLite replays it into the restored file the next time `mm` opens it, which is
why it has to go first.

Both commands use your own identity. The `mistermanager` profile can only `PutObject` — it cannot
read a backup, delete one, or list the prefix.

## Layout

| Path | Responsibility |
|---|---|
| `src/money.rs` | `Cents` — the only money type. No floats. |
| `src/rate.rs` | `Percent` and `BasisPoints` — the two scalings, kept apart. |
| `src/calc/` | Pure formulas: tax, biweekly, per-paycheck, pro-rata, the Planning waterfall, the fund allocation, and `schedule` — when a recurring thing happens. No database. |
| `src/db/` | The schema and the queries — one module per aggregate. |
| `src/db/bill.rs` | The monthly bill block, labelled — the `Planning!C6:E12` rows. |
| `src/db/fund.rs` | The `fund` table — the asset-allocation block of `Planning!I1:M5`. |
| `src/db/recurring_txn.rs` | The `recurring_txn` table — rows whose amount and date are known in advance. |
| `src/import/` | Reads `Money.xlsx`. |
| `src/import/fund.rs` | Reads the fund block, `Planning!I2:M<n>`, into `db::fund`. |
| `src/gate.rs` | `Gate` — the Planning gates, key and name substring together. |
| `src/savings_block.rs` | `Block` — the two blocks of the `Savings` sheet, each owning the key naming its container account. |
| `src/plan_line.rs` | Every Planning line: its label, its amount, and the key saying where it lands. |
| `src/plan.rs` | Runs the Planning waterfall against imported settings. |
| `src/fund.rs` | Feeds the fund table and the birth date to the fund-allocation derivation. |
| `src/recurring_txn.rs` | The policy over `db::recurring_txn`: horizons, adoption order, and regeneration. |
| `src/config.rs` | The TOML config file — the `[backup]` and `[report]` sections, unset means the feature is off. |
| `src/backup/` | The schedule, the snapshot, and the S3 upload. `aws-config`, `aws-sdk-s3` and `tokio` are named only in `src/backup/s3.rs`. |
| `src/tui/` | The terminal UI: screens, forms, key handling. |
| `src/tui/fund.rs` | The Funds screen and its form. |
| `src/tui/accounts.rs` | The Accounts screen — the six things the workbook does not say about an account. |

`ratatui` (which re-exports `crossterm`) is named only inside `src/tui/`, the
same discipline that confines `rusqlite` to `src/db/`, `calamine` to
`src/import/`, `serde` and `toml` to `src/config.rs` (and again in
`src/backup/state.rs`), and `aws-config`, `aws-sdk-s3` and `tokio` to
`src/backup/s3.rs`.

`rusqlite` is named only inside `src/db/`. Everything else holds a `db::Db`,
whose connection is private, and reaches the database through
`db::{account, bill, goal, recurring_goal, recurring_txn, setting, txn}`.
Multi-statement writes go through `Db::transaction`, which commits only if
its closure returns `Ok`.

Row ids are one type per table (`db::AccountId`, `db::GoalId`, …), so an id
cannot be passed where a different table's id belongs.

## Development

`pre-commit install` wires up the hooks in `.pre-commit-config.yaml`, which run
`cargo fmt --check` and the Terraform `fmt`/`validate` hooks — the latter two need `terraform` on
`PATH` — alongside the usual whitespace and YAML/TOML checks. GitHub Actions builds and tests every
pull request.

## Tests

`cargo test`. Integration tests read the workbook `MM_WORKBOOK` points at and
skip (printing a message explaining why) when that variable is unset or the
file is absent, so a clean checkout passes. They compare against the
workbook's own cached values rather than hardcoded balances, because the
workbook is a live document. The workbook is personal financial data and must
never be committed to the repository.

There is deliberately no default path. Where the owner keeps their finances is
the same kind of fact as an account code, and a fallback would put it back in
the repository in the one place nobody would think to grep for it.

Set `MM_REQUIRE_WORKBOOK=1` to turn a missing workbook into a hard test
failure instead of a skip -- useful for wiring these tests into CI on a
machine that is expected to have the workbook available, where a silent skip
would mean the importer is never actually exercised.

Three accounts are named by no cell of the workbook — the current account and
the two `Savings` block containers — and their codes are exactly the kind of
thing this repository may not hold either, so the tests read those from the
environment too:

```bash
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=path/to/Money.xlsx \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test
```

`tests/common/mod.rs` reads both, configures a database the way screen `9`
would, and runs the two-pass first import. Unset, the tests needing them skip
as loudly as they do for a missing workbook.

## No real data in the repository

This repository is public and the owner's finances are not. Nothing committed
here carries a real balance, a real institution, a real account code, or a
goal name traceable to a real person — in source, tests, fixtures or docs
alike. Every money literal in the crate is invented, and the tests that do
assert against real figures read them out of the untracked workbook at run
time rather than restating them. The root `CLAUDE.md` states the rule in full
and carries the invented-fixture vocabulary a new test should copy.
