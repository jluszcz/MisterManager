# MisterManager

A terminal application for tracking money, replacing a per-year spreadsheet.

## Usage

```bash
mm            # launch the application
mm --demo     # the same, with figures and names disguised (needs --features demo)
mm report     # write the HTML report without opening the application
mm import ... # load a Money.xlsx workbook (needs --features import)
```

Screens are `1` Overview, `2` Cash, `3` Credit, `4` Savings, `5` Planning, `6` Funds,
`7` Recurring Goals, `8` Recurring Txns, `9` Accounts; `q` quits. **`?` opens the key reference for
whichever screen you are on**, and it is where every key is spelled out — what follows is the part
of the app a list of keys cannot tell you. The screens are laid out for a terminal at least 120
columns wide. Accounts read by the name and color you gave them on screen `9`, everywhere but
Recurring Txns, whose columns leave room only for the code.

Three things hold across every screen that offers them. `t` moves money, wherever that is: one row
on a ledger, every row a plan calls for, or a figure crossing between savings goals. `/` searches
the list, matching amounts typed without the separators the column draws — `1234` finds
`$1,234.56`. And `Esc` gives back everything the screen is narrowed by in one press, because a
screen that narrows two ways should not make you work out which of them is hiding the row you are
looking for.

### `1` Overview

Accounts stacked in bands — checking, then savings, then the cards — with a subtotal under each and
a total under each kind.

`←`/`→` scrub the Paycheck-Eve column against the date derived from the paycheck transaction, which
is always a day still ahead of today: on the eve itself the column names the eve of the paycheck
*after* rather than naming today, and it reads today until a transaction is marked on screen `8`.
`Shift` with them moves a week, as it does on every date in the app. **The scrub reaches Planning**,
whose excess is quoted at whatever the column is left at.

### `2` Cash and `3` Credit

The two ledgers share one month, so `[` and `]` step both and the two always compare the same weeks.
Each title ends with the balance of whatever `Tab` narrows the screen to — `Cash · Aug 2026 · All ·
Today $42,000.00` — and that figure is the to-date balance the Overview quotes, so neither the month
on show nor a search moves it.

Narrowed to one account, `r` takes the balance a statement says that account holds and the title
carries the difference after it — `… · Today $1,160.00 · Target $1,200.00 · Δ -$40.00`, green above,
red below, a dash when they match — so a typo or a missed row shows up while the rows are still
being entered. Nothing is written: quitting forgets every target.

### `4` Savings

Every open goal with the container it belongs to. `Tab` filters by container, `[` and `]` by goal
date, `a` allocates against the selected goal, `e` edits it, `t` moves part of its value to another
goal in the same container, and `c` ends it — returning its value to unallocated, or moving it to
another goal in that container.

Goals with no date lead the list, in an order you set with `K` and `J`; goals with one follow,
soonest first, since a deadline decides a goal's place for it. `f` marks a goal for the eye only;
the mark is stored on the goal, so unlike an account's color it does not survive a `--replace`.

An allocation's amount takes `/N` for a fraction of the container's unallocated remainder — `/2` is
half of it, `/12` a twelfth — and the form names the remainder it would divide before committing.

`Enter` opens the selected goal's allocation history: every row its balance is the sum of, footed by
what they come to, so an audit visibly adds up. A figure entered wrongly four batches ago is
rewritten in place rather than offset by a second row, and any row is editable whether a payday, an
interest posting or the import wrote it. What a row cannot change is the goal it belongs to: moving
one across containers would move a goal's value with no cash moved between the accounts. A
misdirected allocation is deleted here and re-entered with `a` on the right goal.

`A` opens the allocation worksheet for the container — one amount, a line per goal, and a live
remaining counter, committed as a single batch that `U` takes back whole. `i` posts interest from
the container's unallocated remainder, and `n` opens a blank goal (`n`, not `a`, which this screen
spends on the allocation it is mostly used for).

### `5` Planning

The transfer instructions first — one per destination account — over the waterfall that worked them
out: the excess, the monthly bill block with its biweekly column, the gates, the split, and where
each line lands. `↑`/`↓` move between the editable constants and skip everything computed.

`p` pins the excess so the plan stops moving underneath a payday; pressing it again re-pins at
whatever the excess reads now, and `P` unpins. `Excess (Used)` is an editable constant like any
other, so a figure typed there pins that instead of the one `p` computed.

**The transfers never total more than the excess.** On a payday too small for the fixed bills,
housing is paid first and the line that gave way carries the gap beside it. The excess is the
checking balance at Paycheck-Eve, so a scrubbed plan names its date beside `Excess (Actual)`, and
`t` and `p` act on the figures shown rather than on the derived date.

### `6` Funds

The target/actual split across the funds, with a `Total` row under them. One fund's target is not a
stored figure: it tracks the owner's age directly — a percentage point past thirty for every year —
and the rest split whatever share of the target that leaves. Whichever row sits furthest below its
own target draws in bold, which is where the next contribution belongs; nothing is marked once every
fund is at or above its target. Nothing here moves money, so a delete moves no balance either.

Entering the screen with an age-tracked fund and no birth date on record opens a one-field form
asking for it. `Esc` leaves that fund's target blank rather than guessing, and the screen asks again
the next time it has no answer.

### `7` Recurring Goals

The table each round of goals is created from. `s` opens the picker, where `Space` toggles an entry
and `Enter` creates every ticked one as a goal in the container the Savings screen's `Tab` names,
all in one transaction.

Whatever the month filter is showing opens already ticked and sorted to the top, since a tick alone
is easy to miss in a list dozens long. Every entry is still listed below, so the filter is a
starting point rather than a cage; an entry that already has an open goal opens unticked, and
`Space` still adds it, because a second open goal against one entry is legitimate.

**Each goal is dated for the year ahead**: a year past the next occurrence of the entry's month, so
a September entry created in August 2026 is dated September 2027, and a March one — already past
this year — is dated March 2028. A biennial entry that already has a goal dated this year steps two
years instead, skipping the year between rather than filling it.

### `8` Recurring Transactions

The rows whose amount and date are known in advance — the paycheck and the monthlies. `P` marks the
transaction the Paycheck-Eve column is derived from. `g` regenerates the selected one and `G` every
one, reporting `removed / released / adopted / inserted`.

Regeneration adopts a matching unclaimed row before inserting, so the first `g` after an import
claims what the workbook already held instead of duplicating it. A row it owns on a date the
schedule no longer produces — the mortgage moved by hand from the 1st to the 5th — is *released*
back to the ledger rather than deleted, and so are every one of a rule's rows when `d` deletes it. A
delete never moves a balance.

### Typing

Every box that takes text edits with the readline keys — `Ctrl`+`A`/`E`/`B`/`F`/`W`/`U`/`K`/`D`, as
a shell binds them. `Ctrl` means editing text and nothing else anywhere in the app; `Alt` is unused,
since macOS sends `Option` as `Meta` only where the terminal has been told to.

Every date is typed as `YYYY-MM-DD` or as `M/D`, which takes the next year that month comes round —
typed in August, `9/10` is this September and `3/4` is next March. The year turns on the month
alone, so `8/1` in August is the first of *this* August, which is what makes backdating a row a
fortnight a three-keystroke job. `←`/`→` nudge a date a day, `Shift` with them a week, and `[`/`]` a
month, with the day clamped into the month it lands in: the 30th of January steps to the 28th of
February and stays a 28th on the way back out.

A form editing a row opens on that row's own date, and one entering something new opens on today —
bar a handful that open somewhere more useful. The three that write a ledger row open on the date
the last row added this session was written for, since entering a statement is a run of rows landing
on the same few days. A new goal's opens on the first of the next month, since a goal date is a
deadline. `t`'s confirmation opens two business days out, dated for when the transfers land rather
than for when the plan was read, and the worksheets it queues behind that confirmation open on the
date it wrote — an allocation is the transfer read from the container's side, so both carry the one
date. And a recurring transaction's end date and the Funds birth-date prompt open blank, because
blank means something in both: a rule that does not end, and a date not on record.

## Importing a workbook

```bash
mm import path/to/Money.xlsx
```

Accepts `--db <path>` (default `~/.local/share/mistermanager/money.db`) and
`--today <YYYY-MM-DD>`.

**The importer is a build-time feature, off by default**, the way demo mode
is:

```bash
cargo install --path . --features import
```

A default build has no `import` subcommand and none of the code behind it,
and `calamine` — named nowhere else in the crate — is not compiled at all.
The workbook is a document the owner imports from once per edit of it, not
something the application reads to open a database, so the binary that runs
day to day need not carry a spreadsheet parser. The rest of this section
describes that build.

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
name, color, band, position, interest policy, `Savings` block and which of the
two money forms open on the account are all yours, set on screen `9`, and no
import touches them again: `account` is deliberately outside the tables a
`--replace` clears. Neither is `recurring_txn` — the rules you typed, and the
paycheck flag among them, survive a re-import.

Two of those are `setting` keys rather than columns on the row, and the
`Default` field is where both are pointed: `t` opens its `From` on the account
named for a transfer, `p` on the one named for a payment. They are separate
keys because paying a card and moving savings are separate decisions -- which
is also what lets one account answer for both. A key naming an account that is
gone is not an error here, unlike the `Savings` block's: the form opens on the
head of its list, which is a prefill you can see before pressing `Enter` and
correct on the very screen that sets it.

An account the workbook does not name is yours to create there too: `a` asks a
code, a kind and a name, and writes the same row an import would — default
band, no color, last among its kind. The code is asked there and nowhere else,
because it is what the next import matches the row against, so giving one the
code the sheet later grows means that import adopts the row rather than writing
a second.

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

Demo mode is a build-time feature, off by default:

```bash
cargo install --path . --features demo
```

A default build has no `--demo` flag, because it carries none of the code
behind it.

Draws the application exactly as an ordinary run does, with every absolute
dollar figure's digits replaced by another figure's digits, and every account
name and code, goal name, recurring-goal name, bill label, fund name and
transaction description replaced by a same-length pronounceable pseudoword — both keyed on a salt drawn once
per run, so one amount draws the same everywhere it appears, one word reads
the same wherever it appears, and whole dollars agree with the figure they
came from.

That costs what a fixed-width mask would have hidden: a figure's order of
magnitude and a name's length are both now visible, and a derived figure no
longer reconciles with the ones under it — a scrambled Net is not the sum of
the scrambled bands above it. It is obfuscation for a demonstration, not a
security control: the salt lives only in the process for the run, and
nothing should be built on it.

Percentages, dates, counts, and the app's own words are left alone — a
percentage is the shape of a plan rather than a sum, a scrambled date is not
a date, and a count over rows the reader can see would read as a rendering
fault. Negatives keep their sign and their red.

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
the footer says when the page was written. A goal's `%` is colored the way the
Savings screen colors it — red at nothing saved, yellow at halfway, green at
funded, and every shade between — off the same ramp, so a goal is the same
color on the phone as it is in the terminal.

Cash and Credit carry **every** transaction, not the month or two the screens
window to, with a dropdown that filters to one month at a time. They open on
the current month — or on all of them, in the first days of a month with
nothing entered in it yet. That dropdown is a `<details>` full of radio buttons
rather than a `<select>`: CSS cannot see which option of a `<select>` is
chosen, so a real one would render and do nothing.

The file is minified before it lands, since a page carrying every transaction
crosses a sync folder on its way to a phone. Nothing about reading it changes;
only viewing its source does.

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

There is no key prefix. A backup is `money-<timestamp>.db` at the root of the bucket, which the
application owns outright and which holds nothing else — a prefix would name the only thing in
there, while being a string `backup::key_for` and the IAM policy each spell separately, with
`AccessDenied` as the way they announce having come apart. A `prefix` line in the config file is one
of those unread keys, and does nothing.

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

Its lifecycle rules move an object to Standard-Infrequent Access at 30 days and delete it at 365.
Thirty is IA's own minimum billable duration, so nothing is charged for storage it did not use, and
IA's retrieval charge is only ever paid on a restore, since nothing reads a backup on a schedule.
IA also bills a 128 KB floor per object and S3 declines to transition anything under it, so the rule
is a saving on a database with a ledger in it and a no-op on one without.

The profile must carry static access keys: the AWS SDK is built here with `sso` and
`credentials-process` support left off along with the default HTTPS client, so an SSO profile or
one using `credential_process` will not authenticate.

`mm backup --status` prints the last upload and the next due date; `mm backup --force` uploads
regardless of the schedule.

To restore, quit `mm` first, then list the bucket and copy the object you want over the database:

```bash
aws s3 ls s3://<bucket>/
rm -f ~/.local/share/mistermanager/money.db-wal ~/.local/share/mistermanager/money.db-shm
aws s3 cp s3://<bucket>/money-20260820T140305Z.db ~/.local/share/mistermanager/money.db
```

Backups written before the prefix was dropped are still under `mistermanager/`, where `aws s3 ls`
shows them as a single `PRE mistermanager/` line rather than as objects — so until the last of them
expires at 365 days, the newest backup may be in there rather than at the root, and
`aws s3 ls s3://<bucket>/mistermanager/` is what lists it. `mm backup --status` names the object it
last wrote, prefix and all.

The database runs in WAL mode, so a `-wal` file left over from a crash holds writes of its own; if
it is still there, SQLite replays it into the restored file the next time `mm` opens it, which is
why it has to go first.

Both commands use your own identity. The `mistermanager` profile can only `PutObject` — it cannot
read a backup, delete one, or list the bucket.

## Layout

A Rust crate whose layering is enforced by module privacy rather than by convention: the
dependencies that would otherwise reach everywhere are confined by name — `ratatui` to `src/tui/`,
`rusqlite` to `src/db/`, `calamine` to `src/import/`, which is what lets that last one be optional
and a default build carry no spreadsheet parser at all — everything outside `src/db/` reaches the
database through the query modules rather than a connection, ids are one type per table, and
`Cents` is the only money type in it. `CLAUDE.md` carries the path-by-path map and states each of those rules in
full; the module `CLAUDE.md` files under `src/import/`, `src/calc/`, `src/tui/`, `src/report/` and
`src/backup/` go a level below it.

## Development

`pre-commit install` wires up the hooks in `.pre-commit-config.yaml`, which run
`cargo fmt --check` and the Terraform `fmt`/`validate` hooks — the latter two need `terraform` on
`PATH` — alongside the usual whitespace and YAML/TOML checks. GitHub Actions builds and tests every
pull request.

## Tests

`cargo test` runs everything a clean checkout can run. The integration tests assert against a live
workbook rather than hardcoded balances, and skip — printing why — when they cannot find it, so
running them for real takes three variables:

```bash
MM_REQUIRE_WORKBOOK=1 MM_WORKBOOK=path/to/Money.xlsx \
  MM_ACCOUNTS=<checking>,<goals>,<buckets> cargo test --features import
```

`MM_WORKBOOK` is the path to the workbook, `MM_ACCOUNTS` the three accounts no cell of it names —
the current account and the two `Savings` block containers — and `MM_REQUIRE_WORKBOOK=1` turns a
skip into a failure. `--features import` is part of that invocation rather than an extra on it:
every one of these binaries is behind the feature, so without it they compile to nothing and the
run goes green having asserted nothing.

Why there is no default path, and the rest of what that sentence is protecting, are in `CLAUDE.md`
under "The workbook is the test oracle". `tests/common/mod.rs` is what reads the three variables.

## No real data in the repository

This repository is public and the owner's finances are not. Nothing committed here carries a real
balance, a real institution, a real account code, or a goal name traceable to a real person, and the
tests that do assert against real figures read them out of the untracked workbook at run time.
`CLAUDE.md` states the rule in full and carries the invented-fixture vocabulary a new test copies
from.
