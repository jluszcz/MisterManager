# report — the standing HTML report

`Snapshot` reads the Overview, both ledgers, Savings, Planning and Funds in one pass; `html`
renders them as one self-contained page, one module per tab, the way `tui` keeps one per screen;
`write` minifies that page and puts it on the disk atomically; `write_if_enabled` is the quit
path's gate over it. `minify_html` is named only in `mod.rs`.

The page carries **no script**, by rule, and is read offline on a phone. Almost everything below
follows from those two facts: what a control is allowed to be, what a column may do with its
width, and why the file is renamed onto its name rather than written to it.

`src/tui/CLAUDE.md` is the companion for the screens the page mirrors — where the two disagree
about a figure, one of them is wrong.

## Invariants

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
- **Every control on the page is CSS, and that is what the no-script rule
  buys.** The tabs and the ledgers' month filter are radio buttons the page
  never shows, plus a `:checked ~` rule per control; the radios are moved
  off-screen rather than `display:none`d, so they keep their place in the focus
  order. **A `<select>` cannot do this job**: no selector matches "the option
  that is chosen", so a real dropdown would render and then do nothing. That is
  why the month picker is a `<details>` full of labels -- it is the dropdown a
  page with no script is allowed to have. Every radio sits ahead of everything
  it addresses, since the sibling combinator only looks forward. A control
  whose rules are generated -- the months, whose set is a fact about the
  database -- generates them from the same list that generates its markup.
- **The Cash and Credit tabs carry every transaction, where the screens carry
  a window.** `tui::ledger::Window` exists because a terminal shows one screen
  at a time and `[`/`]` move it; a page is scrolled and filtered instead, and a
  report that stopped at the current window would be missing exactly what
  someone opens it to check. The rows are grouped into one `<tbody>` per month
  and the filter shows one of them -- so the page's size is the whole ledger
  whatever the dropdown says, which is the cost of the file being readable
  offline with no query to re-run. It **opens on the month `today` falls in**,
  and on `All months` when that month has no rows yet: a selection matching no
  group would draw an empty table and no reason for it, which is what the first
  of a month would otherwise look like.
- **The Overview is one table on the page, where it is three on the screen.**
  The three projection dates are that table's header row, and a header labels
  only the table it sits in -- split per section, each would size its columns
  to its own longest figure, so Cash would not line up with Credit and only the
  first would be dated at all. The sections keep their separation as `<tbody>`
  groups. The dates go bare, no `To-Date`/`Paycheck-Eve`/`Month-End` above
  them, which is what `tui::overview::column_headers` does for the same reason.
- **The Planning tab draws the list `plan_rows::rows` hands it and decides
  nothing about it.** Which blocks there are, what they are headed, what each
  row is called and where the two footers sit are the waterfall's facts, not
  the page's: `html::planning` turns each row into a `<tr>` and stops. Two
  classes rather than one, because what a row *is* and how deep it sits are
  separate facts and `tr.tot td` and `tr.sub2 td:first-child` are separate
  rules; `Row::depth` of `n` is `sub{n}`, and `1` is a block's own line, which
  takes no indent because the heading above it has already set the block
  apart. The screen spends that same number as two spaces a level, and goes on
  spending them, so the page spells a class a level and `html::planning`
  generates the rule behind each -- the way `ledger::month_rules` generates
  one per month. One `sub` for every level past the first would draw flat a
  row the screen draws nested, which is the drift one shared list of rows
  exists to prevent. A
  `Kind::Blank` draws nothing at all -- the screen's separator is an empty
  line, and this medium's is `tr.head td`'s padding. Before this, the tab and
  the screen were hand-transcriptions of each other and had already come to
  disagree about which blocks were headed.
- **A cell says what it may do with its width, and a table never widens past the
  phone.** Three classes carry it: `n` is a figure and `d` a date, and neither
  wraps — a comma and a hyphen are both break opportunities, and a column narrow
  enough takes them, which is what once drew `2026-08-` over `22` across all
  three of the Overview's headers. Refusing to wrap puts a floor under those
  columns, so `w` names the one that gives way instead: the owner's own free
  text, a ledger description or a goal name, the longest text on the page and
  the only text still legible broken mid-word. Every other column keeps its
  longest word whole, which is what stops an account name from being shredded to
  make room — and `overflow-x` on the panel is what catches what those floors
  leave: an account name is the owner's text as well, so a long enough one puts
  its table past the phone whatever the description gives up, and the panel is
  then what scrolls rather than the page under every tab. It sits on the panel
  and not on a wrapper around each table because the month filter reaches its
  rows as `#month:checked~table.ledger`, and a `<div>` between those two would
  leave the dropdown showing nothing. The table face and its
  padding are set by the widest tab rather than by the prettiest: Savings is six
  columns, one dated and three of them money, inside the 361px a phone leaves.
- **The report is renamed onto its name, never written to it.** A sync client
  watching the directory will upload a half-written page, and a phone would then
  show a report that ends mid-table with no sign that it had. The temporary file
  sits in the same directory so the rename does not cross a filesystem, carries
  the pid in its name because an `mm report --dir` run can overlap an open app's
  quit path in the same directory, and is removed again on any failure — a
  rename that cannot happen would otherwise leave the partial page in the synced
  folder under a name nothing ever looks for again.
- **The page is minified on the way to the disk, not on the way out of `html`.**
  The whole file crosses a sync folder and is read on a phone offline — every
  row of both ledgers, not a window on them — so its size is worth spending a
  dependency on; but `html::page` is what every test in
  `src/report/html/` asserts exact markup against, and a module whose output no
  longer matched what it was checked for would be testing the minifier instead.
  So `report::minify` sits in `write`, the one seam both writers already pass
  through, and `Written::bytes` reports what actually landed. `minify_css` is on
  because the page's whole layout is one inline `<style>`; `minify_js` is off
  because the page carries no script by rule. What comes out is aggressive —
  a lowercased doctype, unquoted attributes, no `<head>`, no closing tag that
  HTML5 makes optional — and since every control on the page is a radio and a
  `:checked ~` selector, `minification_leaves_every_tab_and_its_switch_intact`
  is what stands between that and a page which renders and then does nothing.
