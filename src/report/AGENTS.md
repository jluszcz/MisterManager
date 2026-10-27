# report — the standing HTML report

`Snapshot` reads the Overview, both ledgers, Savings, Planning, the allocation and Retirement in
one pass. `view` is what the page's script is handed: every tab as data, every decision made.
`html` builds that payload, one module per tab the way `tui` keeps one per screen, and puts it in
`finance-utils`' shell (`report::html::app_page`) beside `report.js`, which draws it, and
`report.css`. `write` hands the page to `finance-utils`' `report::write`, which minifies it and
puts it on the disk atomically; `write_if_enabled` is the quit path's gate over it, and delegates
to the crate's. `minify_html` is named in `finance-utils`, not here.

The page is **a static shell, one JSON payload and one script**, and it is read offline on a
phone. Almost everything below follows from those two facts: where a decision is allowed to be
made, how owner text gets onto the page, and why the file is renamed onto its name rather than
written to it.

`src/tui/AGENTS.md` is the companion for the screens the page mirrors — where the two disagree
about a figure, one of them is wrong.

## Invariants

- **Rust decides; the script lays out.** A figure's sign and its tone (`palette::Sense`, so a
  card paid down is green), the funding color and the `%` text, whole-dollar truncation, the
  waterfall's rows and depths, the allocation's Δ signs and the bar's widths, a chart's
  coordinates: all of it is settled in `view` and the tab modules, from the same `palette`,
  `account_label` and `plan_rows` the screens read. Every figure reaches the payload as the text
  it is drawn as, so the script formats nothing and there is no second money formatter to drift
  from `Cents`' `Display`. What `report.js` decides is only placement — which element a cell
  becomes, which grid row it lands on — so a payload test is a test of the page's content, and
  `html::page_view` is what those tests assert against.
- **The payload is safe inside a `<script>` by one rule: every `<` is `<`.**
  `finance-utils`' `script_json` applies it, which covers `</script>` and `<!--` in any string,
  and JSON reads the escape back as the character it stands for. The script then inserts owner
  text through `textContent` only. The one `innerHTML` is a chart's SVG, which `html::retirement`
  builds and whose only text it escapes. `owner_text_cannot_end_the_payload` and
  `minification_keeps_the_payload_and_the_renderer` hold the rule before and after the minifier.
- **The payload is shaped for size.** Arrays over objects where a row repeats hundreds of times
  (`view::Cell` is bare text, a `[text, tone]` pair, or an index), short keys on `view::Row`,
  every unset field omitted, each account once in `Page::accounts` however often it is drawn, and
  a ledger row carrying only its day, its month's key completing the date. The ledgers are nearly
  the whole page.
- **A tone is a class, with one exception.** `view::tokens_css` emits the eight account colors,
  `NEGATIVE`/`POSITIVE`, the on-track middle, the four `palette::CLASSES` and `TOTAL` as custom
  properties with a class each; the rest of the stylesheet is the static `report.css`. A cell's
  tone names one of those classes — `neg`, `pos`, `mid`, `total`, `a-<AccountColor::as_str>` —
  except a goal's funding `%`, which is a `#rrggbb` off `palette::percent`: the ramp is
  continuous, so it has no class to name, and the script sets it as `style.color`. Dark mode
  re-inks a mark by redefining its token, never by a second rule per element; `TOTAL` is the one
  token it redefines, black on that ground being no line at all, so `--total` follows the shell's
  `--fg`. A lighter dark variant of any other color would come from `palette.rs` the same way.
- **Two switches, independent: width decides a row's shape, the pointer decides its height.**
  Below 48rem rows take their two-line shapes and the shell puts the tab bar at the bottom with
  icons and short labels; above it every table is one line in its columns under a top tab row.
  Under `(pointer: coarse)` every row and control is at least 44px, and the two-line forms 48px;
  under a fine pointer the wide rows are the dense ~32px ones. So every iPad, in either
  orientation, gets touch-sized rows, single-line wherever it is wide enough.
- **Each table is a class, and a row is its own grid placed by position.** `ov`, `lg`, `sv`, `pl`,
  `fs`, `fh`, `rs`, `rm`, `ra` and the histories' `hs`; `report.css` places each cell by
  `nth-child` under its row's class, so a column's meaning lives in one rule rather than on every
  cell. Because each row sizes its own grid, a column sized to its content lines up with nothing
  above or below it, so every column a reader scans down a table has a fixed width. Content-sized
  columns are left where that costs nothing: the column flush against the right edge, which ends
  at the same place whatever its width; a ledger's date, every one the same width; the figures a
  two-line form sets on its second line, which a reader takes with the name above them rather
  than down the table; and Planning's last column, which has a 5rem floor and grows past it for
  a figure too wide for it, moving only that row's figure rather than spilling into the gutter. A
  row with fewer cells
  than its table spans its last cell across the rest — a Planning heading, the Retirement row that
  says to set a salary — which the script marks `span` and the stylesheet places. Below the
  breakpoint:
  - **Savings:** the goal name (the tap target) and its `%` in its funding color; then the
    current figure, `of` the goal, and `Due` the date where there is one, in secondary text; then
    a thin meter in the same color. Above it, Savings sets its containers side by side wherever
    two of them fit, one column under that.
  - **Ledgers:** `date · account` in small secondary text, then the description, which wraps, and
    the amount.
  - **Overview:** the name, then its three figures under the three dates.
  - **Planning** and the **Funds** summary: one line each. A **holding**, a **milestone**, a
    **standing** and a held **account**: two, their header row laid out in the same places so a
    figure is still under its label.
- **The page opens on the Overview and remembers nothing.** The shell switches panels; no tab, no
  month and no open history survives a reload, so the page a phone opens is always the one it
  was written as.
- **The report is written on quit, and never under `--demo`.** That is
  `write_if_enabled`, the gate; `report::write` underneath it is the disk, and
  `mm report` calls that one directly. **An unset `[report]` section is an
  answer to the quit path's question and not to the subcommand's**, which is
  why `--dir` can stand in for it: "do not write a page behind every quit" is
  not "never write me a page". Both reach the disk through the one `write`, so
  the atomic rename has no second implementation to drift from.
  `write_if_enabled` returns before it queries anything when the flag is set: `demo::install` sets
  a thread-local flag once, before the first frame, and nothing ever clears it
  -- `main` regains control on that same thread when `tui::run` returns, so the
  flag it set is still on there, and a report written under it would be a page
  of scrambled figures over the one file that cannot be regenerated without
  quitting an ordinary session. **That skip is the whole of the guard, and
  there is no second line behind it.** The page's figures are formatted off
  `Cents` directly rather than through `demo::figure`, but text is a different
  story: an account reaches the page through `account_label::Account` and a
  description through `description::render`, both of which mask where the text
  becomes a display and both of which the screens share. A page written under
  the mask would therefore carry real figures beside pseudonymous names, which
  is worse than either -- so the flag is checked before anything is queried
  rather than relied on to be harmless. Its dates come
  from `projection::dates`, never from `App::adhoc`: the scrub is a
  hypothetical the owner left a cursor on, and a report cannot say which day
  it was quoting.
  `mm report` refuses `--demo` rather than ignoring it: no subcommand installs
  the mask, so the flag would quietly write the real figures the mask exists
  to keep off the page.
- **A quit that changed nothing does not rewrite the day's page.** The gate is there
  because `dir` names a synced folder: a rename onto the name is an upload and then a
  download on a phone, spent on bytes that come out the same. `finance-utils`'
  `report::is_due` is that gate -- a run that wrote a row is owed a page, and so is a
  run whose directory holds none
  written on the day it is quoting -- and it is where both halves' blind spots are set
  out, since `db::Db::wrote_rows` counts one connection and an mtime is the day a page
  was *written* rather than the day it quotes. **`mm report` is not gated**, for the
  same reason an unset `[report]` section does not stop it: it reaches `write`
  directly, and being asked for outright is the point.
- **The Cash and Credit tabs carry the three months behind the current one,
  the current one, and every future month** -- `report::HISTORY_MONTHS`,
  where the cut is argued. The ledgers are nearly the whole page, so carrying
  every row would grow a file that crosses a sync folder on every write by a
  month of rows each month; anything older is the screen's, where `[` reaches
  it. The filter is a native `<select>` the script redraws the rows from, so
  the page's size is every month carried whatever it says, which is the cost of
  the file being readable offline with no query to re-run. It lists `All
  months` and then the months newest first, and **opens on the month `today`
  falls in**, and on `All months` when that month has no rows yet: a selection
  matching nothing would draw an empty list and no reason for it, which is what
  the first of a month would otherwise look like. **The Credit tab reads its
  signs the way the Credit screen does**: a figure below zero is the card paid
  down and draws in `palette::POSITIVE`, through the same
  `palette::Sense::of_ledger` and `palette::amount` the screen reads, so a
  payment cannot be good news in one medium and bad in the other.
- **The Overview is one table on the page, where it is three on the screen.**
  The three projection dates are that table's header row, and a header labels
  only the rows under it -- split per section, only the first would be dated,
  and on a wide page each would size its columns apart. The sections keep their
  separation as `sep` on each one's first row. The dates go bare, no
  `To-Date`/`Paycheck-Eve`/`Month-End` above them, which is what
  `tui::overview::column_headers` does for the same reason.
- **The Planning tab draws the list `plan_rows::rows` hands it and decides
  nothing about it.** Which blocks there are, what they are headed, what each
  row is called and where the two footers sit are the waterfall's facts, not
  the page's: `html::planning` turns each row into a payload row and stops. A
  row's kind and whether the Biweekly Expenses figure counts it are classes
  (`head`, `note`, `tot`, `counted`), and its `Row::depth` goes on as a number
  the script spends as an indent a level at a time -- the screen spends that
  same number as two spaces a level, and a page that collapsed every level past
  the first onto one indent would draw flat a row the screen draws nested. A
  `Kind::Blank` draws no row at all: the screen's separator is an empty line,
  and this medium's is `gap` on the row *below* it, decided from the list
  because a row does not know what sits above it. A counted row's label takes
  a `•` as a `::after`, the way an expired goal's takes `!` -- it costs the
  table no column on a phone. That it is a class here and a character in the
  screen's label column is the whole reason `Row::counts_as_expense` is a flag
  rather than a glyph, the same argument `Row::depth` is a number for.
- **The Funds tab is a spelling of the Funds screen, and the bar is widths settled in Rust.**
  The rows, the four classes and the apportioning behind them are `allocation::Class`'s, the same
  list the screen draws; what is decided here is what this medium has to decide. A charting
  library is the one thing on a page read offline that could fail to arrive, so every segment's
  share is computed by `html::funds` and the script only sets it, and each class's own share is
  written out in the legend, since a figure is what a reader checks a picture against. A segment
  carries its class's position in `Class::ALL`, which is the order `palette::CLASSES` and so the
  `--c{n}` tokens are written in — nothing stores that position, so the two move together, and
  `palette`'s own test is what holds the lengths level. The Δ is the one cell here that takes a
  tone, `neg` where the portfolio is short of what the rule asks —
  `allocation::SummaryRow::delta` being `actual - target`, so short is the negative one. **The
  class labels stay plain and the legend stays**, where the screen tints its labels and drops its
  legend: this page has the width for a legend under the bar and a terminal does not. Either way
  a class is named in its own color exactly once.
- **A goal's history is a `<dialog>`, filled from `Page::histories`.** Tapping a goal's name on
  the Savings tab opens what `Enter` opens on the screen — the goal's note, its allocations
  oldest first at the cent, and their total, which is the balance beside the name. Each goal row
  points at its entry by index, so names, which are not unique, are never a key. Below the
  breakpoint it is a bottom sheet up to 85% of the viewport's height, its name and a 44px `×` in a
  header that stays put while the rows scroll; above it, a centered modal 34rem wide. A tap on the
  backdrop closes it, and `prefers-reduced-motion` drops its slide. Every goal's history is on the
  page whether it is opened or not, the ledgers' cost again -- so it takes the ledgers' cut too,
  `report::HISTORY_MONTHS`. What is older is summed into one `Earlier` line ahead of the rest
  rather than dropped, because the total has to land on the balance; a lone older allocation
  stays as itself, there being nothing to save. `Snapshot::load` reads them through
  `goal::allocations`, the reader the screen's history uses, so the two cannot list different
  rows.
- **Two tabs leave off columns their screens carry, because a phone cannot fit them.** Funds draws
  the summary's Class, Target, Actual and Δ and not the three tax columns; Savings draws no
  `$/Pay`. Each is a figure spent at the screen -- where a holding sits, what a payday prefills --
  rather than one read off the page, and it is the dropped column the screen still answers.
- **The page has no `Tab`, so the accounts are stacked sections instead.** The screen narrows its
  summary and its holdings together with one key; a page has no key, and a reader scrolls. Each
  section carries both halves for that reason, and its summary is absent where the account has no
  composition at all — a table of dashes over every class says only that a key has not been pressed
  yet, on a page where pressing it is not even possible.
- **A fund's own composition is nowhere on this page, though the screen this tab spells does carry
  it** — `Stock%` is what the holdings table drops, so a reader who knows the screen will look for
  it and not find it; `HOLDINGS_HEADER` is where that is argued and what it costs is stated. The
  trade runs the other way for `As of`, which this table keeps and the screen no longer has: the
  date a composition was read from matters to the reader who is away from the app and cannot press
  `g` to move it.
- **The Retirement tab is a spelling of screen 8**, and is last because the tabs run in key order
  and the report carries none of the screens numbered after it. It reads `crate::retirement` and
  decides only what this medium has to — a status's tone as `neg`, `mid` or `pos` off the three
  `palette::standing` answers, and a `$` on every dollar figure, cells included, because its
  tables set balances beside multiples and shares and a bare `1,253,067` under `Balance` is one
  header away from reading as either. **Its two charts are `balance_history::Charts` drawn as
  inline SVG built in Rust**, the bar's argument again: every coordinate is computed at render
  time, and every line is inked by class (`ln a-<color>`, `ln total`) in `currentColor`, so the
  tokens are the only thing that colors one. They span the whole history, since a page has no key
  to move the screen's window, and share the screen's one scale for the screen's reason. They are
  stacked rather than side by side, each being half a phone otherwise, and carry no hover layer;
  the gridline figures and the legend are what a reader checks a line against.
- **The report is renamed onto its name, never written to it.** A sync client
  watching the directory will upload a half-written page, and a phone would then
  show a report that ends mid-table with no sign that it had. The temporary file
  sits in the same directory so the rename does not cross a filesystem, carries
  the pid in its name because an `mm report --dir` run can overlap an open app's
  quit path in the same directory, and is removed again on any failure — a
  rename that cannot happen would otherwise leave the partial page in the synced
  folder under a name nothing ever looks for again.
- **The page is minified on the way to the disk, not on the way out of `html`.**
  The whole file crosses a sync folder and is read on a phone offline — months
  of both ledgers, not a window on them — so its size is worth spending a
  dependency on; but `html::page` is what the page-level tests assert against,
  and a module whose output no longer matched what it was checked for would be
  testing the minifier instead. So the minifier sits in `finance-utils`'
  `report::write`, the one seam both writers already pass through, and
  `Written::bytes` reports what actually landed. It minifies the CSS and the
  script as well as the markup, and what comes out is aggressive — a lowercased
  doctype, unquoted attributes, no closing tag that HTML5 makes optional — so
  `minification_keeps_the_payload_and_the_renderer` is what stands between that
  and a page which renders and then draws nothing: the payload has to parse back
  to the very view `html` built, hostile goal name included, beside the script
  that reads it.
