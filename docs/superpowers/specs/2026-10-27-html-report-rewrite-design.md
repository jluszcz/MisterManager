# HTML report rewrite — design

## Intent

`mm` writes `Money.html` into the report directory on quit (and on `mm report`). The owner opens it
when MisterManager and the database are out of reach, usually on an iPhone or iPad inside a
file-sync app, sometimes on a computer. It is read-only.

The page has grown tab by tab since it was first written, and two things have gone wrong:

- **On phones and tablets the touch targets are too small and the information is too dense.**
  The page shrinks its type on a narrow screen (0.82rem tables, 0.25rem padding, ~22px rows)
  instead of re-laying out.
- **It is larger than it needs to be.** The current page is large on a real database. Most of it is
  goal histories, one popover per goal rendered as full HTML whether opened or not, and the Credit
  ledger; the CSS is a small share, with many generated per-tab, per-month and per-level rules and an
  inline `style=` on most percentages.

At the same time, desktop gets no benefit from its width: the page is capped at 40rem everywhere.

This is a ground-up rewrite of the page in MisterManager, and of the shared page shell in
`finance-utils` that Paychecker also uses.

## Goals

1. **Phone and iPad: touch-sized and readable, with no horizontal scrolling anywhere.** Every
   tappable thing and every table row is at least 44px tall. Body text is about 16px, secondary
   text no smaller than about 13px. Where a row's columns can't fit at phone width, the row
   becomes two lines instead of the type shrinking.
2. **Desktop: denser.** A wider page (up to about 72rem), single-line rows around 32px tall, and
   side-by-side sections where that helps (Savings' containers).
3. **Smaller file.** Shared styles, no per-row repetition of anything the stylesheet can say once,
   and data carried as compact JSON instead of markup. My estimate, not a measurement: roughly
   half of the current page. Success means materially smaller on the same database, as
   reported by `Written::bytes`.
4. **Light and dark follow the system setting.** No toggle.
5. **Same content.** What the report shows today is the owner's stated minimum. Nothing is dropped
   and nothing new is required. The columns today's page deliberately leaves off (Funds' tax
   columns and `Stock%`, Savings' `$/Pay`) stay off, since that is what "what's currently there"
   means.

**Priority, for where polish goes first:** Overview and Savings, then the Cash and Credit ledgers,
then Funds, Retirement and Planning.

## Non-goals

- No editing, no forms, nothing that writes. The page is a snapshot.
- No new data, tabs or figures beyond today's.
- No change to *when* the page is written (quit gate, `is_due`, `mm report`), to the atomic
  rename, to the `--demo` refusal, or to `HISTORY_MONTHS` (three months back plus every future
  month, older goal allocations folded into one `Earlier` line).
- No theme toggle.
- No redesign of Paychecker's own tables. It only moves onto the new shell.

## Anti-goals (failure even if it technically works)

- Anything that scrolls sideways at phone width, the page or a table.
- Type that shrinks to fit instead of re-laying out.
- A request to the network of any kind: fonts, CDNs, images. The page is opened offline.
- Financial logic in JavaScript. The script formats and lays out. It never decides a figure, a
  sign's color, a status or a rounding. Those stay in Rust, read off the same functions the TUI
  reads, so the page and the screens cannot disagree.
- Real data in the repository: fixtures and tests use the invented vocabulary in `AGENTS.md`.

## Constraints

- **Self-contained single file:** inline CSS, inline JS, inline data, favicon as a `data:` URI.
  It is read from a sync folder, possibly offline.
- **Verified viewer:** the file-sync iOS app's preview (WKWebView, iOS 18.7) runs JavaScript and
  supports `:has()` and `popover`, tested on 2026-10-27 with a probe page. Recent Safari, Chrome
  and Firefox on desktop are the other targets.
- **Minified on the way to disk** by `finance-utils`' `report::write`, which today runs
  `minify-html` with `minify_js: false`. Inline script makes that `true`.
- `rusqlite` stays in `src/db/`, `serde_json` is already a MisterManager dependency, and
  `palette.rs` stays the only place a color is a number.

## Decided

### Script

**The owner chose script throughout.** The page is a static shell plus one JSON payload plus one
script that draws every tab from it. This retires the "no script, every control is a radio and
`:checked ~`" rule that `src/report/AGENTS.md` and `AGENTS.md` are built around.

### Tabs

Seven, in screen-number order: **Overview, Cash, Credit, Savings, Planning, Funds, Retirement.**
Planning stays. (An earlier "omit Planning" was a mix-up with the Recurring screen, which was
never in the report.)

- **Phone and iPad portrait:** a fixed bottom tab bar, each button about 62px tall with a simple
  stroke icon above a short label: `Overview`, `Cash`, `Credit`, `Savings`, `Plan`, `Funds`,
  `Retire`. Padded for the home-indicator safe area. The owner approved this in the prototype.
- **Wide screens:** a sticky top tab row with full labels and no icons.
- The page opens on Overview. The script switches panels. Whether the chosen tab is remembered
  across opens (`localStorage`, or the URL hash) is the builder's call.

### Breakpoint and touch sizing

Two independent switches, because most iPads are wider than any phone breakpoint (an 11" iPad is
820–834px in portrait) and the owner wants touch sizing on every iPad:

- **Row shape follows width.** Below about **48rem**, rows go two-line and the tab bar moves to the
  bottom. Above it, rows are single-line with today's columns and the tabs sit on top.
- **Row height follows the pointer.** Under `(pointer: coarse)` (phones and iPads in any
  orientation), every row and control is at least 44px and the type stays at the touch sizes.
  Under a fine pointer (a mouse), wide layouts use the dense ~32px rows. A narrow desktop window
  gets the two-line shapes at mouse density, which is fine.

So an iPad in portrait or landscape gets single-line rows that are still touch-sized. A phone gets
two-line rows. A desktop gets dense single-line rows.

### Row shapes below the breakpoint

All approved in the prototype:

- **Savings:** each goal is two lines. Line 1 is the goal name (the tap target, dotted underline)
  with `%` right-aligned in its funding color. Line 2 is `balance of target` on the left and
  `Due <date>` on the right, in secondary text. Under that is a thin progress bar in the same
  color. Favorites keep their tinted background, and an expired goal keeps its `!` marker.
- **Ledgers (Cash, Credit):** line 1 is `date · account` (the account in its color) in small
  secondary text. Line 2 is the description (wraps) with the amount right-aligned. Future rows are
  dimmed as today.
- **Overview:** line 1 is the account or subtotal name, line 2 its three figures in three
  right-aligned columns under a header of the three dates. Section subtotals and Net are bold.

### Above the breakpoint

Every one of those becomes a single line with the columns today's tables have. Savings lays out
its containers in two columns.

### Other controls

- **Ledger month filter:** a native `<select>` (Month: All months, then each carried month),
  opening on the current month, or All months when the current month has no rows, as today.
- **Goal history:** tapping a goal opens a `<dialog>`. On phones it is a bottom sheet up to about
  85% of the viewport height, with a sticky header holding the name and a 44px `×`, and scrolling
  rows. On wide screens it is a centered modal about 34rem wide. Tapping the backdrop closes it.
  The content matches today's: note, allocations oldest first at the cent, `Earlier` line, total
  equal to the balance.

### Theme

Color tokens on `:root`, redefined under `@media (prefers-color-scheme: dark)`. Every component
reads tokens. The eight account colors, `NEGATIVE`/`POSITIVE`, the funding ramp, the four
`palette::CLASSES` and `TOTAL` are emitted from `palette.rs` as custom properties. That is the one
generated part of the stylesheet. The rest of the CSS is static. Dark mode may need lighter
variants of the account and status colors to stay legible, and if so they come from `palette.rs`
too, not from the stylesheet.

### Sizes the prototype settled on

Body 16px. Secondary line about 0.82rem. Touch rows at least 48px; dense rows about 32px. Side
gutter `max(16px, safe-area)`. Treat these as a starting point.

## Architecture (decided at the level that matters)

- **Rust builds a view model and serializes it.** `Snapshot` (or a view struct beside it) becomes
  `serde::Serialize`. The page embeds it as `<script type="application/json" id="d">…</script>`
  and the script reads it with `JSON.parse`. Arrays over objects where a row repeats hundreds of
  times, cents as integers, accounts by index into one account table, and colors as token names,
  never hex.
- **Every value that involves a decision is resolved in Rust:**
  - The sign class of a figure (`palette::Sense`, so Credit's payments are green).
  - The funding color and `%` text (`BasisPoints`/`Percent` display).
  - Whole-dollar truncation (`Cents::trunc_to_dollar`).
  - Planning's rows, depths, Δ cells and footers, as `plan_rows::rows`.
  - The allocation rows and Δ signs.
  - Retirement standings and chart coordinates, or at least their scale.

  JS turns an integer of cents into `1,234.56` or `1,234` and nothing more. One JS formatter must
  match `Cents`' `Display`, and a test should pin a few values on both sides.
- **The JSON must be safe inside `<script>`.** Escape `<` as `\u003c` (which covers `</script>` and
  `<!--`) when embedding. The script inserts owner text only through `textContent` or a single
  escape helper. The rule `escape` states today still holds.
- **The CSS and JS are static files** in `src/report/` (`report.css`, `report.js`), pulled in with
  `include_str!`, so they read and diff as ordinary CSS and JS. Only the palette tokens and the
  JSON are generated.
- **`finance-utils` owns the shell:** `<head>`, viewport, favicon, the token and theme base, the
  tab bar's markup and behavior (bottom on narrow, top on wide, icons optional per tab), the footer
  stamp, and `minify_js: true`. The radio-based `tab_inputs`/`tab_nav`/`tab_rules` are replaced.
  **Paychecker moves onto it:** its year tabs become shell tabs without icons, and its panels stay
  the static HTML they are now. Its "no script" doc comments are updated. Its tables are otherwise
  untouched.
- **The tab bar has two forms, and the shell picks one by how the tabs are given.** Tabs with icons,
  which MisterManager's fixed seven have, get the bottom bar on narrow screens. Tabs without icons,
  such as Paychecker's year list that grows by one a year, always get a top row of 44px buttons
  that *wraps* onto more lines and never scrolls sideways. My call.
- **The shell's base styles are only the tokens, the body font, the tab bar and the footer.** Table
  metrics stay with each app, so Paychecker keeps its current table sizing. Verification still
  checks that Paychecker's page doesn't scroll sideways at phone width under the shell. My call.
- **Landing order across the three repos:** the new shell lands in `finance-utils` beside the old
  radio helpers. MisterManager and Paychecker then move onto it, each bumping its `Cargo.lock`.
  The old helpers are removed last. My call.

## Tests

- **MisterManager:** the HTML-markup assertions in `src/report/html/*` mostly become assertions on
  the serialized view model. They are simpler and closer to the facts that matter (rows, order,
  signs, colors, the `HISTORY_MONTHS` cut, the `Earlier` line footing). Keep the page-level
  checks:
  - no `http` reference
  - every tab's panel present
  - the JSON payload survives minification and parses back to the same value
  - owner text containing `</script>` cannot break out
- **JS rendering:** whether to add a headless browser or a Node smoke test is the builder's call,
  given what the dev machine and CI have. At minimum, check the rendered page by hand in a desktop
  browser at phone and desktop widths in both themes, and on the owner's phone through the file-sync app,
  before calling it done.
- **On-device handoff:** the builder writes a page rendered from the invented test fixture (no
  real data) into the report directory under a name other than `Money.html`, as the
  script probe was. The owner opens it on the iPhone and iPad and reports back. After that, the
  owner runs `mm report` on the real database for the final look. No preview mode is added, and
  the `--demo` refusal stays. My call.
- **finance-utils and Paychecker:** existing tests updated for the new shell. Paychecker's page
  still switches years.

## Documentation

Docs describe the code as it is, not how it got here:

- Rewrite `src/report/AGENTS.md` around the new facts: script plus JSON, Rust decides and JS
  formats, the breakpoint and row shapes, the palette tokens. Drop every radio-and-`:checked`
  invariant.
- Update `AGENTS.md` where it says the page carries no script (the `report` row of the
  architecture table, and the invariants that cite `report::html::*`).
- Update the `finance-utils` docs on `report` (no-JS minify, tab helpers).

## Left to the builder

- The JSON's exact shape, and the module split between Rust view structs and `html/` renderers.
  Keep one module per tab.
- **Funds, Retirement and Planning below the breakpoint**, following the same patterns. My calls:
  - **Planning:** one line per row (label, amount), with the Δ shortfall cell moved under the label
    when it doesn't fit.
  - **Funds:** the allocation bar full width with the legend wrapping. The summary table (Class,
    Target, Actual, Δ) fits as one line. Holdings rows go two-line (ticker and fund name on top,
    balance and as-of below) if they don't fit.
  - **Retirement:** charts drawn as inline SVG by the script, full width, stacked. The milestone
    table goes two-line on phones instead of today's smaller face.
- Icons (simple inline SVG strokes), exact spacing and colors within the tokens, and the motion
  (a short sheet slide-in, respecting `prefers-reduced-motion`).
- Whether to remember the last tab.
- How to verify the JS, per Tests.

## Calls I made (for the owner to check)

- Script is used for the whole page, not just the histories. That is my reading of "just go with
  scripts".
- Columns today's page deliberately drops stay dropped.
- Paychecker gets the new shell, with its tables unchanged.
- No theme toggle.
- Dark-mode color variants, if needed, live in `palette.rs`.
- Row shape switches by width at 48rem, and touch sizing by `pointer: coarse`, so every iPad gets
  touch-sized single-line rows.
- The tab bar has two forms: bottom bar with icons for MisterManager, and a wrapping top row for
  icon-less tabs such as Paychecker's years.
- The shell's base styles leave table sizing to each app.
- On-device checks use a fixture-rendered page in the report directory, then a real `mm report`.
- `finance-utils` keeps the old tab helpers until both apps have moved.
