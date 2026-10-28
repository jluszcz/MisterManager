# HTML Report Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild `Money.html` as a touch-friendly, responsive, much smaller page: a static shell, a JSON payload decided in Rust, and one script that lays it out. Move Paychecker onto the same shared shell.

**Architecture:** `finance-utils` gains a script-capable page shell with a two-form tab bar. MisterManager's `report::html` modules stop writing markup and instead build a serializable view (`report::view`) that holds only decided text and classes. A static `report.js` turns that view into DOM, and a static `report.css` lays it out by width (row shape) and by pointer (row height). Paychecker keeps its static HTML panels inside the new shell.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already MisterManager deps), `minify-html` 0.18 (with `minify_js: true`), plain ES2020 and CSS with no libraries.

**Spec:** `docs/superpowers/specs/2026-10-27-html-report-rewrite-design.md`

## Global Constraints

- One self-contained file. No request of any kind: no `http`, fonts, CDNs or external images. The favicon stays a `data:` URI.
- Seven tabs, in this order: `overview` Overview, `cash` Cash, `credit` Credit, `savings` Savings, `planning` Planning (short label `Plan`), `funds` Funds, `retirement` Retirement (short label `Retire`). The page opens on Overview.
- Row shape switches at `48rem`: below it rows are two-line and the tab bar sits at the bottom; above it rows are one line and the tabs sit on top.
- Row height switches on `(pointer: coarse)`: there every row and control is at least `44px`. With a fine pointer and a wide screen, rows are about `32px`.
- Body text is `16px`; secondary text is no smaller than about `0.82rem`. Side gutter is `max(16px, env(safe-area-inset-*))`. The bottom tab bar is about `62px` plus `env(safe-area-inset-bottom)`.
- Light and dark follow `prefers-color-scheme` only. There is no toggle.
- **JS decides nothing.** Every figure's text, sign class, color, rounding, order and label comes from Rust. JS only builds DOM, switches tabs and filters months, opens histories, and joins a ledger row's month key with its day.
- Owner text reaches the DOM only through `textContent`. The single exception is the chart SVG, which Rust builds with `escape`.
- JSON embedded in `<script>` has every `<` written as `\u003c`.
- Content matches today's report exactly: the same columns (Funds without tax columns or `Stock%`, Savings without `$/Pay`), `HISTORY_MONTHS`, the `Earlier` line, and the same empty-state sentences.
- Fixtures use only the invented vocabulary in `AGENTS.md`. No real data goes into any tracked file or commit.
- Never commit a `[patch]` of `finance-utils` into any `Cargo.toml`.
- Every commit uses the `jluszcz:commit` skill on a feature branch: `report-shell` in finance-utils, `report-rewrite` in MisterManager, `report-shell` in Paychecker.
- Every MisterManager commit passes `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`.

## Refinements over the spec (decided here)

- **Money crosses as already-formatted strings, not integer cents.** Rust's `Cents` `Display`, `to_whole_dollars` and `trunc_to_dollar` produce the text, so the page needs no JS money formatter. That is stricter than the spec's "JS formats cents".
- **The funding `%` color is the one hex value in the payload.** `palette::percent` is a continuous ramp and no token can carry it. The cell's tone is then a `#rrggbb` string from `palette::hex`, still decided in Rust.
- **Retirement charts stay SVG built in Rust**, using `currentColor` with account classes instead of hex, and are inserted as one markup string. This keeps coordinate maths out of JS.

## Review Focus

1. **Owner text containing `</script>`, `<`, `&` or quotes** (a goal named `a</script><b>`) must render literally and must not end the JSON block. The test lives in Task 6.
2. **The minifier leaving the page working with `minify_js: true`.** The JSON must survive byte-for-byte and the script must still run. The test lives in Task 6, and headless Chrome renders the minified file.
3. **Empty databases:** no goals, no ledger rows (`no transactions`), an unresolvable plan, no holdings, no history. Each tab must show its sentence, not a blank panel or a JS exception. Tests for each tab's view live in Tasks 3–5, and the rendered page is checked in Task 6.
4. **A month with no rows today:** the month filter must open on `All months`, as now. The test lives in Task 4.
5. **A very long goal name or description at 375px wide** must wrap inside its cell, never widen the page. The Task 6 render check includes a fixture name of at least 60 characters.

---

### Task 1: finance-utils — script-capable shell and two-form tab bar

**Files:**
- Modify: `../finance-utils/src/report/html.rs` (add the new shell beside the existing `page`/`tab_*`, which stay until Task 8)
- Create: `../finance-utils/src/report/shell.css`, `../finance-utils/src/report/shell.js`
- Modify: `../finance-utils/src/report/mod.rs` (`minify`: `minify_js: true`)
- Modify: `../finance-utils/AGENTS.md` (report section)

**Interfaces:**
- Produces:
  ```rust
  pub struct NavTab { pub id: String, pub label: String, pub short: Option<String>, pub icon: Option<&'static str> }
  impl NavTab { pub fn new(id, label) -> NavTab; pub fn short(self, s) -> NavTab; pub fn icon(self, svg_inner: &'static str) -> NavTab }
  pub struct App<'a> {
      pub title: &'a str, pub favicon: Option<&'a str>,
      pub style: &'a str,              // the app's own CSS, appended after shell.css
      pub tabs: &'a [NavTab], pub open: usize,
      pub panels: &'a [String],        // one per tab, in order: static HTML, or "" for a script to fill
      pub data: Option<&'a str>,       // JSON text; emitted as <script type="application/json" id="d">
      pub script: Option<&'a str>,     // the app's JS, emitted after shell.js
      pub stamp: &'a str,
  }
  pub fn app_page(app: &App) -> String
  pub fn script_json(json: &str) -> String   // replaces every '<' with "<"
  ```
- Markup contract that later tasks' CSS and JS rely on: `<nav class="tabs icons">` when every tab has an icon, otherwise `<nav class="tabs wrap">`. Each tab is `<button role="tab" data-t="{id}" aria-selected>` containing an optional `<svg viewBox="0 0 24 24">{icon}</svg>`, then `<span class="s">{short or label}</span><span class="l">{label}</span>`. Panels are `<section id="{id}-panel" role="tabpanel">`, `hidden` on every tab but `open`. `shell.js` switches `hidden` and `aria-selected` on click and scrolls to the top.
- `shell.css` defines the base tokens `--bg --fg --mut --rule --soft --acc` on `:root`, redefines them under `@media (prefers-color-scheme: dark)`, and styles `body`, `main`, the footer stamp and both nav forms:
  - `.icons`: fixed to the bottom below `48rem` (about 62px buttons with icon and short label), a sticky top row with full labels and no icons above `48rem`.
  - `.wrap`: always a top row of buttons at least 44px tall that wraps with `flex-wrap` and never scrolls sideways.

  It sets no table metrics.

- [ ] **Step 1: Write the failing tests** in `html.rs`'s `mod tests`:
  - `script_json_escapes_every_angle_bracket`: `script_json(r#"{"a":"</script><!--"}"#)` contains no `<` and contains `\u003c/script>`.
  - `app_page_hides_every_panel_but_the_open_one`: with three tabs and `open: 1`, exactly two `<section` tags carry `hidden`, and `id="b-panel"` does not.
  - `a_tab_list_with_icons_draws_the_icon_bar_and_one_without_draws_the_wrapping_row`: `class="tabs icons"` versus `class="tabs wrap"`.
  - `app_page_embeds_data_and_scripts_in_order`: the JSON block comes before `<script>` holding shell.js, which comes before the app script.
  - `app_page_makes_no_external_request`: no `http` in the output (the favicon is base64).
  - In `mod.rs`: `minified_page_keeps_its_json_and_script`: minify an `app_page` with data `{"k":"v<"}` and a script `document.body.dataset.ok=1`, then assert that the output contains `\u003c` and `dataset.ok`.
- [ ] **Step 2:** Run `cargo test --all-features report::` and check that they FAIL (the items are undefined).
- [ ] **Step 3:** Implement `NavTab`, `App`, `app_page` and `script_json`, plus the two static files pulled in with `include_str!`. Set `minify_js: true` in `minify`.
- [ ] **Step 4:** Run `cargo test --all-features` and `cargo clippy --all-targets --all-features -- -D warnings`. Expect a PASS.
- [ ] **Step 5:** Update `AGENTS.md` (the report section: the new shell, both nav forms, JS minified). Commit with `feat(report): add a script-capable page shell with a two-form tab bar`, then push and open a PR. **The owner merges it before Task 2's commit.** Until then, Tasks 2–6 build against an *uncommitted* `[patch."https://github.com/jluszcz/finance-utils"] jluszcz_finance_utils = { path = "../finance-utils" }`.

---

### Task 2: MisterManager — the view model and palette tokens

**Files:**
- Create: `src/report/view.rs`
- Modify: `src/report/mod.rs` (`pub mod view;`)
- Test: `src/report/view.rs` `mod tests`

**Interfaces:**
- Produces (all `#[derive(Serialize)]`, serialized compactly):
  ```rust
  /// "text" | ["text","class-or-#hex"] | account index
  #[serde(untagged)] pub enum Cell { Text(String), Toned(String, String), Account(usize) }
  pub struct Row {
      pub c: Vec<Cell>,
      #[serde(skip_serializing_if = "Option::is_none")] pub k: Option<String>, // row classes, space-separated
      #[serde(skip_serializing_if = "Option::is_none")] pub h: Option<usize>,  // index into Page::histories
      #[serde(skip_serializing_if = "Option::is_none")] pub d: Option<u8>,     // Planning depth ≥ 2
      #[serde(skip_serializing_if = "Option::is_none")] pub m: Option<u8>,     // Savings meter, 0..=100
  }
  #[serde(tag = "t")] pub enum Block {
      Heading { text: Cell },                       // h3; an account heading is Cell::Account
      Subheading { text: String },                  // h4
      Note { text: String },
      Table { class: &'static str, head: Vec<String>, rows: Vec<Row> },
      Ledger { months: Vec<Month>, opens: Option<String> /* key, None = All months */, empty: Option<String> },
      Bar { segments: Vec<(String /* width % */, usize /* palette::CLASSES index */)>, legend: Vec<(String, usize)> },
      Chart { title: String, svg: String, legend: Vec<(Cell, Option<usize>)> }, // svg markup built by Rust; legend color = account (Cell::Account) or Total (None)
  }
  pub struct Month { pub key: String, pub label: String, pub rows: Vec<Row> }  // ledger row c = [day "03", Account(i), desc, amount]
  pub struct History { pub name: String, pub note: Option<String>, pub rows: Vec<Row> }
  pub struct TabView { pub id: &'static str, pub blocks: Vec<Block> }
  pub struct Page { pub accounts: Vec<(String, String /* "a-<AccountColor>" */)>, pub tabs: Vec<TabView>, pub histories: Vec<History> }
  pub struct Accounts { /* builds Page::accounts */ }
  impl Accounts { pub fn new() -> Self; pub fn cell(&mut self, a: &crate::account_label::Account) -> Cell; pub fn into_vec(self) -> Vec<(String, String)> }
  pub fn money(sense: palette::Sense, text: String, cents: Cents) -> Cell   // Toned with "neg"/"pos" per palette::amount, else Text
  pub fn whole(cents: Cents) -> Cell                                         // trunc_to_dollar, Natural sense
  pub fn tokens_css() -> String
  ```
- `Accounts::cell` deduplicates by (name, color) through `Account::render_with`, so masked text never enters the payload under `--demo`. The report never runs under the mask anyway.
- `tokens_css()` emits:
  - `:root{--a-<c>:#..;…;--neg;--pos;--c0..--c3;--total}` from `palette`.
  - `.a-<c>{color:var(--a-<c>)}` for every `AccountColor::ALL`, plus `.neg{color:var(--neg)}` and `.pos{…}`.
  - `@media (prefers-color-scheme:dark){:root{--total:var(--fg)}}`.

- [ ] **Step 1: Write the failing tests:**
  - `a_cell_serializes_as_text_pair_or_index`: `json!(Cell::Text("x"))=="x"`, `Toned` gives `["x","neg"]`, `Account(2)` gives `2`.
  - `a_row_omits_every_unset_field`: `Row{c:vec![],k:None,..}` serializes to `{"c":[]}`.
  - `one_account_is_one_entry_however_often_it_appears`: two `cell` calls on the same account give the same index, and `into_vec().len()==1`.
  - `money_below_zero_is_negative_and_a_credit_payment_is_positive`: `Sense::Natural`, −1 → tone `neg`; `Sense::of_ledger(Kind::Credit)`, −1 → `pos`.
  - `tokens_name_every_account_color`: for each `AccountColor::ALL`, `tokens_css()` contains `.a-{as_str}{` and `--a-{as_str}:`.
- [ ] **Step 2:** Run `cargo test --lib report::view` and check that it FAILs.
- [ ] **Step 3:** Implement the items above.
- [ ] **Step 4:** Run `cargo test --lib report::view` and expect a PASS.
- [ ] **Step 5:** Commit `feat(report): add the report's serializable view and palette tokens`.

---

### Task 3: Overview and Savings views

**Files:**
- Modify: `src/report/html/overview.rs`, `src/report/html/savings.rs` (add the `view` functions beside the existing markup, which Task 6 deletes)
- Modify: `src/report/html/fixture.rs` (add `fn json(v: &impl Serialize) -> serde_json::Value`)

**Interfaces:**
- Consumes: Task 2 `Cell`, `Row`, `Block`, `History`, `Accounts`, `money`, `whole`.
- Produces:
  - `overview::view(o: &Overview, acc: &mut Accounts) -> TabView`. It returns one `Table{class:"ov"}`. `head` is `["", to_date, adhoc, month_end]`, with dates as `%Y-%m-%d`. Account lines have `c = [Account, 3×money(Natural, to_string)]`. Band subtotals (when `breaks_down()`), section totals and `Net` take `k:"tot"`, and each section's first row takes `k` including `"sep"`.
  - `savings::view(cs: &[Container], acc: &mut Accounts, histories: &mut Vec<History>) -> TabView`. For each container it returns `Heading{Account}`, then `Table{class:"sv", head:["Goal","Current","Goal","%","Goal Date"]}`:
    - Each goal row has `c = [name, whole(current), whole(goal), percent, goal_date or ""]`, where `percent` is `Toned("{p}%", palette::hex(palette::percent(p)))` or `Text("--")`.
    - `k` holds `fav` and/or `expired`; `h` is the history index; `m` is `p` clamped to `0..=100`, or `None` when the percent is `None`.
    - Then an `Unallocated` row: `c=["Unallocated", whole(unallocated(excess)), "", "", ""]`, `k:"unalloc"`.
  - Each `History` has rows `[date, description::render(note), money(Natural, cents.to_string())]`, ahead of an `Earlier` row `["Earlier", "{n} allocations before {%b %Y}", …]`, plus a final `k:"tot"` row `["Total","",sum]`. With no allocations, `rows` is empty and `note` is still set, and JS shows `No allocations yet.`. That sentence is `pub const NO_ALLOCATIONS` in `view.rs` and is emitted as a `History.rows`-empty marker, so JS reads the text from the payload rather than restating it. Add `pub empty: Option<&'static str>` to `History` for this.

- [ ] **Step 1: Write the failing tests:** port today's assertions to the JSON. The existing tests in `overview.rs` and `savings.rs` name the facts. Keep their sentences as test names, rewritten against `json(&view(..))`. They must include:
  - `the_three_projection_dates_head_the_overview`
  - `credit_totals_are_bold_and_net_closes_the_table`
  - `a_favorite_goal_is_marked_and_an_expired_one_flagged`
  - `a_goals_history_foots_to_its_balance_with_earlier_folded_in` (the sum of the amount cells parsed back as `Cents` equals the row's `current`)
  - `a_goal_with_no_allocations_says_so`
  - `the_funding_percent_is_colored_by_the_ramp` (the tone equals `palette::hex(palette::percent(Percent(40)))`)
  - `unallocated_closes_each_container`
- [ ] **Step 2:** Run `cargo test --lib report::html::overview report::html::savings` and check that the new tests FAIL.
- [ ] **Step 3:** Implement both `view` functions.
- [ ] **Step 4:** Run the same command and expect a PASS.
- [ ] **Step 5:** Commit `feat(report): build the Overview and Savings tabs as data`.

---

### Task 4: Cash and Credit ledger views

**Files:**
- Modify: `src/report/html/ledger.rs`

**Interfaces:**
- Consumes: Task 2.
- Produces `ledger::view(l: &Ledger, acc: &mut Accounts) -> TabView`, holding one `Block::Ledger`:
  - `months` come newest first, as the dropdown lists them today. Each row has `c = [day "%d", Account, description::render(desc), money(Sense::of_ledger(kind), cents.to_string())]`, with `k:"future"` when the row is in the future.
  - `opens` is `Some(current)` when that month has rows, else `None`.
  - `empty` is `Some("no transactions")` when there are no months.
- JS contract (Task 6): the select lists `All months` and then the months; a displayed date is `key + "-" + day`.

- [ ] **Step 1: Write the failing tests** (port from `ledger.rs`'s 11):
  - `the_filter_opens_on_the_current_month`
  - `a_month_with_no_rows_yet_opens_on_all_months`
  - `a_credit_payment_draws_positive`
  - `a_future_row_is_dimmed`
  - `rows_carry_only_their_day_and_the_month_key_completes_the_date` (`key` is `2026-10` and the day cell is `"03"`)
  - `an_empty_ledger_says_no_transactions`
  - `one_account_on_many_rows_is_one_accounts_entry`
- [ ] **Step 2:** Run `cargo test --lib report::html::ledger` and check that it FAILs.
- [ ] **Step 3:** Implement the view.
- [ ] **Step 4:** Run the same command and expect a PASS.
- [ ] **Step 5:** Commit `feat(report): build the ledger tabs as data`.

---

### Task 5: Planning, Funds and Retirement views

**Files:**
- Modify: `src/report/html/planning.rs`, `src/report/html/funds.rs`, `src/report/html/retirement.rs`

**Interfaces:**
- Consumes: Task 2.
- Produces:
  - `planning::view(p: &Planning, acc: &mut Accounts) -> TabView`.
    - `Unresolvable(msg)` gives `Note{msg}`.
    - `Resolved` gives one `Table{class:"pl", head: vec![]}` from `waterfall(view)`, which stays as it is:
      - `Blank` gives no row.
      - `Heading` and `Note` give a one-cell row with `k:"head"` or `k:"note"`.
      - A figure or total gives `c=[label, figure, extra]`. `label` is Text, or Account for `RowLabel::Account`. `figure` comes from today's `Value` match, using `whole` for money. `extra` comes from today's `Extra` match, with `Gap` as `Toned("Δ {whole dollars}","neg")`.
      - `k` holds `tot`, `counted` and `gap` exactly as today's `render` decides them, and `d` holds `Some(depth)` when `depth >= 2`.
  - `funds::view(a: &Allocation, acc: &mut Accounts) -> TabView`: the same sequence as today's `sections`, with each piece mapped:
    - `NO_HOLDINGS` and `NO_COMPOSITION` become `Note`.
    - `<h3>Allocation</h3>` becomes `Heading{Text}`.
    - Coverage becomes `Note`.
    - The summary becomes `Table{class:"fs", head:["Class","Target","Actual","Δ"]}`, with Δ toned `neg` when below zero.
    - The bar becomes `Bar`, with segment widths computed exactly as today's `bar()` and legend entries `("{label} {share}%", class.index())`.
    - Each account becomes `Heading{Account}`, then its summary and bar, then `Table{class:"fh", head:["Ticker","Balance","As of"]}`.
  - `retirement::view(r: &Retirement, c: Option<&Charts>, acc: &mut Accounts) -> TabView`:
    - The title becomes `Heading{Text}`.
    - The standing becomes `Table{class:"rs"}`, or its sentence becomes `Note`. The status cell's tone is a class: `neg` for `Short`, `mid` for `OnTrack`, `pos` for `Ahead`. Add `--mid` to `tokens_css` from `RAMP_MID` through a new `pub const STANDING_MID: Rgb` in `palette`, and make `standing()` use it.
    - The milestones become `Table{class:"rm", head:["Milestone","× salary","Saved $","Tax-free"]}`, and the accounts table follows the same pattern.
    - Each chart becomes `Chart{svg}`, built by today's `chart()` maths with every `stroke="#hex"` replaced by `class="ln a-<color>"` (or `class="ln total"`) and `stroke:currentColor` in CSS. Grid and axis keep their classes.
    - The empty sentences become `Note`.

- [ ] **Step 1: Write the failing tests** (port each module's tests):
  - `an_unresolvable_plan_is_its_message`
  - `blank_rows_draw_nothing_and_the_row_after_takes_gap`
  - `depth_two_and_deeper_carry_their_depth`
  - `a_shortfall_is_a_negative_delta_in_the_extra_cell`
  - `a_counted_row_is_marked`
  - `no_holdings_says_so` and `no_composition_says_so`
  - `a_short_class_delta_is_negative`
  - `the_bar_segments_never_exceed_the_whole`
  - `a_charts_lines_carry_their_account_class_and_no_hex`: the `svg` string contains no `#` followed by six hex digits.
  - `retirement_without_marked_accounts_says_where_to_mark_them`
  - `an_on_track_status_is_mid`
- [ ] **Step 2:** Run `cargo test --lib report::html` and check that the new tests FAIL.
- [ ] **Step 3:** Implement the three views and the `STANDING_MID` change.
- [ ] **Step 4:** Run `cargo test --lib` and expect a PASS, including `palette`'s own tests.
- [ ] **Step 5:** Commit `feat(report): build the Planning, Funds and Retirement tabs as data`.

---

### Task 6: The renderer, the page switch, and the old markup removed

**Files:**
- Create: `src/report/report.js`, `src/report/report.css`
- Modify: `src/report/html/mod.rs` (`page()` rewritten; markup helpers, `STYLE`, `narrow_rules`, `tab_bar`, `MENU`, `NARROW`, `amount_rules` and `account_rules` deleted)
- Modify: every `src/report/html/*.rs` (delete the old markup functions and their tests)
- Modify: `src/report/html/fixture.rs` (delete the HTML-parsing helpers `tables`, `cells`, `row_column_counts`, `misaligned_headers`, `without_positional_tables` and `panel`; add `pub(super) fn long_names_snapshot() -> Snapshot` with a goal name and a description of at least 60 characters)
- Modify: `src/report/mod.rs` (the module doc; the `minification_leaves_every_tab_and_its_switch_intact` test rewritten)
- Modify: `src/report/AGENTS.md` (rewritten), `AGENTS.md` (the `report` architecture row and every "no script" or radio statement, found with `grep -n "script\|radio\|:checked" AGENTS.md src/report/AGENTS.md`)

**Interfaces:**
- Consumes: Task 1 `App`, `app_page`, `script_json` and `NavTab`. Tasks 2–5 `view` functions.
- `page(snapshot) -> String` builds a `view::Page`, sharing one `Accounts` and one `histories` across the tabs in `TABS` order. It calls `app_page` with:
  - tabs from `TABS`, with the short labels and icons (static SVG strings in `html/mod.rs`, one per tab: Overview, Cash, Credit, Savings, Planning, Funds, Retirement)
  - `open: 0` and empty `panels`
  - `data: script_json(serde_json::to_string(&page))`
  - `style: tokens_css() + include_str!("../report.css")`
  - `script: include_str!("../report.js")`
  - the stamp as today
- `report.js` reads `JSON.parse(d.textContent)` and renders every `TabView` into `#{id}-panel`:
  - A cell is a `<div>`. A `Toned` cell takes its class, or `style.color` when the tone starts with `#`. An `Account` cell is the name with its `a-*` class.
  - A row is `<div class="row {table class} {k}">`, with `--d` set from `d` and a `.meter` child when `m` is set.
  - A goal name cell becomes `<button class="goal">`, which opens `<dialog id="hist">` filled from `histories[h]`.
  - `Ledger` renders a labelled `<select>` and redraws its rows on change.
  - `Chart` inserts `svg` with `innerHTML`. Every other text goes through `textContent`.
- `report.css` uses classes `ov sv lg pl fs fh rs rm` and implements the approved shapes:
  - **Below 48rem:**
    - `sv`: the name and `%` on the first line; on the second line the current figure, the goal figure with `::before{content:"of "}` and the date with `::before{content:"Due "}` when it isn't empty; then the meter.
    - `lg`: `date · account` on top in secondary text, the description and amount below.
    - `ov`: the name on line 1, the three figures on line 2.
    - `pl`, `fs`, `rs`: one line each. `fh` and `rm`: two lines.
  - **Above 48rem:** every class lays out as one line in its columns, and Savings containers sit in two grid columns.
  - **`(pointer:coarse)`:** rows and buttons have a `min-height:44px`. Touch rows are 48px in the two-line forms.
  - The history `<dialog>` is a bottom sheet below 48rem (up to 85dvh tall, with a sticky header and a 44px close button) and a centered 34rem modal above. Tapping the backdrop closes it. `prefers-reduced-motion` disables the slide.
  - Favorites take a tinted background through a token, `.expired` cells take `::after{content:" !"}` and `.counted` cells take `" •"`, as today.

- [ ] **Step 1: Write the failing page-level tests** in `html/mod.rs`:
  - `the_page_makes_no_external_request`
  - `every_tab_has_a_panel_in_order`
  - `the_payload_parses_back_to_the_view`: extract the JSON block, unescape it, parse it, and compare with `serde_json::to_value(&page_view)`.
  - `owner_text_cannot_end_the_payload`: a goal named `a</script><b>&"` gives exactly one `</script>` after the JSON block opens, from the block's own closer, and the parsed name equals the input.
  - In `mod.rs`: `minification_keeps_the_payload_and_the_renderer` (run `jluszcz_finance_utils::report::minify` and repeat the parse check).
- [ ] **Step 2:** Run `cargo test --lib report` and check that the new tests FAIL.
- [ ] **Step 3:** Write `report.js`, `report.css` and the new `page()`. Delete the old markup, the old tests and the fixture HTML helpers.
- [ ] **Step 4:** Run the full gate: `cargo fmt --check`, both clippy runs, `cargo test`, and `MM_REQUIRE_WORKBOOK=1 … cargo test --features import`. The owner's environment values come from their shell, so ask if they are unset. Expect a PASS.
- [ ] **Step 5: Render check.** Write the fixture page with a throwaway `#[test] #[ignore] fn write_fixture_page()` that writes `long_names_snapshot()` through `report::write` to `$MM_FIXTURE_DIR`. Then run `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless --screenshot=<scratch>/{w}.png --window-size={w},1600 file://…` at widths 375, 820 and 1280, plus 375 with `--force-dark-mode`. Expect:
  - no horizontal scrollbar at any width
  - two-line Savings and ledger rows at 375, one-line rows at 1280
  - the bottom bar at 375, top tabs at 1280
  - legible dark colors

  Fix what you see, then check once more.
- [ ] **Step 6: Device check.** Write the same fixture page to `ReportPreview.html` in the report directory (invented data only). Ask the owner to open it on the iPhone and iPad and report back, and fix what they report. Then the owner runs `cargo run --bin mm -- report` on the real database, and you compare `Written::bytes` against the current page's size.
- [ ] **Step 7:** Rewrite `src/report/AGENTS.md` around the new facts:
  - script plus JSON
  - Rust decides, JS lays out
  - the `<` rule
  - the breakpoint and pointer split
  - the row shapes
  - the palette tokens and the single hex exception
  - `HISTORY_MONTHS`, the quit gate, the rename and minify-on-write, all unchanged

  Update `AGENTS.md` to match. Delete `ScriptTest.html` from the report directory, and `ReportPreview.html` once the owner is happy with it.
- [ ] **Step 8:** Confirm that the Task 1 PR is merged, remove the local `[patch]`, run `cargo update -p jluszcz_finance_utils`, and re-run the full gate. Commit `feat(report)!: rewrite the HTML report as a responsive, script-drawn page`, with `Cargo.lock`. Then push and open a PR.

---

### Task 7: Paychecker — move onto the shell

**Files:**
- Modify: `../Paychecker/src/report/html.rs` (`page`, `tab_bar` and `tabs`; the module doc's no-script paragraph), `../Paychecker/Cargo.lock`

**Interfaces:**
- Consumes: Task 1 `App`, `app_page` and `NavTab`. Tabs have no icon, so they get the wrapping top row.
- `page` passes `tabs` (years, newest first, with today's `tab_id` ids), `open` from `opening_year`, `panels` as today's `grid(s)` strings, and `style: STYLE`, with no `data` and no app `script`. An empty snapshot still shows `No paychecks yet.` as one panel-less body. Pass a single tab `paychecks` labelled `Paychecks` whose panel is that sentence.

- [ ] **Step 1:** Update Paychecker's page tests to the new markup: `data-t="y2026"`, `id="y2026-panel"`, and `hidden` on the panels that aren't open. Keep their names wherever the fact still holds. Run `cargo test` and check that they FAIL.
- [ ] **Step 2:** Run `cargo update -p jluszcz_finance_utils` (Task 1 merged) and rewrite `page`.
- [ ] **Step 3:** Run `cargo test`, `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings`, and expect a PASS. Render it in headless Chrome at 375 and 1280, and check for no sideways scroll and that the year buttons wrap.
- [ ] **Step 4:** Commit `feat(report): move the report onto the shared shell`, then push and open a PR.

---

### Task 8: finance-utils — remove the radio helpers

**Files:**
- Modify: `../finance-utils/src/report/html.rs` (delete `Tab`, `tab_inputs`, `tab_nav`, `tab_rules` and the old `page`, with their tests), `../finance-utils/AGENTS.md`

- [ ] **Step 1:** Run `grep -rn "tab_inputs\|tab_nav\|tab_rules\|html::page\b\|html::Tab\b" ../MisterManager/src ../Paychecker/src` and expect no output. Tasks 6 and 7 must be merged first.
- [ ] **Step 2:** Delete the items and their tests, then run `cargo test --all-features` and `cargo clippy --all-targets --all-features -- -D warnings`. Expect a PASS.
- [ ] **Step 3:** Commit `refactor(report)!: drop the radio-driven tab helpers`, then push and open a PR.
