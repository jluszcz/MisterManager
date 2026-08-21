# tui — one keyboard across every screen

There is no mouse and no command line: every action in the app is one keystroke, and the owner uses
every screen in a sitting. What that buys is reflexes — the hand that knows `d` deletes the
selected thing does not stop to read the footer — and reflexes are built across screens, not within
one. So the rule the screens are held to runs in one direction: **the same action takes the same
key.** Moving money between accounts is `t` wherever a screen offers it, whether that is one row on
the cash ledger or every row a plan calls for.

The converse does not follow, and holding to it would cost more than it buys. A key is only ever
pressed on one screen, so a letter may mean unrelated things on two of them without ever making the
hand hesitate: `p` pays a card on the ledgers and pins a plan on Planning, and no reflex is caught
between them, because paying and pinning have nothing to do with each other. What breaks a reflex is
the opposite — one action wearing two letters, so that the hand has to remember which screen it is
on before it can act. Reaching for a fresh key because the obvious one is "not quite the same thing"
is the way that happens: the distinction is real to whoever writes the screen and invisible to
whoever presses the key.

## The vocabulary a new screen picks from

| Key | Means |
|---|---|
| `a` / `e` / `d` | add, edit, delete — the selected row, or the thing the screen is about |
| `t` | move money between accounts |
| `p` | pay a card on the ledgers, pin a plan on Planning — unrelated actions, so the letter is free to serve both |
| `P` | unpin a plan on Planning, mark the paycheck on Recurring Txns — likewise |
| `r` | reconcile the ledgers' filtered account against a statement |
| `[` / `]` | step the month, whatever a month filters here |
| `←` / `→` | step a date a day at a time, or cycle the focused selector — see the invariant below |
| `Esc` | back out of the innermost thing: a form, a search box, a filter, the panel |
| `Tab` | cycle the screen's filter, or move to the next field in a form |
| `BackTab` | the same cycle or field order backwards — `Shift`+`Tab` steps back wherever `Tab` steps forward |
| `/` | filter by typing — and, where a screen has a pot to divide, `/N` takes 1/N of it |
| `Enter` | open a list's choice, or the long form of something the screen only had a cell for |
| `1`-`9`, `q`, `?`/`F1` | switch screens, quit, open Help — answered in `App::dispatch`, above every screen handler, so no screen table names them |

A capital is the same verb on a wider or a second object: `A` opens a whole payday where `a`
allocates one goal, `G` regenerates every recurring transaction where `g` regenerates the selected
one, `E` edits the selected bill in full where `e` edits just the figure on its row.

A new screen takes what it needs from that list before inventing a letter, and invents one only for
an action with no relative anywhere else — or when the obvious key is already spoken for on that
screen by something older. Either way the divergence is worth a sentence in that key's
`help::Entry` detail. That is what the Help panel is for: the footer has room for the word, and the
panel has room for the reason.

`Enter` is the one entry above that a screen answers rather than a modal. On Planning it opens the
full account of an unresolved plan — a screen key, not a row key, because that cursor only ever
lands on rows `e` acts on, and making the `unresolved` row selectable would park the cursor
somewhere `e` does nothing with no way to tell that from a key that failed.

## What the footers and the panel share

`help` owns both, so a key cannot be called one thing in a footer and another in the panel. The
app-wide keys are footer chrome rather than table entries — every screen has them, and a copy of
"Switch screens." in every screen's table would crowd out the sentences a reader opened the panel
for. The scroll keys are absent from both, for the same reason and one better:
`cursor::scroll_key` answers them identically on every list in the app.

A status message — a write's result, a parse error, a "nothing selected" — borrows the footer rather
than owning it, and gives it back on its own after `app::STATUS_TTL`. A key press still clears it
sooner, since `on_key` clears before it dispatches; the timeout is for the owner who reads the
message and then does nothing, and would otherwise sit in front of a screen whose keys are missing.
The clock is started in one place, `on_key`, from whatever `dispatch` left behind, so the fifty-odd
sites that write a message do not each have to remember to start it — and it is not started at all
while a modal is open. A message under a modal qualifies the question the modal is asking: the
duplicate rows a payday would land on top of, the field a form refused. No modal repeats it, so one
that faded would leave the question to be answered without it, and under a modal the next key press
is the very thing being waited for. `App::expire_status` is what
drops an expired one, called by the event loop before the draw rather than by `footer`, which only
reads: the message leaves the app rather than being hidden inside the one function that shows it.
Expiry is therefore only as prompt as `tui::TICK`, which is a quarter second and already what a
resize waits for.

## Which module is which screen

`overview` is the first screen; `ledger` backs both the second and the third, Cash and Credit, from
one type. `savings` and `goal_form` are the fourth screen and its forms, the goal
form serving `e` and `n` alike; `worksheet` backs `A`/`i`, and `picker` backs `s` on the seventh
screen. `planning` is the fifth screen, its `Target` and `Editable` enums, its bill form,
`Enter`, which opens the long form of a plan that will not resolve, and
`t`, which confirms a computed plan, writes its payday through `transfer::execute`, and opens the
allocation worksheets prefilled; `destination` is the list `e` opens on one of its destination
rows. `fund` is the sixth screen, its form, and the birth-date prompt the age row needs and no
other screen owns. `recurring_goal` is the seventh screen and `recurring_txn` the
eighth, closing out the app's CRUD coverage. `accounts` is the ninth and the
smallest: one key, `e`, over the six things the workbook does not say about an account.

`month` is the `[`/`]` filter Savings and Recurring Goals share. `style` is where color is decided —
`ratatui::style::Color` is *decided* there and nowhere else, so no screen grows its own opinion
about what red means. The helpers that carry one of those decisions to a cell — `tui::tinted`,
`form::field_line_tinted` — have to name the type to pass it, and they reach it as `style::Color`,
which `style` re-exports: every mention is then visibly routed through the module that owns the
choice, and the plumbing stays visibly plumbing. `help` is where the screen footers are joined from — one `Topic` per context,
so a footer cannot drift from the panel that explains it; modal border titles are still written
where they are drawn. `cursor` answers the scroll keys for every list at once, and `search` is the
`/` box the ledgers, Savings, the worksheet and the destination chooser share. What every screen
shares lives in `mod.rs`, not in whichever screen needed it first.

`modal` is not a screen but the layer over one: the `Modal` enum, which of its variants carry form
fields, which `Topic` each is showing, how each one draws, and what a `Confirm` asks and writes.
`app` keeps the `Option<Modal>` itself and `modal_key`, which says which handler answers a modal's
keys — every arm of it is a call into a handler `app` owns, so that one match stays with them.
Everything else about adding a modal is one file.

The tab bar abbreviates screens seven and eight as `7 Goals` and `8 Txns`. It is a row of shortcuts
rather than a set of headings, and the screens title themselves in full the moment they are opened.

## How wide a screen is

`tui::MIN_WIDTH` is the narrowest terminal the screens are laid out for. Nothing enforces it at
runtime — a narrower terminal still draws, and ratatui truncates whatever no longer fits. What the
number is for is the width tests: each screen renders one row of its widest plausible content at
`MIN_WIDTH` and asserts nothing is cut. Write `MIN_WIDTH`, never the number, so the contract is
stated once and the next retarget is a one-line edit.

Every table gives its fixed columns a `Constraint::Length` sized for their true content — dates at
11, money at 10 to 16 — and hands exactly one column a `Constraint::Min`, which absorbs whatever is
left over. A wider terminal therefore spends all of its extra columns on that one column, and no
screen carries a second layout.

Why those tests are worth their weight: **a right-aligned cell that gets truncated loses its
*leading* characters.** A `Goal Date` column one column short turns `2026-11-27` into a wrong year,
and a money column one short turns a figure into a smaller figure — a wrong number rather than a
visible ellipsis. That is also why widening one column is paid for out of another on the same
screen rather than out of the terminal, and why a test that pins a cell by absolute position must
derive it from `MIN_WIDTH` rather than write the offset out.

## Invariants worth knowing before editing a screen

- **Every editable Planning constant is a `Target` variant**, which owns both its `Key<T>` and how
  its text parses — the same construction as `gate::Gate`. Never write a Planning key at a call
  site. A row's `Editable` says which of the two kinds of edit `e` opens on it — a constant into a
  field, a destination into a list of goals — and the cursor skips every row carrying neither.
- **Some destination rows are read-only, and for reasons that differ.** The plug has no key to
  point anywhere. Retirement and Investment hold an *account* id, and unset there means the
  money leaves the tracked system, which is how they are meant to stand. And Roth and Emergency Fund
  borrow `gate::Gate`'s key — deliberately, so a line and its gate cannot name two different goals —
  which means writing it is never only a destination change: `plan::compute_from_db` reads the same
  key as that gate's remaining shortfall, so a pick there would decide whether the gate fires and
  re-route four other lines' amounts. The Gates block is where that belongs. A destination row must
  not become a second, quieter door to it.
- **`←`/`→` step a date a day at a time, wherever there is a date.** The Overview scrub, the
  worksheet's date, both dates on a recurring transaction, and the date every form and confirm
  dialog opens on all answer the same two keys the same way — the reflex is "nudge the date", not
  "nudge the date on the screens that happen to have wired it". On a form the arrows arrive through
  `FormFields::next_choice`/`previous_choice`, which is also what cycles a selector, so each form's
  handler is a match on its focus: the field under the caret decides, and an arrow pressed on the
  date must never move the account beside it. Three rules hold the meaning together:
  - **A field that does not parse as a date is left exactly as typed.** The arrows nudge a date that
    is already there; they do not conjure one. That is what keeps them off a half-typed date, and
    off the two empty fields that mean something in their own right — an undated goal, and a
    recurring transaction with no horizon, which is the paycheck. Seeding either from an arrow press
    would date a goal, or end a rule that does not end, with a keystroke that reads as an adjustment.
  - **A step counts as the user's own**, the same as a keystroke: `Field::step_date` marks the field
    touched, so a date arrived at by pressing an arrow is not a prefill an accepted suggestion may
    overwrite.
  - **The typing never goes away.** A date field stays free text — the arrows are the nudge, not the
    only way in — which is why `-` still types on the worksheet's date focus and why every date is
    still parsed at commit rather than only ever assembled by arrow.
  `ValueForm` is the one form that has to be *told*: it is one labelled field over text its caller
  parses, so `ValueForm::date` is what marks the Funds screen's birth-date prompt as a date and
  `ValueForm::new` leaves a figure that happens to read as one alone.
- **A form with autocomplete puts its description ahead of its amount.** Accepting a suggestion
  fills the amount, so an amount sitting *before* the description meant tabbing through a field the
  suggestion was about to write anyway — and tabbing off the description now lands on the figure
  that arrived with it, which is the one worth checking. `TxnField::ORDER` and
  `TransferField::ORDER` are both tab order and render order, so the screen and the hand cannot
  disagree; `RecurringTxnField::ORDER` already opens on its description.
- **`Shift+←`/`Shift+→` on the Overview scrub a week, and that is the only modified arrow in the
  app.** The plain arrows step this date a day like every other date; Shift is the same nudge with
  a bigger step, on the key that already means "move this date", rather than a second letter for
  one action.
  It earns the modifier because this is the one date that is *scrubbed* rather than typed — a
  horizon several paydays out is a plausible question here and nowhere else — and a week is the
  step that reaches the middle of the fortnightly paycheck cycle in one press. **Shift and not
  Ctrl**: macOS claims `Ctrl`+arrow for its own spaces, and a key the terminal never receives is a
  key that does nothing with nothing on screen to say why.
- **`←`/`→` on the Overview is the one key that changes another screen.** It moves `App::adhoc`,
  and Planning reads that same date: `Excess (Actual)` is the checking balance at it, and so is
  every figure below. `App::scrub` therefore reloads both screens, and `t` and `p` act on the
  scrubbed plan, since a confirmation or a pin quoting a different day than the rows above it is
  the failure this exists to prevent. Overview marks the scrub on its column header and in the
  footer drift; Planning has no header to mark, so `build` puts the date in the `Excess (Actual)`
  extra column, `*`-suffixed, and only when `View::scrubbed_adhoc` is `Some`. `App` decides
  whether the plan is scrubbed — the screen only renders what it is handed.
- **`p` always pins, and `P` is the only way out of a pin.** The pin freezes `excess_used` so the
  waterfall holds still while a payday's legs are entered — transfers land *before* the ad-hoc
  date, so each leg entered collapses `Excess (Actual)` with the rest still to go, which is the
  whole reason the pin exists (see `src/calc/CLAUDE.md`). Two things follow.
  - **`p` is not a toggle.** The press after a forgotten pin is the *next* payday's, and a `p`
    answering it with "unpinned" makes the press that matters the second one every time. Worse,
    mid-payday a toggle press does the actively wrong thing — it clears the pin protecting the
    figure being worked against. Re-pinning is the only thing a second press can sensibly mean:
    the drift line exists to say a pin has gone stale, and a fresh pin is the answer to that. The
    status line says `re-pinned` rather than `pinned` so a press that replaced a pin does not read
    as one that made the first.
  - **Unpinning is a real operation, not an undo**, and it is why `P` earns a key rather than
    being dropped. It puts the waterfall back on the live balance; without it a pin is permanent
    and `excess_used` runs off a frozen figure that never tracks reality again. The capital is not
    the usual "same verb, wider object" — it is the *inverse* of `p` — and that divergence is
    stated in its `help::Entry` detail, which is what the rule asks for. It is named on the footer
    only while something is pinned, through `footer_without`, though the key is live either way
    and says "nothing pinned" rather than failing silently.
  - **`p` is refused with no live view and `P` is not.** `set_unavailable` leaves `excess_actual`
    holding whatever the last successful view left there, so pinning against it would freeze a
    number belonging to a plan the screen has just said it cannot compute. `P` only clears two
    keys and reads nothing off the view — refusing there would strand a pin behind a footer still
    offering to remove it.
  - **Both keys move together.** `PINNED_EXCESS` and `PINNED_AT` are written and cleared as a
    pair: a date with no amount would render a line about a plan that is not pinned. A re-pin
    therefore advances the date to today, and the drift falls back to the cents the whole-dollar
    floor drops — never to zero, which is what makes a *fresh* pin distinguishable from one that
    happens to sit exactly on the excess.
- **Planning leads with the transfers, not with the sheet's first row.** The rows `t` would write
  head the screen; `Planning!C1:G41` follows underneath, Target and Buffer first. The transfers are
  what the owner acts on, and everything below them is the working that produced them — so a
  trusted plan is read without scrolling, and a doubted figure is chased downwards from the total
  that looked wrong. When the plan does not resolve, `unresolved` and its message take the block's
  place at the top rather than sitting where nothing but a scroll would reach them.
- **The two colors on the Destinations block carry opposite instructions.** Red
  (`style::Tone::Negative`, through `Landing::breaks_the_plan`) means this plan will not run: an
  ambiguous plug, a plug with nowhere to spread, a key naming a row that is gone. Amber
  (`style::Tone::Warning`) means a gap with something on offer to fill it, which breaks nothing —
  the money leaves the tracked system, which is how Retirement and Investment are meant to stand. An
  unset line with nothing to suggest is drawn plain, because a warning that is always on is a
  warning nobody reads.
- **A tinted cell colors its characters, never its padding and never its indent.** `tui::tinted`
  is the one place that happens; `account_cell`, `money_cell`, `savings::percent`,
  `fund::tinted_percent`, Planning's three columns and `form::field_line_tinted` all go through it
  or its rule. **A `Cell::style` carrying an `fg` anywhere under `src/tui/` is this bug**, and the
  sweep is one line: `grep -rn '.style(Style::default().fg(' src/tui/` should match nothing outside
  `mod.rs` and `style.rs`. A `Cell`'s own style — and a
  `Line`'s — covers the cell's whole area, and a table's `row_highlight_style` is patched over the
  row *after* its cells have drawn, so `Style::patch` leaves each cell's foreground in place and
  `REVERSED` turns it into a background. A colored cell on the cursor row therefore drew as a solid
  block the full width of its column, and two colored columns side by side read as one column of
  the wrong width — which is what Recurring Transactions showed, `Acct` and `Amount` being adjacent
  there with nothing between them. Styling the *spans* leaves the padding to the row: an ordinary
  row is unchanged, since padding has no glyphs to color. The leading indent is split off the first
  span for the same reason — Planning indents its labels to show nesting, and an indent is
  structure rather than content.
  - **The colors these tests read back are at a *column*, and `str::find` answers in bytes.** Every
    screen draws inside a border, and `│` is three bytes and one column, so a byte offset lands two
    columns right of the word asked for — which for a word longer than two characters is still
    inside it, so the mistake passes and the test quietly stops checking what it says. `column_of`
    in `mod.rs` is what the color tests use instead.
- **An account is one color everywhere, and the owner picks it.** `account.color` is an
  `AccountColor` — a name in a `TEXT` column with the schema's `CHECK` behind it, the same
  construction as `kind`, `grp` and `interest_policy`, so that reordering a palette array cannot
  repaint a database. `style::palette` is what a name looks like, and `ratatui::style::Color` stays
  named in `style` alone. **Unset is a real state and the common one**: `style::account_color`
  falls back to the shade the id derives, so a freshly imported database is already
  distinguishable and the field is an override rather than a step the owner has to complete. That
  is also what `—` on the selector writes back.
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
    `Worksheet::title`, the new-goal title and `ValueForm::title` (the Value, Reconcile, Fund
    Value and Birth Date modals) all return one, and `label_line` turns one into spans. `text`
    and `account` only ever append; `prepend` is the one way to put plain text ahead of a label
    built elsewhere, which is what lets `ValueForm::title` keep its "Edit " prefix in front of a
    label that may already carry a colored account segment.
  - **Every `display(field)` returns a `Label`**, including the fields that name no account. One
    shape per form rather than one for the account fields and one for the rest. The Accounts
    screen's `Color` field is the exception and still goes through `field_line_tinted`: its tint
    says what `Teal` looks like, not which account this is.
  - **The status line is deliberately uncolored.** It is transient prose rather than a place a
    reader looks to identify an account.
  - **Four account displays are outside this guarantee, and this is the entire list.** The first
    two are gaps nothing has closed yet; the last two are routes deliberately taken around
    `Account`, where the account is tinted by another mechanism or named in color elsewhere on
    the same row:
    - **The destination picker's `Offered.container`**, in `app.rs`'s `open_destination` (backed
      by `destination.rs`): an uncolored account display in a picker column, a genuine gap rather
      than a justified exemption — no task has put `destination.rs` in scope.
    - **`AllocationForm`'s `container_name: String`**, in `goal_form.rs`, drawn into the
      Allocation modal's body by `unallocated_line`. It reads through `Savings::account_name`,
      which has a second caller too — the Unallocated footer below, transient prose like the
      status line — but the Allocation modal's is a real display, kept out of `Account` because
      converting `AllocationForm` is outside this guarantee's scope.
    - **`transfer::Container`'s `String` name, and `planning::Tint`.** The Planning screen tints
      an account through that mechanism (below) rather than through `label::Account`: `plan` and
      `wiring` already have the account row in hand, and `Account` would only look it up again
      for the same color.
    - **The Accounts screen's `Code` column**, built in `app.rs` and drawn by `accounts.rs` as a
      bare `Cell`. Deliberately plain rather than missed: the next cell along that row is
      `account_cell`, which names the same account in color, so the row already says which
      account it is and tinting the code as well would say it twice.
  - **`as_str` is the escape for text, and it is pinned.** `AccountName::as_str` serves the uses
    that are not displays — a description prefill, a search filter folding case, a form seeding
    its editable field — and `nothing_in_the_screens_reads_an_account_name_as_bare_text` lists
    them with a reason each. It also carries the two displays from the list above that reach
    their text the same way — the destination picker's `Offered.container` and the Accounts
    screen's `Code` column — so the sanctioned list is wider than "not a display" and says so
    entry by entry. A source scan rather than a type, because the property is "nobody reached for
    the escape hatch", which no signature can state — and it is purely textual, so a reflow that
    hid an escape behind a local variable would pass it just the same. The same test's second
    clause pins `Label::plain_text`, the escape for a `Label` rather than an `AccountName`: it
    flattens whatever accounts a label carries and is meant for wording assertions, never a draw.
- **Every Planning row that names an account is tinted by it, and the tone outranks the tint.** A
  `Row` carries **one** `Tint`, which says which of the three cells holds the name — `Column::Label`
  for a transfer, which heads its own account; `Column::Value` for the two account-backed
  destination lines; `Column::Extra` for a goal's container or the plug's. One field rather than
  one per column because a row naming *two* accounts is not a state this screen has, and making
  that unrepresentable is cheaper than checking it. The id and color reach the screen on
  `transfer::Container` (destinations) and on `transfer::Row::Transfer`'s own `color` (transfers):
  `plan` and `wiring` have the account row in hand and the screen does not. Red and amber carry
  *instructions* where a tint only says which account, so `render` reads the tone first. Three
  states carry no tint at all, for the same reason each time — nothing single is named: an
  ambiguous plug spans several containers, a withdrawal leaves the tracked system, and a suggestion
  **displaces** the container so that cell holds a goal's name. The lines *under* a transfer are
  plain too: the account is said once, at the head of the group it heads.
- **A section of one band draws no band subtotals.** The Overview stacks accounts in bands
  (`account::Group`) inside sections (`account::Kind`): cash breaks into Checking and Savings,
  credit does not break at all. A single band's subtotal and its section's total are the same
  number under two names, so `Section::breaks_down` suppresses the band row — which is why credit
  shows one `Credit` line and not two. Band order is `overview::BANDS`, fixed, not the order
  accounts come back in: an unplaced account sorts last and taking the scan order would let it
  split its own band in two. A subtotal is set apart by weight alone — every label starts in the
  same column, and the subtotals are the only bold rows. Blank rows separate sections, not bands.
- **The Accounts screen has no `a` and no `d`, and that is the whole shape of it.** An account
  exists because the workbook names it, so there is nothing to add; deleting one would orphan every
  transaction, goal and recurring rule pointing at it, and the next import would put it straight
  back. What is left is `e` over the six things the sheet does *not* say — the name, the color, the
  band, the position, how an interest posting against it is divided, and which block of the
  `Savings` sheet it is the container for — and none of them is touched by an import after the
  row's first insert. The code and the kind are deliberately not on the form:
  they are what `account::by_code` matches the next import against, so editing either would orphan
  the row from the sheet that produced it.
  - **All but the name are selectors, cycled with `←`/`→` like every other selector in the app.**
    A band the schema's `CHECK` would refuse, a position off the end of its kind, a policy that is
    not a policy, and an account claiming *both* `Savings` blocks are all unrepresentable rather
    than validated. `Group::bands` is what the band selector offers, and it offers exactly what
    `account::set_group` accepts; the `Savings` selector holds one value per account, which is what
    makes the both-blocks state impossible to type.
  - **The `Savings` field is the one thing on this screen the import reads.** Until both blocks
    have been pointed at a container, `mm import` writes the accounts and stops — the sheet names
    its blocks by position and carries no account code, so there is nowhere else to learn it from.
    Moving an account *off* a block clears that block's key rather than leaving it naming an account
    that no longer answers for it, and a key naming another account is left alone, so editing one
    row never disturbs the other block's mapping.
  - **Which fields the form shows depends on the kind**, the way `FundForm`'s depend on the target.
    Credit does not split into bands, so there is nothing for a band selector to cycle; only a cash
    account holds the goals an interest posting is divided among or a `Savings` block fills, so only
    a cash account is asked about either. A card's form is three fields.
  - **`Color` is the one of the six that every kind is asked.** A card is named on the Credit
    ledger and on Recurring Transactions, so it is tinted there like any other account, and what it
    looks like is not a fact about cash. It sits beside the name because the two are one decision:
    what this account looks like wherever it is named.
  - **The `Color` field draws its own value in the color it names**, through
    `form::field_line_tinted` — the one field on any form whose text is a name for something the
    form could not otherwise show, and `Teal` in black is not an answer to "what will this look
    like". Only the value: the label and the caret are the form's chrome. It resolves through
    `style::account_color` with the account's own id, so `—` shows the **derived** shade the row
    will actually take rather than nothing at all — which is the whole reason that choice is worth
    drawing rather than leaving plain.
  - **It opens on the color the account is *being drawn in*, not on what it has stored.** An
    account nobody has picked for opens on `AccountColor::derived(id)`, named, rather than on `—`.
    Opening on `—` put the selector somewhere the row behind the modal visibly contradicted, and
    made the first `→` a jump to the head of the palette instead of a step off the current color —
    and stepping off a color is only a nudge if you start on it. `—` stays in the cycle, one step
    back, so the derivation is recoverable. The cost is that Enter on an untouched form writes the
    derived shade down: nothing on screen changes, because it is the shade already drawn, and what
    it gives up is the stored difference between "not chosen" and "chosen to be exactly what it
    already was" — which no screen shows.
  - **Which variant an id derives is `AccountColor::derived`, in `db::account`, not in `style`.**
    It is a fact about the enum — which of its variants an id lands on — and says nothing about
    what a variant looks like. `style::account_color` is where the two halves meet, and the
    Accounts screen reads the derivation directly, without reaching for a color at all.
  - **`Order` reads one-based and commits zero-based.** It is a place in a list to whoever reads it
    and an index to `account::reorder`, which renumbers the whole kind so that what the screen
    shows is what is stored.
  - **A rename has to reach the screens holding their own account list.** `Ledger` and `Savings`
    each cache a `Vec<Account>`, so `App::reload_accounts` re-sets both — otherwise a name changed
    here would not appear on the ledgers or Savings until a restart. `Ledger::set_accounts` carries
    the `Tab` filter across by **id**: the filter is a position in that list, and this screen can
    reorder, so an index kept across the swap would silently point at another account.
    `App::reload` runs it **first**, ahead of every screen that copies out of that list: a Savings
    row snapshots its container's name at `set_goals` time and a ledger reads its `filter` out of
    the accounts it was last handed, so a reload that refreshed the list last would leave the
    Savings `Acct` column on the old name while the title and the `Unallocated` footer, which
    resolve live, already showed the new one.
- **Only Recurring Transactions' row and a filtered ledger's title show an account code.** Every
  other account display — Overview, both ledgers' rows, Savings, and the worksheet and picker
  titles — shows `account.name`, through `Account::named`. Screen 8's `Acct` column goes through
  `Account::coded` because its other columns already pin a row down exactly, so the code alone is
  enough to say which account it belongs to, and that much detail reads better tight than padded;
  the ledger title goes through `Account::coded` too, for the other reason a code beats a name — a
  title is a chain of filter terms, and the code is the tighter one. Widening a name column is
  still paid for out of another column on the same screen, so the width tests are the guard — see
  *How wide a screen is* above.
- **A right-aligned column takes a right-aligned header**, through `tui::right_header` — one
  decision in `mod.rs` rather than each screen deciding for itself. Left over right, a
  header sits at the far side of its column from every figure in it and reads as a label for the
  column beside it. Which columns are right-aligned is not guessable from the header list, so each
  screen's `the_right_aligned_headers_end_where_their_own_columns_do` measures the drawn header
  against a drawn data row. `Last` on screen 8 is the standing exception: its cells go through that
  screen's own `optional`, which does not right-align, so its header does not either.
- **Some screens drop the cents, all through `Cents::to_whole_dollars`.** Savings, Planning, and
  Funds render whole dollars; the digits are dropped rather than rounded, truncating toward zero, so
  `200.99` and `-200.99` read as the same figure under opposite signs. (A sub-dollar negative
  therefore reads `-0`.) Not `floor_to_dollar`'s direction, which is for *computing* a transfer —
  these screens only render what is already there. All are display only, but they part ways past
  that: on Savings and Planning the `edit` prefill keeps the stored cents *and* the commit path
  accepts it back, so opening a constant and pressing Enter cannot quietly round it. Funds' `e`
  prefill keeps the stored cents too, but its commit path goes through `form::parse_whole_amount`,
  which *refuses* a value carrying cents rather than rounding it — opening a fund whose actual value
  has cents and pressing Enter is a parse error, not a silent round. Savings and Planning each keep
  one footer at full precision — Savings' reconciliation and Planning's pin drift, because sub-dollar
  drift is the only thing those two lines exist to show. Funds has no such footer: nothing on the
  screen reconciles at full precision, so there is nothing for one to show.
- **Every figure a goal carries is typed in whole dollars.** A goal's target, a recurring goal's
  base, and the allocations booked against them all parse through `form::parse_whole_amount`, which
  *refuses* cents rather than flooring them — `1800.5` typed for `1800.50` is a typo, and booking
  $1,800 for it hides the slip in a figure that looks deliberate. The edit prefill stays the stored
  figure with its cents, so a goal imported off a fractional cell shows what it really holds and is
  rounded by hand rather than silently moved the first time its form is opened. The cents a goal
  drifts by therefore come only from interest and rounding, and they collect in the container's
  unallocated remainder — which is the figure the Savings footer reports and `tui::is_reconciled`
  judges. The worksheet is not part of this: its lines are prefilled by `per_paycheck` and
  `pro_rata`, not typed.
- **The worksheet's cursor and its focus are two marks, not one.** `> ` is where the line cursor
  sits and is drawn whatever has focus, since the scroll keys move it from the amount and date
  fields too. The reversed bar is the *focus*, and it belongs to `Focus::Lines` alone: every
  worksheet opens on the amount, and a bar on the first goal there reads as that goal being what
  the next digit edits, which the digits contradict. The marker stays either way — its
  width is reserved on every row, so dropping it would slide all three columns two cells sideways
  as focus moved.
- **The line operators are live from the amount focus, and only the date types them.** The amount
  field takes digits and drops every other character, so gating `Space`, `*`, `-`, `z`, `s`, `w`
  and `/` on `Focus::Lines` made them dead keys on the worksheet as it opens — the footer naming
  them the whole time. The date is a text field where `-` is a character and `s` must not spread
  mid-edit, so it is the one focus that still types them. The bar not being drawn is not a problem
  here: `> ` marks the line an operator lands on under every focus, which is exactly why it
  outlives the bar.
- **A tick means "this posting funds this goal", and it takes three keys to say so.** `z`
  (`zero_untargeted`) clears every *visible* line the selection does not cover, which is what frees
  the pot; `s` then divides the remaining equally over the ticked lines, and `w`
  (`spread_by_weight`) divides it in the proportions they were prefilled with. `z` is the one
  operator that reads as the *complement* of the selection, and deliberately so — the alternative
  is ticking the goals you are not funding, then re-picking the ones you are before you can split.
  All three stop at the filter for the reason `targeted` does: a line off screen is not a line an
  operator may move.
- **`w`'s weights are the amounts the lines *opened* with, not the ones they hold.** `WorksheetLine`
  keeps a `weight` written by `new` and by `set_lines` — the per-paycheck ask on a payday sheet, the
  policy's share on an interest one — so a line `z` has just zeroed still weighs what the prefill
  said it was worth. That is the whole reason `w` earns a key beside `s`: it reproduces the opening
  split over a subset, where `s` divides equally and `/N` divides the pot. `interest_prefill` is
  therefore consulted once, at open, and never re-run from the sheet — the worksheet stays a pure
  function over numbers and never reaches for the policy or the database itself.
- **`/N` is one arithmetic in two places, and `tui::share_of` is that place.** The worksheet
  divides its posted amount across the targeted lines; the allocation form divides the container's
  unallocated remainder. Same pot and same divisor must give the same figure whichever screen it is
  typed on, so both go through the one function, which floors to a whole dollar and refuses a
  non-positive divisor. Where they differ is how the divisor is *read*: the worksheet takes it as
  the keystroke after `/`, which caps it at nine, while the allocation form's amount is a text
  field, so `/12` is reachable and `parse_share` is what tells a fraction from a figure. A `/N`
  amount floors its cents where a *typed* `12.50` is refused, and the two are not in tension: cents
  typed into a whole-dollar field are a typo, cents left over from a division are arithmetic, and
  they stay in the remainder where the footer reports them.
- **The allocation form carries the pot and the key that divides it, because nothing else can.**
  The remainder is on the Savings screen *behind* the modal, and `/N` has no room in the help table
  `Topic::Form` shares with the forms that do not offer it — a `/N` entry there would advertise it
  on the bill, value, goal, close-out and recurring-goal-entry forms. So the form draws its own
  line naming the container and its remainder, at full precision as the Savings footer does, and
  annotates the amount with what a `/N` resolves to. The remainder is snapshotted at open: the
  form writes once and closes, and nothing can move the figure while it is up.
- **A container reconciles when `|excess| < $1.00`**, through `tui::is_reconciled`, called by the
  Savings screen's `Unallocated` footer — the one place the reconciliation is shown. One container
  has sat a few cents out for months; a warning that is always on is a warning nobody reads.
- **The goal form's `Interest` field is the one thing on it that is not typed.** It is a selector,
  flipped with `←`/`→` and ignoring keystrokes, because a `bool` the owner is spelling out one
  letter at a time can sit at `n`, `no`, or `nope` and mean nothing in between. It is on the form
  at all because eligibility is a *policy* rather than a fact about the goal: the importer sets the
  opening value from `Planning!J7`'s forced-zero weight, and whether a bucket keeps sitting out
  every interest posting after that is the owner's call — under either policy, since
  `interest_prefill` filters a `manual` container's copied posting to the eligible set before
  rescaling it. `n` opens it eligible, which is what every goal the sheet ever had is. Nothing else
  about a posting is editable here: the split itself is the worksheet's, and this only decides who
  gets weighed in it.
- **Creating one goal and creating goals from recurring entries are different keys on different
  screens.** `n` on Savings is a goal typed from scratch — `a` there is the allocation the screen is
  mostly used for, so the add takes another letter. `s` on Recurring Goals opens the picker: goals
  created *from* the entries that screen lists. Both land in the container the Savings screen's
  `Tab` names, which is the app's one answer to "which container" — screen 7's entries carry none.
  A goal's container is chosen once and never again, so `n`'s border names the one it is about to
  use, the way `Picker` and `Worksheet` name theirs: under the `Tab` filter's All the screen's own
  title says only `Savings · All`, and `default_container` has quietly picked the first. The form
  carries that account id rather than re-reading it at commit, so the write cannot land somewhere
  the border did not say it would.
  The picker's month filter **preselects rather than narrows**: the entries it ticks are also sorted
  to the top, since a tick alone is easy to miss in a list dozens long, but every entry is still
  listed below them — a filter is a starting point the list can be scrolled out of, not a cage. An
  entry that already has an open goal is left unticked and sinks with the rest, because the annual
  reseed is what the ticks are for. Unticked is not refused — goal names are not unique and a second
  open goal is legitimate, so `Space` still adds it. The ticked group being first is also the order
  the goals are created in, and so the order they land in the container.
- **A created goal is dated for the year ahead, not the next occurrence.** `picker::goal_date`
  starts from `next_occurrence` — the first of the entry's month on or after today — and steps a
  year past it, because creating goals is a reseed rather than a catch-up. Counting from the
  occurrence rather than from the calendar is what puts a month already gone this year two
  calendars out: March, reseeded in August 2026, next occurs in March 2027 and so lands in 2028.
  `Biennial` steps two years instead when `goal::has_goal_dated_in_year` says the entry already has
  this year's round — every two years means the year between is skipped rather than filled. Closed
  goals count there: a round that has been through and been closed out has still been through.
- **Savings and Recurring Goals share one month filter; the ledgers deliberately do not.**
  `tui::month::MonthCycle<M>` is the All ↔ month state machine both screens want, generic over what
  a month *is* because their domains differ: Recurring Goals filters a bare month of the year,
  Savings a `YearMonth` (a `goal_date` carries a year, so December 2026 and December 2027 are
  different filters). Both screens open on **All**, so `[` and `]` *start* the filter and enter at
  today's month; both wrap at the ends; `Esc` returns to All and the next step re-enters at today's
  month rather than a remembered one — no state crosses the All filter. Neither re-queries: both
  screens already hold every row.
  - **A goal with no date belongs to no month**, so any month filter drops it and All is the only
    place it appears. That is what the filter is *for*, not an edge case.
  - **The cycle is the span, not the set of months that have rows.** Recurring Goals steps all
    twelve months; Savings steps every month from its earliest `goal_date` to its latest, empty ones
    included, so stepping never skips. No dated goals leaves the cycle empty, and an empty cycle
    cannot be stepped out of All.
  - **`MonthCycle` stores the selected month, not a position in the cycle.** Savings rebuilds its
    cycle on every reload; an index would silently come to mean a different month. A rebuild that
    lost the selected month falls back to All.
  - **The entry month is clamped into the cycle.** Every goal being dated next year is a filter that
    should still open somewhere real, so `Savings::rebuild_months` clamps today to the nearer end of
    the span. `MonthCycle`'s contract is that `entry` is one of `months`.
  - The `[`/`]` on the Recurring Goal *form's* Month selector is a field, not a filter — it has no
    All to fall out of — so it stays `recurring_goal::wrapped_month`.
- **The ledger title's total is a balance at today, and the window does not bound it.** `Tab` is the
  one key that moves it: All is `txn::balance_at_by_kind`, one account is `txn::balance_at`, both
  quoted at today — which is what makes it the same figure as the Overview's To-Date column, and the
  reason it is not a sum over `Ledger::rows`. `[`, `]` and `/` therefore leave it alone, and the
  future-dated rows on screen are outside it. `Today` in the title is not decoration: `Aug 2026` sits
  two terms to its left, and a bare figure there reads as a total of the month on show. It goes last
  in the chain so the filter terms stay contiguous and the figure sits in one place whether or not a
  search is running. Rendered **as stored**, so Credit's is debt-positive like the column above it —
  the Overview is still the one screen that negates. It is also the app's **one figure carrying a
  `$`**, through `tui::money_span`: a column under a right-aligned `Amount` header is already
  unmistakably money and a `$` on every row is noise, but a lone figure in a title sits in prose
  where nothing else says what it is. The sign goes outside it, `-$42.00`. `App` queries it and hands it over through
  `Ledger::set_total`, the same division of labour `set_rows` has, so the screen holds no `Db`; the
  title is composed in `render` as spans rather than in `Ledger::title`, which stays the filter
  chain, because only a span can carry `style::amount_color`.
- **The reconciliation target sits beside that total, and dies with the process.** `r` asks for the
  balance a statement says the filtered account holds; the border then reads
  `Today $1,160.00 · Target $1,200.00 · Δ -$40.00`, which is the question the owner is answering
  read left to right — what do I have, what should I have, how far off am I. It lives in
  `Ledger::targets`, keyed by `AccountId` rather than by the filter's position, so it survives the
  `Tab` cycle, the month steps and a search; nothing about it reaches the database, because the
  figure is a bank's screen read off while rows are being typed and it stops meaning anything once
  the typing is done. **Only under an account filter**: All quotes the whole kind's balance, which
  no statement names, so `r` there says so in the status line rather than opening a form over a
  figure it cannot check. The delta is `Today − Target` on **both** ledgers — Credit renders as
  stored like every other figure on it — and it is the app's one green figure, through
  `style::delta_color` rather than `amount_color`, which would leave a surplus in the same no-color
  as every other positive number. Zero renders as a bare `-`: a state to see at a glance, where
  `$0.00` is a figure to compare with the two beside it. The form is the same `ValueForm` the
  Planning and Funds prompts use; an empty field clears the target, and `Esc` means what it means
  everywhere else — leave the figure alone. With no target the border is exactly what it always was.
- **Cash and Credit share one month, and it is a window, not a `MonthCycle`.** The ledgers' window
  is a one-or-two month span clamped to the data's range and pushed down into the SQL, so they have
  no All to clear to: "no filter" there would be every transaction ever. `Esc` therefore means the
  window the screen opens on, `Window::containing(today)` — the footer says `Esc today` where the
  other two say `Esc all`. `[`, `]`, and `Esc` all step the active ledger and then go through
  `App::sync_month`, which copies the resulting window onto the other and re-anchors both cursors —
  so `2` and `3` always compare the same weeks. It re-queries both ledgers, because a synced window
  over stale rows shows one month's rows under another's heading. Purely view state: nothing
  persists it, and both ledgers reopen on the window around today. Anything that must reach both
  goes through `App::ledgers_mut`, which iterates the pair rather than naming `cash` and `credit`
  in two lines one of which is later forgotten.
- **The `/` box is one box, and `refilter` is where a screen says what narrowing means.** A screen
  implements `search::Search` by handing over its `SearchBox`; the methods over it and the keys
  `search::search_key` answers are written once, so `Esc` abandons a filter and `Enter` keeps it
  identically everywhere. Every mutation calls the screen's `refilter` hook, which is what stops a
  screen from filtering in one place and forgetting to in another. The Ledger is
  the shape that hook exists for: it filters in **SQL** — the needle rides `Ledger::filter` into
  the query — so its hook is empty and `App::search_key` re-queries once the key is consumed.
  The worksheet is the one screen that overrides anything: `/` is two keys there, so opening the
  box also spends the pending slash that `/N` would have used.
- **The scroll keys are documented nowhere, on purpose.** `↑`/`↓`,
  `PgUp`/`PgDn` and `Home`/`End` reach every `cursor::Scroll` implementor
  through one `cursor::scroll_key` call, so they mean the same thing on every
  list and no footer or Help topic names them. That is a promise rather than an
  observation, and `the_scroll_keys_work_on_every_list_in_the_app` is what holds
  it up — a new list screen that forgets its `scroll_key` call breaks
  undocumented keys, with nothing on screen to say they ever worked. The
  Overview is the one screen where they do nothing, and it holds no list.
- **Every form is drawn by `form::render_fields`, and its height is its lines.** The centered
  `FORM_WIDTH` box, the border and its title, and one row per line are written once; the callers
  only build their lines. Height is `lines + 2`, which is what makes the forms that add a line past
  their fields — the allocation form's pot, the transfer confirmation's date and prompt — a row
  taller without anyone restating the arithmetic. It returns the `Rect` it took, which is what the
  forms with autocomplete hand to `render_popup`. `FundForm`'s variable field list needs nothing
  special: it passes its own `fields()`.
- **Every confirmation is one dialog, and `Modal::Confirm`'s `action` is what tells them apart.**
  `d` and `U` open the same 64×5 box over the same lines — the row as its screen describes it, a
  blank, and the verb. What differs is carried by the `Confirm` variant: the border's question, the
  verb `y` reads as, the write itself, and how a cancel reads. It is a variant rather than a boxed
  closure so the write stays an exhaustive match — a new dialog cannot be added without saying all
  four. The write runs while the modal is still up, so a refusal that reaches the status line
  (`recurring_goal::delete` while a goal still references the entry) leaves the question on screen
  with the reason under it.
- **`?` opens Help; `F1` does too, and is the only way in where `?` is a
  character.** `help::Topic::takes_typed_chars` is that list: the form topics
  and the search boxes. The worksheet is deliberately not one of them —
  everywhere but its date focus drops all but digits. The panel is drawn
  last in `App::render` and its handler runs first in `App::dispatch`, so it
  sits above a modal and swallows every key it does not use; `q` must not quit
  out from under it. Running before the modal check is also what gives the
  confirm dialogs their one exception to "any key but `y` cancels". Adding a key
  to a handler means adding it to its `Topic`'s table, which
  `every_key_a_screen_handler_matches_appears_in_its_table` and
  `every_key_a_modal_handler_matches_appears_in_its_table` enforce.
- **Screen 8 shows the last date a recurring transaction reaches, not the horizon it may reach.**
  `recurring_txn::last_owned_dates` is `MAX(txn.date)` over the rows it owns, beside the `Rows`
  count from the same source — so the column is `—` until the first `g`, and it is the number `x`
  moves. The end date stays editable in the `e` form: a cap the owner sets belongs with the rest of
  the fields, not in a table of what the schedule has actually produced.
- **The Funds screen asks for the birth date, because nothing else has anywhere to.** The bond
  row's target is `(age − 30)` points and the age comes from `setting::key::BIRTH_DATE`, which the
  import writes and no screen owns. Entering the screen with an age row and no birth date on record
  opens a one-field date form writing that same key. `Esc` dismisses it — the target draws as `—`,
  the share rows divide the whole 100%, and the footer says the birth date is unset — and it asks
  again the next time the screen is entered, never once the setting exists. Not an error and not a
  silent zero: a zero target would read as "bonds are not wanted" rather than "we have not been
  told".
- **`e` edits the figure and `E` edits the row**, the bill precedent exactly. Nothing on this
  screen moves money — there is no `t` — and no fund row links to an account, a goal, or a
  transaction: the values are typed or imported, and nothing reconciles them against a balance.
