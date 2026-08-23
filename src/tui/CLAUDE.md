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
| `←` / `→` | move the caret in a text field, step a date a day at a time, or cycle the focused selector — see the invariant below |
| `Shift`+`←` / `Shift`+`→` | the same nudge, a week at a time on a date; one choice on a selector |
| `Ctrl`+a letter | edit the text under the caret, in every box in the app — see the invariant below |
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

**The chrome is one string for the whole app, and it is drawn against the right edge.**
`help::chrome` states `1-9 screens · q quit` once — no screen has a say in it, because `1-9` and `q`
are answered in `App::dispatch` above every screen handler — and `App::render` splits the footer row
in two, a screen's own keys filling the left and the chrome holding exactly its own width on the
right. The two keys every screen answers are therefore found in the same place whatever the screen in
front of them costs, and the half ratatui truncates when a terminal is narrower than `MIN_WIDTH` is
the screen's own, not `q quit`.

Who *shows* it is a separate question, and the rule is that the chrome appears only where `dispatch`
actually answers those two keys. `Topic::answers_app_wide_keys` states it — the eight screens, and
nothing else — and `App::footer_chrome` asks it through `App::topic`, so one question covers every
modal and all four search boxes rather than a list of screens re-derived at the call site. That
`dispatch` returns into `modal_key` *above* its `q` and `1-9` arms is what makes the answer false
under a modal: a digit typed into a worksheet's `/` box is part of the needle, a `q` under a confirm
dialog is one of the "any key" that cancels it, and naming a key that does nothing is worse than
naming none. The open Help panel is the one context outside that match — it is not a `Topic`, and
`dispatch` returns into `help_key` above everything — so `footer_chrome` asks about it separately. A
status message withholds the chrome for an unrelated reason: it borrows the whole line for
`STATUS_TTL` and gives it back.

**The shared filter keys lead every footer that has them, in one order, under one word each.**
`Tab acct`, `[ ] month`, `Esc clear`, `/ search` — `help::FILTERS` states the order and the four
words, and a table reaches them through `Entry::filter` rather than writing a `Label::Own` of its
own, so a filter over the same thing cannot be called two names by two screens. What a screen still
writes is the `detail`: `Esc` genuinely clears to different places — today's window on the ledgers,
All on Savings and Recurring Goals — and the panel is where that difference belongs, which is why
the shared word is `clear` rather than one screen's answer imposed on the others. Two tests hold it
up: `every_filter_key_is_labelled_with_its_shared_word` over every topic there is, and
`the_shared_filters_lead_every_screen_footer_in_one_order` over the eight with footers.

**A footer must fit `MIN_WIDTH`, and the budget is what decides how a key is labelled.** ratatui
truncates a `Paragraph` from the right, so an over-wide left half runs into the chrome and drops its
own last keys with nothing on screen to say a word went missing.
`help::tests::every_screen_footer_fits_the_minimum_width` measures every screen topic against both
halves plus the separator; `app`'s two own width tests measure what `App::footer` composes at
runtime, which a `Topic` alone does not see. The first lever when a screen runs out of room is
`Label::Shared`: several keys join under one word naming what they act *on* — `E/a/d bill` on
Planning, `a/A/i allocate` and `n/e/c goal` on Savings, `a/t/p money` on the ledgers, where the
three keys that write new rows join against the `e` and `d` that act on the one selected — which
buys back a whole item's separator per key absorbed, and the verbs it costs are a keystroke away in
the panel, which has room for them. A shorter word is the smaller adjustment beside it — `f fave`,
and the `acct` every screen's `Tab` takes. What neither of them does is drop a key: a key nothing
advertises is a key nobody presses, so `Label::Hidden` stays for the entries a footer word would
only say twice, `BackTab` being the one.

**The ledgers' footer is the one closest to the edge**, by some margin, so the next key that needs a
group is likelier to be Cash or Credit's than any other screen's — and both of its levers are now
spent, the grouping on `a/t/p` and the shorter word on `Tab`. A footer that overflows again has
nothing left to fall back on but a shorter word somewhere.

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
smallest: `a`, which creates an account the workbook does not name, and `e`, over the six things
the workbook does not say about one.

`month` is the `[`/`]` filter Savings and Recurring Goals share. `style` is where color is decided —
`ratatui::style::Color` is *decided* there and nowhere else, so no screen grows its own opinion
about what red means. The helpers that carry one of those decisions to a cell — `tui::tinted`,
`form::field_line_tinted` — have to name the type to pass it, and they reach it as `style::Color`,
which `style` re-exports: every mention is then visibly routed through the module that owns the
choice, and the plumbing stays visibly plumbing. `help` is where the screen footers are joined from — one `Topic` per context,
so a footer cannot drift from the panel that explains it; modal border titles are still written
where they are drawn. `cursor` answers the scroll keys for every list at once, `text` is the line
of text under the caret and the keys that edit one — every box in the app is one — and `search` is
the `/` box the ledgers, Savings, the worksheet and the destination chooser share: the box, its
keys, and `Matcher`, which is what a needle *means* on all four. What every screen
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

- **No screen formats a `Cents` itself.** Every figure a screen draws goes through `tui::amount`,
  `whole_amount`, `money_span` or `money_text` — or, where it lands in prose rather than a cell,
  through `demo::figure`/`whole_figure` directly. Two reasons, and the second is why the rule is
  absolute: "negative reads red" is one decision rather than one per screen, and `mm --demo` blocks
  every absolute figure out of the app at those same four functions. A `format!("{cents}")` written
  at a call site is outside both — it draws an uncolored figure, and it publishes a real balance to
  whoever the app is being demonstrated to. `crate::demo` is where the mask lives and what it looks
  like; it sits at the crate root rather than here because `transfer::diagnose` writes the plug's
  figure into the prose this screen draws.
  - **A `Field` holding an amount keeps the real text and is masked on the way out.** The buffer is
    what commits, so `demo::typed` is applied where the field becomes a `Label` — in each form's
    `display(field)` — and never to what `Field::given` was handed. That is what lets a demo be
    driven rather than only watched: what is typed still parses.
  - **A refusal that quotes a field back is a second way out, and `parse_whole_amount`'s is the one
    that had to be masked.** It fires only on text that *already parsed* as money — cents in a
    whole-dollar field — so what it names is a real figure every time, and a form's amount field is
    prefilled from the row it opened on. `parse_amount`'s own error is safe unmasked for the
    opposite reason: it fires only on text no reading of which is a figure.
  - **A one-field `ValueForm` does not know what it is collecting, so its caller says.**
    `ValueForm::money` is the amount and `ValueForm::new` the plain figure, because the Planning
    screen edits a pay-period count and a split percentage through the same modal it edits a target
    through. `planning::Target::is_money` is what picks between them, and it is the other half of
    `Target::write`'s match: a target is money exactly when its arm parses with `parse_amount`.
  - **`search::searchable_amount` is a match key, not a figure, and is never masked.** A needle is
    matched against the amount itself rather than against what the screen drew, so `mm --demo`
    narrows a list by amount exactly as an ordinary run does; masking it would turn every row's key
    into the mask and leave the owner unable to find anything mid-demonstration. The needle stays
    visible for the same reason — it is a query being typed, not a figure off a row.
  - **Percentages, dates, counts and names are never masked.** A percentage is a shape rather than a
    sum — it is what makes the Funds and Planning screens worth demonstrating at all — and a goal's
    name is already the owner's own word for it. The mask is six blocks wide, plus a sign where the
    figure has one, which is exactly the narrowest money column any screen lays out (`$/Pay`, at 7):
    a demo moves no column and truncates nothing.
    - **The allocation form's amount field is the one field whose text may be either**, so it is the
      one `display(field)` that asks before masking. `/12` is a count and stays; a typed figure goes
      through `demo::typed`. `form::is_share` is the question, beside the `parse_share` that answers
      it for real, so the two cannot come to disagree about what a divisor looks like. Masking it
      too would leave that field with no feedback at all — what it divides is on the line below and
      blocked, and `resolved_share` puts the answer beside it blocked — so `/12` and `/2` would read
      the same right up to Enter.
  - **The net is one test per screen and two sweeps, and both sweeps assert something *arrived*.**
    `a_demo_leaves_no_figure_on_any_screen` walks all nine screens with the mask on and asserts none
    of the fixture's own figures reach the buffer; `a_demo_leaves_no_figure_on_any_form_a_row_opens`
    presses every key that opens a form or a worksheet over a row carrying a figure and asserts the
    same of the modal. The second is not redundant: a screen sweep draws only screens, and a form
    prefills from the row it opens on, so a form is where a real figure is *most* likely to reach
    the screen. `BillField::Amount` was the one amount field this feature first missed, and only the
    form sweep sees it.
    - **An absence check over an empty table passes for free**, which is the failure mode both
      sweeps are built against. The screen sweep runs on `app_with_two_rows_on_every_list` rather
      than `app` — which has no funds, no recurring goals and no recurring transactions — and
      asserts the mask *appears* on every screen but Accounts, the one screen that draws no figure.
      The form sweep asserts `modal.is_some()` before it looks at the buffer, because a key that
      finds nothing to open on leaves the screen as it was: `('2', 'r')` needs the account filter
      `r` reconciles against, and screen 6 needs a fixture with a `fund` row under the cursor.
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
- **A date field is `form::DateField`, and that is where every rule about one lives.** The text and
  the reading of it are one value, because a date is the one field whose meaning depends on *when*
  it is being typed — the `M/D` shorthand needs a `today` to resolve against — and a bare `Field`
  beside a free `parse_date` is two halves a form has to keep in step itself. Every date in the app
  is one: both ledger forms, the allocation, goal and close-out forms, the worksheet, both dates on
  a recurring transaction, `t`'s confirmation, and the Funds birth-date prompt through
  `ValueForm`'s `Entry::Date`. A new form asks for a `DateField` rather than assembling one.
- **`←`/`→` step a date a day at a time, wherever there is a date, and `Shift` with them a week.**
  The Overview scrub, the worksheet's date, both dates on a recurring transaction, and the date
  every form and confirm dialog opens on all answer the same keys the same way — the reflex is
  "nudge the date", not "nudge the date on the screens that happen to have wired it". On a form the
  arrows arrive through `FormFields::choice`, which reads **the field under the caret** and is the
  one place all three readings are written: a text field moves the caret a character, a date steps
  a day, a selector cycles. A form says which of the three it has focused, once, in
  `FormFields::focused` — an arrow pressed on the date must never move the account beside it, and
  a form that answered "which kind of field is this?" in two places would answer it differently in
  two places. Four rules hold the date's meaning together:
  - **A field that does not parse as a date is left exactly as typed.** The arrows nudge a date that
    is already there; they do not conjure one. That is what keeps them off a half-typed date, and
    off the two empty fields that mean something in their own right — an undated goal, and a
    recurring transaction with no horizon, which is the paycheck. Seeding either from an arrow press
    would date a goal, or end a rule that does not end, with a keystroke that reads as an adjustment.
  - **A step counts as the user's own**, the same as a keystroke: `DateField::step` writes through
    `Field::retype`, so a date arrived at by pressing an arrow is not a prefill an accepted
    suggestion may overwrite. `Field::fill` is the untouched counterpart, which is what a
    suggestion uses.
  - **The typing never goes away.** A date field stays free text — the arrows are the nudge, not the
    only way in — which is why `-` still types on the worksheet's date focus and why every date is
    still parsed at commit rather than only ever assembled by arrow.
  - **One step type, so a modifier cannot mean a week on one handler and nothing on the next.**
    `form::Step` carries the days and hands a selector `direction()` alone; `tui::WEEK` is the only
    place `7` is written; and `app::week_step` is how the three handlers that answer an arrow
    themselves — the Overview scrub, the worksheet, and `t`'s confirmation — read the modifier.
    A selector steps **one** choice under `Shift` rather than none: it has no week to move, and a
    modified arrow the terminal delivers and the app drops is a dead key with nothing on screen to
    say why.
- **Every text box in the app is a `text::TextBuffer`, and the caret is the buffer's.** A form's
  `form::Field` is that buffer plus "has the user touched this"; a `search::SearchBox` is the same
  buffer plus "is the box open". Both answer one dispatcher, `text::edit_key`, which is to text
  what `cursor::scroll_key` is to a list: `Ctrl`+`W` deletes a word in a form field, in a `/` box
  and on the worksheet's date because it is written once rather than in each of them.
  - **`Ctrl` means editing text and nothing else, anywhere in the app.** `Ctrl`+`A`/`E` to the ends
    of the line, `Ctrl`+`B`/`F` a character, `Ctrl`+`W` the word before the caret, `Ctrl`+`U`/`K`
    back to the start or on to the end, `Ctrl`+`D` (and `Delete`) the character under it. A `Ctrl`
    combination nobody has bound is **dropped rather than typed**: as a bare `KeyCode::Char` it
    would otherwise arrive in the buffer as its own letter, which is what `Ctrl`+`C` used to do.
    That rule is also why the worksheet's operators — `s`, `w`, `z` — are guarded against it: a
    hand reaching for "delete the last word" must not spread the pot.
  - **A modified character stops at `App::dispatch` wherever there is no caret to edit**, which is
    the same rule read from the other end: `text::is_bare` says whether a press is the character it
    appears to be, and `help::Topic::takes_editing_keys` says whether this context has a buffer for
    it. Where it does not — a screen, a confirm dialog, the picker, the destination chooser with
    its box shut — the combination is dropped before any handler sees it. Without that the drop
    inside `edit_key` covers only the boxes, and every screen goes on reading the bare letter: on
    Savings `Ctrl`+`F` is the `f` that writes `goal.favorite`, and `Ctrl`+`D` raises a delete. It is
    the confirm dialogs' second exception to "any key but `y` cancels", beside `?`: a modifier the
    app does not bind is not a keystroke it received.
  - **`Alt` is deliberately unbound, and unbound means dropped.** macOS sends `Option` as `Meta`
    only where the terminal has been told to, so `Alt`+`B` is a word-motion that silently does
    nothing on the machine this app is used from. `Ctrl`+`B`/`F` are the motions instead. `Shift`
    is the one modifier a typed character may carry, since it is how a capital arrives at all.
  - **`text::edit_key` says what it did, and the box acts on that.** `Changed` is what re-asks for a
    form's suggestions and re-narrows a search box's list; `Moved` is a motion, or a kill with
    nothing left to kill, and must do neither — a caret dragged back across a needle leaves every
    row where it was. `Ignored` is a key that was never ours, and the caller decides what it means.
  - **The arrows are not in `edit_key`,** because in a form they belong to the date and the
    selector as well. The caller that knows a text field has the focus is the one that may read
    them as the caret, which is `FormFields::choice` in a form and `search_key` in a box that has
    neither a date nor a selector to share them with.
- **The caret is reverse video over the character it is on, and it is `form::value_spans` that
  puts it there.** A block *over* a character rather than a bar *between* two of them: a bar costs
  a column, so the value shifted right of the caret every time the caret moved through it and the
  field read as though a space had been typed into it. `value_spans` splits the one span the caret
  falls in and patches `form::caret_style()` onto that character, keeping the span's own style
  underneath — so an account keeps its colour with the caret in the middle of it, swapped into the
  background. Every box goes through it: a form's field, both search footers, the destination
  title, and the worksheet's date.
  - **At the end of a line the caret sits on the space past it,** which is the one place it costs a
    column, and where a terminal's own cursor sits too. A **selector** draws it there always: its
    text is the choice rather than a buffer, and so does the worksheet's **amount**, a figure
    digits are pushed onto and rubbed off the end of.
  - **The offset is honoured only where the text on screen *is* the text in the buffer.**
    `form::Caret` carries both, and where they differ the caret goes to the end. That is what keeps
    it out of a **figure `--demo` has blocked**, where a caret among the blocks would count the
    digits back out. Comparing the text rather than its length is what makes it airtight — the mask
    is six characters, so `123.45` is a figure a length check would place the caret inside of.
  - **A search box draws its caret only while it is open.** `SearchBox::caret` is `None` once
    `Enter` has left the filter narrowing the list, because a kept filter takes no keystrokes. The
    ledger title is the one echo that never carries a caret even so: the box itself is in the
    footer there, and two carets on one screen would leave it ambiguous which is taking the typing.
    The destination chooser has no footer, so its title *is* the box and does carry one — which is
    why `Chooser::title` returns a `Line` where every other title is a `String` or a `Label`.
    `App::footer` returns one for the same reason. Being the only place the box can be drawn is
    also why that title shows it **as soon as it opens**, empty: `/` on a title that waited for the
    first character would leave the screen unchanged, and the `Esc` closing the box after it would
    look like a second key that does nothing.

- **A form with autocomplete puts its description ahead of its amount.** Accepting a suggestion
  fills the amount, so an amount sitting *before* the description meant tabbing through a field the
  suggestion was about to write anyway — and tabbing off the description now lands on the figure
  that arrived with it, which is the one worth checking. `TxnField::ORDER` and
  `TransferField::ORDER` are both tab order and render order, so the screen and the hand cannot
  disagree.
- **A form's prefilled fields lead, and it opens on the first field the hand has to fill.** Those
  are two rules, and on `TxnForm` they point at different fields: the account arrives from the
  ledger's own filter and the date from the last row added, so `TxnField::ORDER` runs
  `Account, Date, Description, Amount` — two defaults to scan, then two fields to type — while the
  form opens focused on `Description`, which is `ORDER[2]`. A form that opened on a field it had
  already answered would cost two `Tab`s before the first character, every time. `Shift`+`Tab`
  reaches the defaults on the rounds where one of them is wrong.
  **`TxnForm` is the form both rules hold on, and the other two each keep one.** The rules are
  worth stating anyway — they are what a new form is designed against — but a reader checking one
  against `ORDER` will find the exemptions, so they are named here rather than left to be
  rediscovered as bugs:
  - `RecurringTxnField::ORDER` runs `Description, Amount, Account, Cadence, Anchor, Horizon` and
    opens on `ORDER[0]`, so the second rule holds and the first does not: the prefilled `Account`
    and `Anchor` sit *after* the typed `Amount`. What leads on `TxnForm` are two prefills that
    arrived **answered** — a filter the owner set, a day they last worked on — while this form's
    account is index 0 of a list nothing chose and its anchor is today because a form has to open
    somewhere. Leading with those would put two fields still to be decided ahead of the two that
    say what the rule *is*, which is the cost the first rule exists to avoid rather than a case
    of it.
  - `TransferField::ORDER` opens on `Date`, which is prefilled, so the first rule holds and the
    second does not. `t` and `p` are pressed once or twice a sitting where `a` is pressed in runs,
    and every other field is either prefilled from the kind (`Transfer`, `CC1 Payment`) or a pick
    from a list, so there is no run of typing for an opening `Tab` to be measured against — and
    the date, arriving from `App::entry_date`, is the guess on the form worth seeing. `a` answers
    the same hazard the other way, by naming the date in its confirmation, because there the
    keystroke is charged on every row.
- **`a`, `t` and `p` on a ledger open on the date the last row *added* this session was written
  for.**
  `App::entry_date` is view state beside `adhoc` — `None` until the first add, which is what leaves
  today as the answer for the first row without a second rule saying so, and restarting is what
  returns it there. Entering a statement is a run of rows landing on the same few days, so the day
  the last one was written for is a better guess than today, and moving off today is how the owner
  says which days those are. **Only an add writes it**: `e` is a correction to a row already
  written rather than a statement about the day being worked on, and a fix to something months back
  that dragged the next new row there with it would be worse than no memory at all. One field
  serves both ledgers, because what it records is the day being entered for and a statement and the
  card rows on it are one sitting — and it serves all three keys, because the card payment at the
  bottom of a statement belongs to the same sitting as the rows above it. `App::entry_field` is
  the one place the fallback to today is written, so a form cannot open on a different answer than
  the one beside it.
  - **The confirmation names the date, whatever it is.** The form opens on `Description` rather
    than on the date, so a row can be written without a keystroke ever visiting the date field,
    and the status line is the only place the day it landed on appears — which matters most
    exactly when the prefill is stale and the ledger's own month window is showing somewhere else
    entirely, so the row is not on screen to contradict it. Unconditionally, because a line that
    spoke up only when the date was surprising is a line the eye learns to skip on the rounds it
    says nothing.
  - **The day a form opens on and the day its `M/D` shorthand resolves against are two different
    facts.** `DateField` has carried both since it existed, and they part company the moment a form
    opens on anything but today: `9/10` typed in August is this September, and read off a December
    prefill it would land a year out in silence. So `TxnForm::add` takes an already-built
    `DateField` rather than a day to open on — two adjacent `NaiveDate` parameters would be one
    transposition away from exactly that. `Worksheet::on` takes one for the same reason;
    `Worksheet::new`, which opens on today and has only the one day to be told, builds it.
- **The app reads two modifiers, and each means one thing.** `Shift` is always the same nudge with
  a bigger step. It is on the key that already means "move this", rather than a second letter for
  one action, and it reaches every date rather than only the Overview's — a horizon several paydays
  out is the plausible question on the scrub, and a bill three weeks off is the same question on a
  form. A week is the step that reaches the middle of the fortnightly paycheck cycle in one press
  and the cycle after in two. **Shift and not Ctrl on the arrows**: macOS claims `Ctrl`+arrow for
  its own spaces, and a key the terminal never receives is a key that does nothing with nothing on
  screen to say why.
  `Ctrl` always means **editing the text under the caret**, and never anything else — see the
  editing-keys invariant below. Nothing reads `Alt`.
- **A date is typed as `YYYY-MM-DD` or as the `M/D` shorthand, and the year turns on the month
  alone.** `M/D` takes the next year that month occurs in: typed in August, `9/10` is this
  September and `3/4` is next March. It is deliberately **not** "the next time that date comes
  round" — `8/1` typed in August is the first of this August, a fortnight back, because backdating
  a ledger row a week or two is the commonest thing the shorthand is typed for and a rule that
  always resolved forward could not express it at all. `YYYY-MM-DD` stays the display date:
  `DateField::display` shows the text as typed while the caret is in the field and the date it
  means once focus leaves, computed at render rather than written back, so there is no blur hook
  for the next form to forget and no half-typed date rewritten under the cursor.
  The Funds birth-date prompt is the one field built `DateField::iso_only`, and it needs no `today`
  at all — which is the distinction made visible. Every `M/D` reading is present or future and a
  birth date is decades past, so a shorthand there could only ever be a wrong year that nothing
  refuses.
- **A date field entering something new opens on today, and the ones that do not each say why.**
  `DateField::today` is the default and most fields take it. The exceptions:
  - The three forms that write a ledger row — `a`, `t` and `p` — open on `App::entry_date` through
    `App::entry_field`, which is today until a row has been added this session. The bullet above is
    the whole of why.
  - A new goal's `Goal Date` opens on the first of the next month, through `GoalForm::opening_date`:
    a goal date is a *deadline*, and today is never one.
  - `t`'s confirmation opens two business days out, through `calc::business_day::add(today, 2)`:
    the rows it writes are dated for when the transfers *land* rather than for when the plan was
    read. `business_day::add` skips weekends and deliberately carries no holiday calendar, which
    is the other half of why this date is editable at all.
  - The worksheets `t` queues behind that confirmation open on the date it wrote, through
    `Worksheet::on`: an allocation is that transfer read from the container's side, so the two
    carry one date, and a sheet left on today would credit the goals days before the cash reaches
    them. It is the *confirmed* date rather than the two-business-day default, since the owner may
    step it before committing. `A` and `i` open a worksheet of their own and stay on today, which
    is what they are entering.
  - A recurring transaction's `Horizon` and the birth-date prompt open blank, because blank is a
    supported state in both — a rule that does not end, and a date not on record — and `parse_opt`
    is what reads the first of those back.

  A field **editing** an existing row is not an exception and takes `DateField::given`: it opens on
  that row's own date, which is what editing one means. `given` marks it touched for the same
  reason `Field::given` does — a real date the owner can see and did not ask to change.
- **`←`/`→` on the Overview is the one key that changes another screen.** It moves `App::adhoc`,
  and Planning reads that same date: `Excess (Actual)` is the checking balance at it, and so is
  every figure below. `App::scrub` therefore reloads both screens, and `t` and `p` act on the
  scrubbed plan, since a confirmation or a pin quoting a different day than the rows above it is
  the failure this exists to prevent. Overview marks the scrub on its column header; Planning has no
  header to mark, so `build` puts the date in the `Excess (Actual)` extra column, `*`-suffixed, and
  only when `View::scrubbed_adhoc` is `Some`. `App` decides whether the plan is scrubbed — the
  screen only renders what it is handed.
  **The drift the press reports is a status message**, written by `App::scrub` and expiring with
  every other one. It says what the press did rather than what the screen is, so a scrub left
  standing while its columns are read costs the Overview's keys for four seconds rather than for
  as long as it stands; the `*` is what carries the state after the message has gone. Landing back
  on the baseline reports that instead of a `+0d` drift, since an arrow that undoes a scrub is
  still an arrow that did something.
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
  - **`e` on `Excess (Used)` pins too, and it is the only way to pin a figure nothing computed.**
    `p` freezes the floored actual; the row takes an arbitrary one. That is the sheet's hand-typed
    `Excess (Fixed)` cell, back as a row — the need it answers is the payday whose excess the owner
    knows better than the balance does. It is a `Target` like every other constant on the screen, so
    it goes through the one `Target::write`, which is what keeps both pin keys moving together here
    as well. Whole dollars, refused below zero, and an empty field does not parse — so the row can
    create or replace a pin and never clear one.
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
  - **A plan with nothing to transfer is not an unresolved one**, and does not take that label.
    Every line zero is what a payday whose excess is nothing produces, now that the fixed bills are
    capped by it — there is no goal in the wrong container, nothing for `Enter` to explain, and
    `transfer::plan` refuses for a reason that is not a failure. `transfer::NOTHING_TO_TRANSFER` is
    the named sentence the screen tells the two apart by, the same construction `goal::NO_TAX_RATE`
    uses and for the same reason: two modules have to agree on one string.
  - **One thing reaches a transfer line's own cell, and two more foot the block.** The heading is
    a heading and nothing else: the transfers never total more than the excess, so there is no
    longer anything for it to say. Every cell is silent unless something is wrong, and all three
    figures are a `Δ` in the same column and the same red.
    - **On the line: what the excess cut short of it**, from `Plan::shortfall` — in practice one
      of the two fixed bills on a payday too small to cover both. The figure the line *moves*
      stays plain: it is right, and the gap beside it is what is not. A cut line that leaves as a
      **withdrawal** carries no `Δ` at all — it is one line under a head that repeats its own
      figure — and `report::html::planning` draws it the same way, for the reason
      `transfer::unmet_asks` is one statement rather than two: two sinks disagreeing about the one
      condition the owner reads either of them for is worse than whichever answer they settle on.
      What that line lost is in `Shortfall` below.
    - **`Unmet Asks`**, when `transfer::spread_asks` comes to more than the plug: `calc::fit` is
      about to scale every goal to the same fraction of what it asked, and nothing else on the
      screen says so — the Goals line's own figure is right either way, and each goal's `$/Pay` is
      a column on Savings. `transfer::unmet_asks` is the one statement of when that gap exists,
      because the report draws it too, and it takes **`lines.goals` rather than the transfer row**:
      `transfer::plan` skips a line at zero, so a plug of nothing has no Goals row for a cell to
      hang off, and that is the payday whose goals are worst served.
    - **`Shortfall`**, whenever the excess cut anything at all: `Plan::shortfall`'s whole total,
      and the sum of every per-line cell above it.
    - **Both footers are pushed outside the block's own `match`, and that is the point of them.**
      The payday each exists for is a payday with no transfer row to annotate — a plug of nothing
      for the first, an excess the fixed bills took whole for the second, which is also the payday
      `transfer::plan` finds nothing to move at all. Each is absent rather than zero on an
      ordinary payday, for the reason a silent cell is silent. Which is why `App::planning_view`
      reads `View::spread_ask_total` on its own rather than through that call: asks chained to it
      are zero on exactly the payday `Unmet Asks` was put outside the `match` to reach, and the
      row would be silent there whatever the block did.
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
  - **A `Row::style` is the exception, and it is a different thing rather than the same bug
    tolerated.** The rule above is about a *cell* coloring the column it sits in; a row style is
    the row's base, which every cell then patches over, so it is the one place a color may cover
    padding — and covering the padding is the whole point where what is marked is the row.
    `ledger`'s `future` dim and `savings`'s favorite band are the two, and both reach their style
    through `style` rather than naming a `Color`, so the sweep above still finds nothing.
- **An account is one color everywhere, and the owner picks it.** `account.color` is an
  `AccountColor` — a name in a `TEXT` column with the schema's `CHECK` behind it, the same
  construction as `kind`, `grp` and `interest_policy`, so that reordering a palette array cannot
  repaint a database. `style::palette` is what a name looks like, and `ratatui::style::Color` stays
  named in `style` alone. **Unset is a real state and the common one**: `style::account_color`
  falls back to the shade the id derives, so a freshly imported database is already
  distinguishable and the field is an override rather than a step the owner has to complete. That
  is also what `—` on the selector writes back.
- **An account cannot reach a screen without its color, and that is a property of the types
  rather than a rule to remember.** An account's text is reachable only through
  `account_label::Account::render_with`, which hands the resolved color with it;
  `tui::account_label::account_cell` and `label_line` are this directory's only sinks, and
  `report::html` is the other one in the crate. One layer down, `db::account::AccountName` and
  `AccountCode` have no `Display`, so an account cannot become a `String` on the way to a
  `format!` either.
  - **`Account::named`, `Account::coded` and `Account::labelled` are one per caller.** The
    ledgers' rows, Savings, Overview and Accounts show a name; Recurring Transactions and the
    ledger *title* show a code, the one because its other columns pin the row down already and
    the other because a title is a chain of filter terms; the three form selectors show
    `CHK — Everyday` as one segment, because both halves name the same account and splitting
    them would leave the code reading as chrome in front of a colored name. An account still
    named after its own code — which is every account an import has just written — shows that
    text once rather than joined to itself.
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
      an account through that mechanism (below) rather than through `account_label::Account`: `plan` and
      `wiring` already have the account row in hand, and `Account` would only look it up again
      for the same color.
    - **The Accounts screen's `Code` column**, built in `app.rs` and drawn by `accounts.rs` as a
      bare `Cell`. Deliberately plain rather than missed: the next cell along that row is
      `account_cell`, which names the same account in color, so the row already says which
      account it is and tinting the code as well would say it twice.
  - **`as_str` is the escape for text, and it is pinned.** `AccountName::as_str` serves the uses
    that are not displays — a description prefill, a search filter folding case, a form seeding
    its editable field — and `nothing_that_draws_an_account_reads_its_name_as_bare_text` lists
    them with a reason each. It also carries the two displays from the list above that reach
    their text the same way — the destination picker's `Offered.container` and the Accounts
    screen's `Code` column — so the sanctioned list is wider than "not a display" and says so
    entry by entry. A source scan rather than a type, because the property is "nobody reached for
    the escape hatch", which no signature can state — and it is purely textual, so a reflow that
    hid an escape behind a local variable would pass it just the same. **It reads three roots, not
    just this directory**: `src/account_label.rs`, which owns `Account` and whose constructors are
    the one sanctioned place the text is read; `src/tui/`; and `src/report/`, the second sink,
    which would otherwise be free to flatten an account into a `format!`. Entries are keyed by the
    path below `src/`, since `tui/ledger.rs` and `report/html/ledger.rs` are two different files and a
    sanctioned line in one must not excuse the same text in the other. The same test's second
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
  - **The extra cell takes a tone of its own, `Row::extra_tone`, and it outranks the tint the same
    way.** A second field rather than a second reader of `tone`, because the two cells say
    different things about one row: the Goals line's figure is right while the gap beside it is the
    problem, and a single tone over both would paint the amount red for the plan's sake. Only ever
    `Tone::Negative` — the one thing that cell reports rather than states is a gap the money will
    not cover.
- **A section of one band draws no band subtotals.** The Overview stacks accounts in bands
  (`account::Group`) inside sections (`account::Kind`): cash breaks into Checking and Savings,
  credit does not break at all. A single band's subtotal and its section's total are the same
  number under two names, so `Section::breaks_down` suppresses the band row — which is why credit
  shows one `Credit` line and not two. Band order is `crate::overview::BANDS`, fixed, not the order
  accounts come back in: an unplaced account sorts last and taking the scan order would let it
  split its own band in two. A subtotal is set apart by weight alone — every label starts in the
  same column, and the subtotals are the only bold rows. Blank rows separate sections, not bands.
- **The Accounts screen has `a` and `e` and no `d`, and the two it has ask disjoint questions.**
  Deleting an account would orphan every transaction, goal and recurring rule pointing at it, and
  the next import would put a sheet's account straight back — so there is no `d`, and a code typed
  wrongly is corrected by renaming around it rather than by starting over.
  - **`a` asks the code, the kind and the name, and stops.** Those are the two an account cannot be
    given afterwards, plus the one it needs to draw as a row. A new account takes its kind's
    default band, no color, no interest policy, no `Savings` block, and the last place among the
    accounts of its kind — the row `mm import` writes for a code it has just met, which is what
    makes an account the sheet does not name indistinguishable from one it does.
    `account::insert` refuses a code the kind already holds, naming it as the database spells it:
    the schema's `account_code_kind` is the backstop, but a constraint failure names an index
    where the owner needs to be told which code they just retyped. Per kind, because the index is
    — one code naming both a cash account and the card drawn on it is the shape it exists for —
    and case-insensitively, because a code typed here has to meet the same code read off the
    sheet. The form bounds the code at `CODE_WIDTH`, the `Code` column's own width: that column is
    the only place a code is ever drawn, so a code too wide for it would be stored whole and read
    cut, leaving the owner unable to check the string the next import matches the row against.
  - **`e` asks the six things the sheet does *not* say** — the name, the color, the band, the
    position, how an interest posting against it is divided, and which block of the `Savings` sheet
    it is the container for — and none of them is touched by an import after the row's first
    insert. All six are *placements*: an account is created by `a` and placed by `e`, which is why
    the kind decides an edit form's fields and decides nothing on an add form.
  - **The code and the kind are on `a` and deliberately absent from `e`.** They are what
    `account::by_code` matches the next import against, so choosing them is the whole of creating
    an account and editing either would orphan the row from the sheet that produced it. An account
    created here whose code the workbook later grows is therefore *adopted* by the import rather
    than duplicated, since `import::constants` skips a code the kind already holds. `AccountForm`
    carries both modes with `editing: Option<AccountId>`, the shape `FundForm` and
    `RecurringTxnForm` carry, and `edit` leaves the code field blank rather than seeding it from
    the account — a field nobody sees is not worth reading an account's text as a bare string for,
    which is what `nothing_that_draws_an_account_reads_its_name_as_bare_text` would catch.
  - **All but the name and the code are selectors, cycled with `←`/`→` like every other selector in the app.**
    A band the schema's `CHECK` would refuse, a position off the end of its kind, a policy that is
    not a policy, a kind that is not a kind, and an account claiming *both* `Savings` blocks are all
    unrepresentable rather than validated. `Kind::ALL` is what the add form's kind selector offers,
    beside the enum for `InterestPolicy::ALL`'s reason. `Group::bands` is what the band selector
    offers, and it offers exactly what `account::set_group` accepts; the `Savings` selector holds
    one value per account, which is what makes the both-blocks state impossible to type.
  - **The `Savings` selector names the block's contents, not its columns.** `Block::label` is
    `goals` and `buckets`: `Savings!A:E` and `Savings!I:K` are what the *import* tells the two
    blocks apart by, and the owner pointing a container at one is choosing between two sets of
    goals rather than between two spans of a spreadsheet.
  - **The `Savings` field is the one thing on this screen the import reads.** Until both blocks
    have been pointed at a container, `mm import` writes the accounts and stops — the sheet names
    its blocks by position and carries no account code, so there is nowhere else to learn it from.
    Moving an account *off* a block clears that block's key rather than leaving it naming an account
    that no longer answers for it, and a key naming another account is left alone, so editing one
    row never disturbs the other block's mapping.
  - **Which fields an *edit* shows depends on the kind**, the way `FundForm`'s depend on the target.
    Credit does not split into bands, so there is nothing for a band selector to cycle; only a cash
    account holds the goals an interest posting is divided among or a `Savings` block fills, so only
    a cash account is asked about either. A card's edit form is three fields. An add form is three
    whatever the kind says, because none of the five a kind decides is on it.
  - **`Color` is the one of the six every kind is asked, and no kind is asked on `a`.** An account
    that does not exist yet has no id, so there is no derived shade for `—` to stand for and
    nothing for the field to draw itself in; a new account is drawn in its derivation until `e`
    says otherwise. A card is named on the Credit
    ledger and on Recurring Transactions, so it is tinted there like any other account, and what it
    looks like is not a fact about cash. It sits beside the name because the two are one decision:
    what this account looks like wherever it is named.
  - **The `Color` field draws its own value in the color it names**, through
    `form::field_line_tinted` — the one field on any form whose text is a name for something the
    form could not otherwise show, and `Teal` in black is not an answer to "what will this look
    like". Only the value: the label and the caret are the form's chrome. It resolves through
    `style::account_color` with the account's own id, so `—` shows the **derived** shade the row
    will actually take rather than nothing at all — which is the whole reason that choice is worth
    drawing rather than leaving plain. An id is what it needs, which is the other half of why the
    field is `e`'s alone.
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
- **A ledger row may have no description, and `description::render` is what draws one that does
  not.** A cash withdrawal, or a card charge whose merchant is on the receipt and nowhere worth
  retyping, is worth having for its amount alone — so `TxnForm::commit` accepts a blank, and a form
  that refused one would only be worked around by typing a placeholder worse than the blank. It
  draws as `—`, the same mark every other absence in the app draws: an account with no color, a
  fund with no percentage, a recurring transaction with no horizon. Three callers here, and they
  are the three places a description is put in front of a human on a screen — the ledger's
  `Description` column, the status line a write reports on, and the label on the delete
  confirmation. The last two are not cosmetic: a status line reading `added  42.50 on …` has a gap
  between two spaces where a figure looks to have failed to render, and a delete confirmation is
  the one irreversible question on the screen, whose label is the whole of what identifies the row
  being taken away.
  - **It is `crate::description`, not a helper in `tui`, because the report draws the same rows.**
    A screen and a page disagreeing about what a description-less row looks like is what one shared
    rule prevents — the same split `crate::palette` makes for color.
  - **The `/` filter is deliberately not a caller.** `Matcher` matches the stored text, so typing an
    em dash finds nothing rather than sweeping up every unnamed row — the same split as
    `search::searchable_amount`, which matches the figure rather than what the screen drew.
  - **A blank is stored as `""`, never as whitespace.** `commit` trims before it stores, so nothing
    downstream has to ask whether a description is empty or merely looks it. `txn::autocomplete`
    needs no guard of its own for the same reason a suggestion never appears over an empty field:
    `refresh_suggestions` clears the popup when the field is blank, and a `LIKE 'prefix%'` over a
    non-empty prefix cannot match `""`. An unnamed row is therefore never suggested onto a later one.
  - **The transfer and recurring-transaction forms still refuse a blank, and that asymmetry is the
    point.** A transfer writes both its legs from one description, and a pair of unnamed rows in two
    different accounts is the one shape that cannot be read back out of the ledger later; a
    recurring rule's description is copied onto every row it generates and is that rule's only
    identity on screen 8. Both arrive prefilled, so refusing costs nothing that was typed.
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
  unallocated remainder — which is the figure the Savings footer reports and `savings::is_reconciled`
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
- **The Savings list is two blocks, and only the first one is the owner's to arrange.** Undated
  goals lead, in `goal.sort` order; dated goals follow, soonest first. The two halves are arranged by
  different things — a deadline decides a goal's place for it, and a goal with none is placed by
  hand — so `goal::list` and `all_with_balances` state it as one `ORDER BY` and every screen reading
  them gets the same list. Among the dated goals `sort` survives only as a tiebreak between two
  falling on the same day, which is what keeps an arrangement made in the undated block from
  reaching in and reordering a deadline.
  - **`K` and `J` move a goal one place, through `goal::reorder`, which takes a position.** The
    same bargain `account::reorder` makes with a kind: it renumbers the container's undated block
    `0..n-1`, so what the screen shows is what is stored, and "put it third" has a result the
    caller can predict where "set sort to 2" does not. `App::move_goal` computes the position
    against the container's undated goals rather than against the rows on screen — `reorder`
    renumbers that block, and the two have to be counting the same list.
  - **The cursor is put back by id, not by index.** The rows moved under it, so an index would
    leave the selection on whichever goal took the vacated place and the next press would move
    that one instead. `Savings::select_goal` is the one caller's reason for existing.
  - **Two refusals, each with a message.** A dated goal has no manual order to move in, and a kept
    search hides part of the block being reordered — a move would then be one place in a list the
    owner cannot see. Both say so rather than doing nothing quietly, because a key that sometimes
    silently declines is a key nobody trusts. Reaching either end of the block is the third case
    and is a genuine no-op: `Move::applied` returns `None` there, and the block simply has no
    further place.
  - **`K`/`J` join the `goal` run in the footer rather than taking a word.** This footer is one of
    the two closest to `MIN_WIDTH`, and `K/J move` as its own item does not fit; grouped under the
    word naming what the keys act on, they cost four characters instead of eleven. It is the lever
    this document names for exactly this case, and the verbs are a keystroke away in the panel.
- **A favorited goal is drawn as a band and moved nowhere.** `f` on Savings toggles
  `goal.favorite`, and `render` gives that row `style::favorite()` plus a bold. Standing out and
  coming first are different requests, so `refilter` and the `all_with_balances` order never read
  the flag: a marked goal in another container is still filtered out by `Tab`, and a marked goal
  outside the month is still dropped by `[`/`]`.
  - **The band is a background, so it carries its own foreground.** `FAVORITE_BG` alone would be
    the terminal's default text on a fixed band — readable under one theme and invisible under the
    other — so `style::favorite()` is the pair, and it is a `Style` rather than two constants
    precisely so no screen can take half of it.
  - **The band sits at an end of the range, and the light end is the end with room.** Every color
    that lands *on* it — the account palette, the funding ramp, a negative figure — is a mid-tone
    chosen to read against a *terminal's* background rather than against this one, so a band among
    them is a band that hides one. That leaves two places to put it, and they are not equally
    good. Below everything, the tightest neighbour is `NEGATIVE` at luma 77 and the band clears it
    by eight; above everything, the tightest is the ramp's halfway yellow at 172 and the band at
    218 clears it by forty-five. So the band is pale and its foreground near-black, and what a
    future softening has to argue with is that yellow rather than the red. The temptation both
    ends resist is the same: a genuinely mid-tone band, around luma 95 or 140, is what the eye
    asks for and what the `%` column cannot survive — it would erase the ramp on exactly the goals
    that column exists for.
  - **A cast, not a hue.** A flat grey reads as dirt beside the saturated colors around it, so the
    band is cool — but its channels spread 12 where the flattest entry in `palette` spreads 70, and
    the test measures against the palette rather than a number, so what counts as desaturated is
    said by the colors the band could hide rather than by whoever last edited it. Both this and
    the ceiling above are pinned in `style`'s tests, and both bite: a hued band and a mid-tone one
    each fail one of them.
  - **The bold is not decoration; it is what survives the cursor.** `row_highlight_style`'s
    `REVERSED` is patched over the row after its cells draw, and it swaps the band's two halves —
    so a marked row under the cursor would otherwise be indistinguishable from any other cursor
    row. `REVERSED` leaves an independent modifier alone, which is what keeps the mark readable on
    the one row the owner is always sitting on.
  - **The toggle re-reads rather than writing the row in hand.** `App::toggle_favorite` writes
    through `goal::set_favorite` and then calls `reload_savings`: the row is a copy of what the
    query returned, and updating the copy instead is how a screen starts disagreeing with the
    table under it. It costs nothing here — every goal is already loaded for the reconciliation
    line — and the cursor keeps its index, because nothing reorders.
- **A container reconciles when `|excess| < $1.00`**, through `savings::is_reconciled`, called by the
  Savings screen's `Unallocated` footer — the one place the reconciliation is shown. One container
  has sat a few cents out for months; a warning that is always on is a warning nobody reads.
- **The goal form's two `bool`s are selectors, not typed fields.** `Taxed` and `Interest` are
  flipped with `←`/`→` and ignore keystrokes, because a `bool` the owner is spelling out one
  letter at a time can sit at `n`, `no`, or `nope` and mean nothing in between. `Interest` is on
  the form at all because eligibility is a *policy* rather than a fact about the goal: the importer
  sets the opening value from `Planning!J7`'s forced-zero weight, and whether a bucket keeps
  sitting out every interest posting after that is the owner's call — under either policy, since
  `interest_prefill` filters a `manual` container's copied posting to the eligible set before
  rescaling it. `n` opens it eligible, which is what every goal the sheet ever had is. Nothing else
  about a posting is editable here: the split itself is the worksheet's, and this only decides who
  gets weighed in it.
- **`Taxed` is a field of the goal, and the Target field holds the base.** The flag is stored in
  `goal.taxed` and `crate::goal::target` derives the figure from it on every read, so a goal saved
  at `1,000` taxed reopens holding `1,000` with the selector still on — flipping it off is how it
  is untaxed, and nothing can tax a taxed figure twice. What makes the derived figure visible
  before Enter rather than after is the **note beside the Target**: `GoalForm::tax_note` puts
  `(1,065 w/ tax)` past the caret — the same place, and for the same reason, that the allocation
  form shows what a `/N` share comes to. The Recurring Goals form's `Base` field carries the same
  note, so the two forms answer the same question the same way.
  - **The Target field's label stays `Target`, not `Base`.** `Base` would match the other form
    exactly, but this field is the goal's target for every goal that is not taxed, which is most of
    them, and the note is what disambiguates the ones that are.
  - **The rate is read when the form opens, not when it commits.** The note has to be drawable on
    every keystroke. `None` — a database no `Constants` sheet has reached — still opens the form and
    draws no note, and the commit still **refuses a taxed goal** with `goal::NO_TAX_RATE`, even
    though it no longer needs the rate to compute anything: letting it through would write precisely
    the row the read side calls corrupt, and the form is the one place that can ask for the rate
    before there is a goal to be broken by its absence.
  - **The note is empty wherever there is nothing to say**, and never a guess: the flag is off, the
    Target is not a whole figure yet, or no rate is on record. It is drawn through
    `demo::whole_figure`, so `--demo` blocks it like every other absolute figure on a form.
  - **The `Base` field carries the same note the goal form's `Target` does.** `RecurringGoalForm::tax_note`
    puts `(1,065 w/ tax)` past the caret whenever the `Taxed` selector is on and the field holds a whole
    figure, so both forms that edit a base answer the same question in the same words. This form asks
    nothing about the rate — an entry writes no goal, and it is `App::commit_picker`, the picker that
    turns a taxed entry into one, that refuses when no rate is on record.
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
  - **On Savings `Esc` clears the container filter too**, because that screen narrows two ways and
    the key is one way out of either. A screen showing a `Tab` filter and a `[`/`]` filter side by
    side in one title asks the owner to work out *which* of the two is hiding the goal they are
    looking for before they can widen it, and `Esc` clearing only one of them is exactly the
    "one action wearing two letters" the key vocabulary exists to avoid. A **kept** search goes
    first rather than with them: while the box is open `Esc` is the box's, the one
    `search::search_key` answers on every screen that has one, and once `Enter` has left the box
    the needle is still narrowing the list — `search::escape_kept_filter` clears it before these
    two. That is the same order on all four `/` screens, so Savings is not the odd one out either
    way.
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
  window the screen opens on, `Window::containing(today)` — which is why the footer word shared with the
  other two screens is `clear` rather than `all`, a state these two have no way to reach; what `Esc`
  clears *to* is the panel's to say. `[`, `]`, and `Esc` all step the active ledger and then go through
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
  screen from filtering in one place and forgetting to in another — it is a **required** method,
  so a screen that narrowed nothing could not compile.
  The worksheet is the one screen that overrides anything else: `/` is two keys there, so opening
  the box also spends the pending slash that `/N` would have used.
- **`Esc` clears a kept filter before it means anything else**, through `search::escape_kept_filter`
  in each of the four screens' own `Esc` arms. `Enter` leaves the box and keeps the needle, so
  without this the only way back to the whole list is to open the box again and close it the other
  way -- the one route nothing on screen suggests. It is the vocabulary's "innermost thing" read
  literally: the needle first, then the screen's own filter (a ledger's window, Savings' container
  and month together) or, on a modal, the modal itself. A kept filter is therefore two `Esc` presses away from discarding a
  worksheet rather than one. The needle belongs to one screen where a ledger window belongs to
  both, so clearing Cash's filter leaves Credit's alone — `sync_month` is only on the other half of
  the branch.
- **A needle matches a row's text *and* the figures the row is about**, and what that means is
  `search::Matcher` rather than four `contains` calls. It folds the needle once, treats an empty
  one as "match everything", and compares against `search::searchable_amount` — the figure as
  `Cents` prints it with the thousands separators taken back out, so `1234` finds `$1,234.56` and
  the sign survives for `-50` to find a withdrawal. What a screen decides is which figures it
  hands over:
  - Savings offers **Current and Goal**. `%` and `$/Pay` are derived from those two and are
    deliberately withheld — a needle reaching a readout narrows through a column nobody was
    searching — and the goal date has `[`/`]` already.
  - The worksheet offers the amount the line **currently holds**, not its prefill: that column is
    typed into, and a filter quoting a figure the screen has stopped showing is a filter over
    stale rows.
  - A ledger offers the amount **as stored**, so Credit's debt-positive figures match the column
    above them.
  - The destination chooser offers none. What is being chosen is a goal by identity, and the
    amount that will land on it is the waterfall's rather than the goal's.
- **Every `/` screen filters in memory, the Ledger included.** Its rows come out of SQL, but
  `txn::Filter` carries no needle: the window and the account filter bound the fetch before
  anything is typed, so the rows in hand are the only rows a needle could ever reach. Narrowing
  them in `Ledger::refilter` is what lets the rule be `Matcher`'s once instead of restated as a
  `LIKE` clause with nothing holding the two statements together — and a keystroke costs no query.
  This is sound only while the window bounds the fetch; a ledger that ever showed *all* rows would
  want the needle back in the query.
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
  everywhere but its date focus drops all but digits, which is also why its
  table qualifies the shared `EDITING` entry rather than taking it whole:
  the panel may not advertise eight keys that do nothing on the focus the
  worksheet opens on. (`takes_editing_keys` is the wider list beside it, and
  every topic in this one is in that one.) The panel is drawn
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
