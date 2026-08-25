//! What every key does, in longer form than a footer has room for -- and no
//! longer than that.
//!
//! A `detail` answers "what does this key do", in the fewest sentences that
//! answer it. "Why is it this key" is a maintainer's question and belongs in
//! `src/tui/CLAUDE.md`, which is where a maintainer looks for it; an owner
//! pressing `?` wants the first answer and has to read past the second to
//! reach it. [`tests::no_panel_entry_runs_longer_than_a_glance`] is what
//! keeps the two apart.
//!
//! **A single-character key is quoted where a `detail` names it** -- `'a'`,
//! `'s'`, `'y'`. Bare, it reads as the word it also is ("opening on the same
//! date a does"), or as a stray letter where it is not a word at all ("are s
//! on screen 7"), and either way the sentence has to be read twice. The
//! multi-character names -- `Tab`, `Esc`, `Enter`, `Shift` -- are already
//! unambiguous and take no quotes.
//!
//! One module owns this for the same reason `style` owns color: so no screen
//! grows its own opinion about what its keys are called. The screen
//! footers are joined from these tables, so a footer cannot drift from the
//! panel that explains it. Modal border titles are still written where they
//! are drawn -- the join covers only the footers.
//!
//! **The scroll keys are deliberately absent.** `↑`/`↓`, `PgUp`/`PgDn` and
//! `Home`/`End` reach every `cursor::Scroll` implementor through one
//! `cursor::scroll_key` call at the top of each handler, so they mean the same
//! thing on every list in the app. Naming them per topic would repeat one fact
//! in every table that owns a list, ahead of the keys the reader opened the panel for.
//! `app::tests::the_scroll_keys_work_on_every_list_in_the_app` is what holds
//! that promise up.
//!
//! **The app-wide keys are footer chrome, not panel rows.** `1-9` and `q` are
//! answered in `App::dispatch` for every screen at once, so no table carries
//! them and no panel repeats them; [`chrome`] states them once for the whole
//! app, and the draw sets that against the footer's right edge while a
//! [`Topic`] fills the left. Their guard is
//! `app::tests::the_app_wide_keys_work_from_every_screen`.

use super::form;
use ratatui::Frame;
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Clear, Paragraph};

/// One key, as the footer says it and as the panel says it.
#[derive(Copy, Clone)]
pub(super) struct Entry {
    /// How the key is printed -- `Tab`, `[ ]`, `←/→`.
    pub(super) key: &'static str,
    /// The footer word, if any, and whether it is this entry's own or shared
    /// with its neighbors.
    pub(super) label: Label,
    /// The sentence. What the footer has no room for.
    pub(super) detail: &'static str,
}

/// How an entry's key joins the footer, if at all.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum Label {
    /// Live, but the footer does not name it.
    Hidden,
    /// The entry's own footer word: joins as `{key} {word}`.
    Own(&'static str),
    /// A word shared with the entries beside it: their keys join with `/`
    /// under one word, as `E/a/d bill`. Adjacency in the table is what groups
    /// them, so grouping cannot silently reorder the footer.
    Shared(&'static str),
}

/// An app-wide key: a footer item with no panel row, and so with no `detail`
/// to be one.
#[derive(Copy, Clone)]
struct Chrome {
    key: &'static str,
    word: &'static str,
}

const SCREEN_KEYS: Chrome = Chrome {
    key: "1-9",
    word: "screens",
};

const QUIT_KEY: Chrome = Chrome {
    key: "q",
    word: "quit",
};

/// A filter key more than one screen offers: the key, and the single word
/// every footer that names it takes.
///
/// The word lives here rather than in each screen's table because a filter
/// over the same thing called two names by two screens is the reflex this
/// app is built to protect -- the same rule as `the same action takes the
/// same key`, one level down, at what the action is *called*. A screen still
/// writes its own `detail`: what `Esc` clears is genuinely different on the
/// ledgers and on Savings, and the panel is where that belongs.
#[derive(Copy, Clone)]
struct Filter {
    key: &'static str,
    word: &'static str,
}

/// The shared filters, in the order they lead a footer: widest scope first,
/// down to the one that reads what you type.
///
/// The order and the four words are stated here and nowhere else -- the
/// constants below are handles into this array rather than four more
/// literals. A screen offering some of these shows them in this order,
/// before the first key it owns alone;
/// `the_shared_filters_lead_every_screen_footer_in_one_order` is what holds
/// that up, and `every_filter_key_is_labelled_with_its_shared_word` is what
/// stops a table naming one of these keys itself.
const FILTERS: [Filter; 4] = [
    Filter {
        key: "Tab",
        word: "acct",
    },
    Filter {
        key: "[ ]",
        word: "month",
    },
    // `clear` rather than `all`: Savings and Recurring Goals do clear to All,
    // but the ledgers have no All to reach -- their window bounds the query
    // itself -- and a word true on two screens of three is how a shared
    // filter starts meaning two things.
    Filter {
        key: "Esc",
        word: "clear",
    },
    Filter {
        key: "/",
        word: "search",
    },
];

/// Cycle what the screen is scoped to.
const ACCOUNT_FILTER: Filter = FILTERS[0];

/// Step the month the screen is scoped to.
const MONTH_FILTER: Filter = FILTERS[1];

/// Back out of whatever is narrowing the screen.
const CLEAR_FILTER: Filter = FILTERS[2];

/// Narrow the rows by typing.
const SEARCH_FILTER: Filter = FILTERS[3];

impl Entry {
    /// One of the shared filters, with the sentence this screen tells about
    /// it. The key and the footer word come from the [`Filter`], which is
    /// what keeps them from being written per-screen.
    const fn filter(filter: Filter, detail: &'static str) -> Entry {
        Entry {
            key: filter.key,
            label: Label::Own(filter.word),
            detail,
        }
    }
}

/// A set of keys that are live together.
///
/// Fewer topics than contexts: the two ledgers differ by one key, the
/// confirm dialogs are one dialog with a label per confirmation, and the
/// field forms split into two key sets between them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Topic {
    Overview,
    Ledger,
    Savings,
    Planning,
    Funds,
    RecurringTxns,
    RecurringGoals,
    Accounts,
    /// `/` on either ledger.
    LedgerSearch,
    /// `/` on the Savings screen.
    SavingsSearch,
    /// `/` on the Recurring Goals screen.
    RecurringGoalsSearch,
    /// `/` then a non-digit inside a worksheet.
    WorksheetSearch,
    /// `/` inside the destination list.
    DestinationSearch,
    Worksheet,
    Picker,
    /// `e` on a Planning destination row: the goals one line could be
    /// pointed at.
    Destination,
    /// `Enter` on Planning: the long form of a failure the screen only had a
    /// table cell for.
    Details,
    /// Every confirm dialog: one dialog with a label per confirmation.
    Confirm,
    /// The field forms with no description field.
    Form,
    /// The three field forms whose description field raises the autocomplete
    /// popup, which changes what `↑`, `↓`, `Enter` and `Esc` do.
    SuggestForm,
    /// `t` on the Planning screen: one field, the transfer date, confirmed
    /// before the payday writes.
    PlanTransfers,
}

const OVERVIEW: [Entry; 2] = [
    Entry {
        key: "←/→",
        label: Label::Own("scrub"),
        detail: "Move the Paycheck-Eve column a day at a time. View state only: nothing is saved, and restarting discards it.",
    },
    Entry {
        key: "Shift+←/→",
        label: Label::Own("week"),
        detail: "The same scrub, a week at a time, as Shift does on every date in the app.",
    },
];

const LEDGER: [Entry; 11] = [
    Entry::filter(ACCOUNT_FILTER, "Cycle the account filter, All included."),
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Cycle the account filter the other way.",
    },
    Entry::filter(
        MONTH_FILTER,
        "Step the month shown. Cash and Credit share one window, so both ledgers move together.",
    ),
    Entry::filter(
        CLEAR_FILTER,
        "Clear a kept search if one is narrowing the rows; otherwise return the account filter to All and the window to the month containing today. Both ledgers share the window, so that half moves them together.",
    ),
    Entry::filter(
        SEARCH_FILTER,
        "Filter rows by description or amount as you type -- 1234 finds a row of $1,234.56. Enter keeps the filter and leaves the box; Esc clears it.",
    ),
    Entry {
        key: "r",
        label: Label::Own("target"),
        detail: "Reconcile the filtered account against a statement: type the balance it should hold, and the border carries it beside today's figure with the difference after it. Needs an account filter. An empty field clears it, Esc leaves it alone, and nothing is saved.",
    },
    Entry {
        key: "a",
        label: Label::Shared("money"),
        detail: "Add a transaction, opening on the account the ledger is filtered to, or the first when the filter is All, and on the date the last row added this session was written for -- today, until a row is added.",
    },
    Entry {
        key: "t",
        label: Label::Shared("money"),
        detail: "Move money between two cash accounts, opening on the same date 'a' does. Cash ledger only.",
    },
    Entry {
        key: "p",
        label: Label::Shared("money"),
        detail: "Pay a credit card from a cash account, writing both sides. Opens on the same date 'a' does.",
    },
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected row.",
    },
    Entry {
        key: "d",
        label: Label::Own("delete"),
        detail: "Delete the selected row. Confirms first, because the write commits immediately.",
    },
];

const SAVINGS: [Entry; 15] = [
    Entry::filter(
        ACCOUNT_FILTER,
        "Cycle the container filter: All, then one entry per account that holds goals.",
    ),
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Cycle the container filter the other way.",
    },
    Entry::filter(
        MONTH_FILTER,
        "Step the goal-date filter, wrapping at either end.",
    ),
    Entry::filter(
        CLEAR_FILTER,
        "Clear a kept search if one is narrowing the list; otherwise clear both filters at once, showing every goal again, undated ones included. The next month step re-enters at today's month rather than the one you left.",
    ),
    Entry::filter(
        SEARCH_FILTER,
        "Filter goals by name, balance or target as you type -- 1234 finds a goal at $1,234.56. The % and $/Pay columns are derived and are not searched. Enter keeps the filter and leaves the box; Esc clears it.",
    ),
    Entry {
        key: "a",
        label: Label::Shared("allocate"),
        detail: "Allocate cash to the selected goal. One row, written as its own batch.",
    },
    Entry {
        key: "A",
        label: Label::Shared("allocate"),
        detail: "Open a payday worksheet for the container, prefilled from per-paycheck. One commit is one batch, so a fumbled payday is one undo. Payday means running it once per container.",
    },
    Entry {
        key: "i",
        label: Label::Shared("allocate"),
        detail: "Open an interest worksheet. The container's interest policy decides the prefill: pro rata, or a rescale of its previous Interest batch.",
    },
    Entry {
        key: "n",
        label: Label::Shared("goal"),
        detail: "Create a goal from scratch -- a name, a target and a date -- in the container Tab names. Goals created from recurring goal entries are 's' on screen 7.",
    },
    Entry {
        key: "e",
        label: Label::Shared("goal"),
        detail: "Edit the selected goal's name, target and date.",
    },
    Entry {
        key: "c",
        label: Label::Shared("goal"),
        detail: "End the selected goal: return its value to unallocated, or move it to another goal in the same container. Crossing containers is refused.",
    },
    Entry {
        key: "K",
        label: Label::Shared("goal"),
        detail: "Move the selected goal up one place in its container's manual order. Only the undated goals are ordered by hand -- a dated goal takes its place from its date. Refused while a search is narrowing the list.",
    },
    Entry {
        key: "J",
        label: Label::Shared("goal"),
        detail: "Move the selected goal down one place, the mirror of 'K'. It stops at the last undated goal.",
    },
    Entry {
        key: "f",
        label: Label::Own("fave"),
        detail: "Mark the selected goal, or take the mark back. A marked goal's row is drawn as a band, and that is the whole of what it does: it does not sort the goal up and it does not survive a filter the goal itself would not.",
    },
    Entry {
        key: "U",
        label: Label::Own("undo"),
        detail: "Undo the most recent batch by insert order. Never an Import batch: that one holds every opening balance in the database.",
    },
];

const PLANNING: [Entry; 8] = [
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected row: a constant is typed into a field, a destination is chosen from a list of goals. The cursor settles on the nearest editable row after every move. Roth and Emergency Fund are read-only among the destinations. A figure typed into Excess (Used) pins it, in whole dollars. The three split percentages are bounded as a set: Goals takes what they leave, so a combination over 100 is refused.",
    },
    Entry {
        key: "E",
        label: Label::Shared("bill"),
        detail: "Edit the selected bill in the monthly block.",
    },
    Entry {
        key: "a",
        label: Label::Shared("bill"),
        detail: "Add a bill. Housing bills and other bills reach different waterfall lines, so the category is part of the form.",
    },
    Entry {
        key: "d",
        label: Label::Shared("bill"),
        detail: "Delete the selected bill, after a confirmation. A dropped bill inflates the excess the waterfall has left, which moves every line below it.",
    },
    Entry {
        key: "t",
        label: Label::Own("transfers"),
        detail: "Confirm the computed plan: writes its payday transfers, then opens the allocation worksheets prefilled.",
    },
    Entry {
        key: "Enter",
        label: Label::Own("why"),
        detail: "Explain in full why the transfers could not be resolved -- more than the screen's one cell has room for. Nothing to open when they resolve.",
    },
    Entry {
        key: "p",
        label: Label::Own("pin"),
        detail: "Freeze Excess (Actual) at its whole-dollar floor, so the waterfall holds still while a payday's legs are entered. Always pins, and replaces a pin already there rather than clearing it; the drift line under the plan is what says a pin has gone stale. This pins the figure it computed, where 'e' on the Excess (Used) row pins whatever is typed there.",
    },
    Entry {
        key: "P",
        label: Label::Own("unpin"),
        detail: "Put the waterfall back on the live balance -- the only way out of a pin, since typing over Excess (Used) replaces one rather than removing it. Named on the footer only while something is pinned, though the key is live either way.",
    },
];

const FUNDS: [Entry; 4] = [
    Entry {
        key: "a",
        label: Label::Own("add"),
        detail: "Add a fund: a name, whether its target tracks your age or takes a share of what age leaves, and the value it holds now.",
    },
    Entry {
        key: "e",
        label: Label::Own("value"),
        detail: "Edit just the figure on the selected row -- the value that fund holds. Whole dollars; cents are refused rather than rounded.",
    },
    Entry {
        key: "E",
        label: Label::Own("edit"),
        detail: "Edit the selected row in full: the same form 'a' adds with.",
    },
    Entry {
        key: "d",
        label: Label::Own("delete"),
        detail: "Delete the selected row, after a confirmation. Nothing here holds money, so no balance moves.",
    },
];

/// Two keys, and no `d`. An account is created here or by the workbook
/// naming it, and deleting one would orphan every transaction, goal and
/// recurring rule pointing at it -- and the next import would put a sheet's
/// account straight back.
const ACCOUNTS: [Entry; 2] = [
    Entry {
        key: "a",
        label: Label::Own("add"),
        detail: "Add an account the workbook does not name: a code, a kind, and what to call it. The code and the kind are asked here and nowhere else -- they are what the next import matches this row against -- and a code the same kind already holds is refused. The account takes its kind's default band, no color, and the last place among its kind.",
    },
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected account: its name, its Overview band, its place among the accounts of its kind, and -- for a cash account -- how an interest posting is divided and which block of the Savings sheet it is the container for, which is what the first mm import waits on. The code and the kind are set by 'a', not here. Nothing here is imported, so all of it survives mm import --replace.",
    },
];

const RECURRING_TXNS: [Entry; 7] = [
    Entry {
        key: "a",
        label: Label::Own("add"),
        detail: "Add a recurring transaction: an account, an amount, a cadence and an anchor date.",
    },
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected one. Does not touch the paycheck flag or the generate-through floor: the form has a field for neither.",
    },
    Entry {
        key: "d",
        label: Label::Own("delete"),
        detail: "Delete the selected one, after a confirmation. Its rows are released rather than cascaded, so no balance moves.",
    },
    Entry {
        key: "g",
        label: Label::Own("regen"),
        detail: "Regenerate the selected one's rows out to the horizon. Idempotent: running it twice produces identical rows, and hand-corrected rows are left alone.",
    },
    Entry {
        key: "G",
        label: Label::Own("all"),
        detail: "Regenerate every recurring transaction.",
    },
    Entry {
        key: "x",
        label: Label::Own("extend"),
        detail: "Push this one's rows further out. Refuses when the end date or the ten-year ceiling already binds, and reports where the rows actually stop.",
    },
    Entry {
        key: "P",
        label: Label::Own("paycheck"),
        detail: "Mark the selected one as the paycheck, clearing the flag from every other. The ad-hoc projection date is the day before the next one.",
    },
];

const RECURRING_GOALS: [Entry; 7] = [
    Entry::filter(
        MONTH_FILTER,
        "Step the month filter. Entries carry a month and no date, so the cycle is the calendar: December wraps to January. The screen opens on All, and the first step enters at this month.",
    ),
    Entry::filter(
        CLEAR_FILTER,
        "Clear a kept search if one is narrowing the list; otherwise return to All. The next month step re-enters at this month rather than the one you left.",
    ),
    Entry::filter(
        SEARCH_FILTER,
        "Filter entries by name or base as you type -- 128 finds an entry at $128.00. The month is [ ]'s already, and the Open tally is not searched. Enter keeps the filter and leaves the box; Esc clears it.",
    ),
    Entry {
        key: "a",
        label: Label::Own("add"),
        detail: "Add a recurring goal entry.",
    },
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected entry.",
    },
    Entry {
        key: "d",
        label: Label::Own("delete"),
        detail: "Delete the selected entry, after a confirmation. Refused while any goal still references it, open or closed.",
    },
    Entry {
        key: "s",
        label: Label::Own("savings"),
        detail: "Open the picker: goals created from these entries, in the container the Savings screen's Tab names. The month filter opens it with those entries ticked and sorted to the top, every other entry still listed below. An entry that already has an open goal is left unticked. Each goal is dated a year past its month's next occurrence; a biennial entry that has had this year's round steps two.",
    },
];

/// The one key string the editing entries share, so the two of them cannot
/// come to advertise different keys.
const EDITING_KEYS: &str = "Ctrl+A/E/B/F/W/U/K/D";

/// What the editing keys do, as a macro rather than a `const` so a table that
/// has to *qualify* it -- the worksheet, where only one focus holds text --
/// can `concat!` a clause onto the end at compile time instead of restating
/// the eight keys in its own words.
macro_rules! editing_detail {
    () => {
        "Edit the text under the caret: 'A' to the start of the line, 'E' to the end, 'B' and 'F' one character back or forward, 'W' deletes the word before the caret, 'U' deletes back to the start, 'K' forward to the end, 'D' the character the caret is on."
    };
}

/// How a date field is typed, for the four entries whose `Enter` parses one.
///
/// A macro for the reason [`editing_detail`] is one: `form::DateField::parse`
/// answers all four, so a copy of the rule per table is a copy free to drift
/// from what the parser does.
macro_rules! date_detail {
    () => {
        " A date is typed as YYYY-MM-DD, or as M/D for the next year that month comes round -- in August, 9/10 is this September and 3/4 is next March."
    };
}

/// The editing keys, which every text box in the app answers.
///
/// One entry rather than eight rows, and one entry shared by every table
/// whose context takes text rather than a copy per table: `text::edit_key` is
/// what answers them, so `Ctrl`+`W` deletes a word in a form, in a search box
/// and on the worksheet's date alike, and a table that said so in its own
/// words could come to say something else.
const EDITING: Entry = Entry {
    key: EDITING_KEYS,
    label: Label::Hidden,
    detail: editing_detail!(),
};

/// [`EDITING`], qualified for the worksheet, which is the one context that
/// takes these keys on some of its focuses and not others.
///
/// Two of its three take digits and drop everything else, so the unqualified
/// entry would promise eight keys that do nothing on the focus the worksheet
/// opens on -- with nothing on screen to say why, which is the failure the
/// panel exists to prevent.
const WORKSHEET_EDITING: Entry = Entry {
    key: EDITING_KEYS,
    label: Label::Hidden,
    detail: concat!(
        editing_detail!(),
        " The date is the one focus here that holds text: the amount takes digits and the line list takes the operators, so these keys do nothing on either."
    ),
};

/// Shared by all four search boxes: the keys are the same, and only the rows
/// underneath and the figures they answer to differ.
const SEARCH: [Entry; 5] = [
    EDITING,
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Leave the box and keep the filter, so the row operators stay usable on the narrowed list.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Clear the filter and leave the box. Enter leaves it and keeps the filter instead, and Esc on the screen behind is then what clears a kept one.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the character before the caret. Every keystroke that changes the needle re-filters; moving the caret does not.",
    },
    Entry {
        key: "F1",
        label: Label::Hidden,
        detail: "Open this panel. A question mark types here instead, because a search may legitimately be for one.",
    },
];

const WORKSHEET: [Entry; 14] = [
    WORKSHEET_EDITING,
    Entry {
        key: "Tab",
        label: Label::Hidden,
        detail: "Move between the amount, the date and the line list.",
    },
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Move back through the same three.",
    },
    Entry {
        key: "←/→",
        label: Label::Hidden,
        detail: "Step the date back or forward a day, while the date has focus. It stays typeable; this is the nudge.",
    },
    Entry {
        key: "Shift+←/→",
        label: Label::Hidden,
        detail: "The same step, a week at a time.",
    },
    Entry {
        key: "Space",
        label: Label::Hidden,
        detail: "Select or deselect the line under the cursor. The selection is what 's' and /N operate on.",
    },
    Entry {
        key: "*",
        label: Label::Hidden,
        detail: "Select every line the current filter shows.",
    },
    Entry {
        key: "-",
        label: Label::Hidden,
        detail: "Clear the selection.",
    },
    Entry {
        key: "z",
        label: Label::Hidden,
        detail: "Zero every visible line the selection does not cover, so what is ticked is what this posting funds. Lines the filter hides keep their amounts.",
    },
    Entry {
        key: "s",
        label: Label::Hidden,
        detail: "Spread what is left equally across the selected lines, adding to what they already hold.",
    },
    Entry {
        key: "w",
        label: Label::Hidden,
        detail: "Spread what is left across the selected lines in the proportions they were prefilled with.",
    },
    Entry {
        key: "/N",
        label: Label::Hidden,
        detail: "With a digit: divide the selected lines by it. With anything else: begin a filter over the line names and the amounts they currently hold.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: concat!(
            "Commit every line as one batch, so a fumbled payday is one undo.",
            date_detail!()
        ),
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Clear a kept filter if one is narrowing the lines; otherwise discard the worksheet. Nothing has been written yet.",
    },
];

const PICKER: [Entry; 3] = [
    Entry {
        key: "Space",
        label: Label::Hidden,
        detail: "Select or deselect the entry under the cursor. The Open? column flags entries that already have an open goal -- a hint, not a refusal.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Create one goal per selected entry, all in one transaction, in the order shown.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Close without creating anything.",
    },
];

const DESTINATION: [Entry; 3] = [
    Entry {
        key: "/",
        label: Label::Hidden,
        detail: "Filter the goals by name as you type. The withdrawal row survives every search, so clearing a line's destination never depends on what the goals are called.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Point this line at the goal under the cursor, storing its id rather than its name. The list opens on the suggested goal when there is one, and otherwise on the goal the line already names.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Clear a kept filter if one is narrowing the list; otherwise close without changing where the line lands.",
    },
];

const DETAILS: [Entry; 1] = [Entry {
    key: "Esc",
    label: Label::Hidden,
    detail: "Close the panel. It explains rather than asks, so there is nothing here to accept.",
}];

const CONFIRM: [Entry; 3] = [
    Entry {
        key: "y",
        label: Label::Hidden,
        detail: "Go ahead. The write commits immediately, so this dialog is the only chance to back out.",
    },
    Entry {
        key: "any",
        label: Label::Hidden,
        detail: "Every other key cancels, except ?/F1, which open this panel instead.",
    },
    Entry {
        key: "?",
        label: Label::Hidden,
        detail: "Open this panel: the one key besides 'y' that does not cancel.",
    },
];

const FORM: [Entry; 9] = [
    EDITING,
    Entry {
        key: "Tab",
        label: Label::Hidden,
        detail: "Move to the next field.",
    },
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Move to the previous field.",
    },
    Entry {
        key: "←/→",
        label: Label::Hidden,
        detail: "The field under the caret decides: a text field moves the caret one character, a date field steps back or forward a day, and a choice field -- a bill's category, a close-out's destination -- cycles. A date stays typeable; the step is the nudge.",
    },
    Entry {
        key: "Shift+←/→",
        label: Label::Hidden,
        detail: "The same arrows, a week at a time on a date. A choice field has no week to move, so it steps one choice as it would unmodified.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the character before the caret in the focused text field. A choice field ignores it.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: concat!(
            "Save. A value that will not parse reports itself in the status line and the form stays open.",
            date_detail!(),
            " The birth-date prompt is the exception and takes YYYY-MM-DD alone: every M/D reading is present or future."
        ),
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Discard everything typed and close the form.",
    },
    Entry {
        key: "F1",
        label: Label::Hidden,
        detail: "Open this panel. A question mark types here instead, because a name may contain one.",
    },
];

const SUGGEST_FORM: [Entry; 10] = [
    EDITING,
    Entry {
        key: "Tab",
        label: Label::Hidden,
        detail: "Move to the next field -- or, while suggestions are on screen, accept the highlighted one.",
    },
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Move to the previous field.",
    },
    Entry {
        key: "↑/↓",
        label: Label::Hidden,
        detail: "Move through the suggestions, while any are on screen. Typing in the description field re-queries them on every keystroke.",
    },
    Entry {
        key: "←/→",
        label: Label::Hidden,
        detail: "Cycle a choice field, such as the account -- or, on a date field, step it back or forward a day. A date stays typeable; this is the nudge.",
    },
    Entry {
        key: "Shift+←/→",
        label: Label::Hidden,
        detail: "The same arrows, a week at a time on a date. A choice field has no week to move, so it steps one choice as it would unmodified.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the last character of the focused text field, re-querying the suggestions: backing a letter out of the description widens them again.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: concat!(
            "Accept the highlighted suggestion if any are on screen, otherwise save the form.",
            date_detail!()
        ),
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Dismiss the suggestions if any are on screen, otherwise discard the form.",
    },
    Entry {
        key: "F1",
        label: Label::Hidden,
        detail: "Open this panel. A question mark types here instead, because a description may contain one.",
    },
];

/// `t` on the Planning screen: `Esc` cancels, `Enter` commits, `Backspace`
/// edits the date. The typed-character catch-all is not named, the same
/// convention as [`Topic::Form`]: `TransferConfirm::type_char` forwards any
/// `char` to the date field, which needs `-` as well as digits, so no single
/// key stands in for it.
const PLAN_TRANSFERS: [Entry; 6] = [
    EDITING,
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Cancel. Nothing has been written yet.",
    },
    Entry {
        key: "←/→",
        label: Label::Hidden,
        detail: "Step the date back or forward a day. It stays typeable; this is the nudge.",
    },
    Entry {
        key: "Shift+←/→",
        label: Label::Hidden,
        detail: "The same step, a week at a time.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: concat!(
            "Parse the date and commit: writes the payday transfers, then opens the allocation worksheets prefilled. A date that will not parse leaves the dialog open.",
            date_detail!()
        ),
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the character before the caret in the date. Typing inserts at the caret rather than replacing the prefill, so retyping the date means clearing it first -- Ctrl+U does that in one press.",
    },
];

impl Topic {
    pub(super) fn keys(self) -> &'static [Entry] {
        match self {
            Topic::Overview => &OVERVIEW,
            Topic::Ledger => &LEDGER,
            Topic::Savings => &SAVINGS,
            Topic::Planning => &PLANNING,
            Topic::Funds => &FUNDS,
            Topic::RecurringTxns => &RECURRING_TXNS,
            Topic::RecurringGoals => &RECURRING_GOALS,
            Topic::Accounts => &ACCOUNTS,
            Topic::LedgerSearch
            | Topic::SavingsSearch
            | Topic::RecurringGoalsSearch
            | Topic::WorksheetSearch
            | Topic::DestinationSearch => &SEARCH,
            Topic::Destination => &DESTINATION,
            Topic::Details => &DETAILS,
            Topic::Worksheet => &WORKSHEET,
            Topic::Picker => &PICKER,
            Topic::Confirm => &CONFIRM,
            Topic::Form => &FORM,
            Topic::SuggestForm => &SUGGEST_FORM,
            Topic::PlanTransfers => &PLAN_TRANSFERS,
        }
    }

    /// What the panel's border calls this context.
    pub(super) fn title(self) -> &'static str {
        match self {
            Topic::Overview => "Overview",
            Topic::Ledger => "Ledger",
            Topic::Savings => "Savings",
            Topic::Planning => "Planning",
            Topic::Funds => "Funds",
            Topic::RecurringTxns => "Recurring Transactions",
            Topic::RecurringGoals => "Recurring Goals",
            Topic::Accounts => "Accounts",
            Topic::LedgerSearch => "Ledger search",
            Topic::SavingsSearch => "Savings search",
            Topic::RecurringGoalsSearch => "Recurring Goals search",
            Topic::WorksheetSearch => "Worksheet search",
            Topic::DestinationSearch => "Destination search",
            Topic::Destination => "Where a line lands",
            Topic::Details => "Why the transfers are unresolved",
            Topic::Worksheet => "Worksheet",
            Topic::Picker => "Recurring goal picker",
            Topic::Confirm => "Confirm",
            Topic::Form => "Form",
            Topic::SuggestForm => "Form with suggestions",
            Topic::PlanTransfers => "Confirm transfers",
        }
    }

    /// Whether `App::dispatch` answers the app-wide keys in this context --
    /// which is the whole of what decides whether [`chrome`] may be shown.
    ///
    /// The eight screens, and nothing else. `dispatch` returns into
    /// `modal_key` above the `q` and `1-9` arms, so every modal makes both
    /// dead: a digit typed into a worksheet's `/` box is part of the needle,
    /// a `q` under a confirm dialog is one of the "any key" that cancels it,
    /// and a form takes both as text. The screen-level search boxes are
    /// the same case one layer up. Naming a key that does nothing is worse
    /// than naming none: `q quit` under an unanswered question reads as a way
    /// out of it.
    ///
    /// An exhaustive match rather than a check of `self.modal` at the call
    /// site, so a topic added for a new modal has to answer this here instead
    /// of inheriting a footer's word for keys it does not answer.
    pub(super) fn answers_app_wide_keys(self) -> bool {
        match self {
            Topic::Overview
            | Topic::Ledger
            | Topic::Savings
            | Topic::Planning
            | Topic::Funds
            | Topic::RecurringTxns
            | Topic::RecurringGoals
            | Topic::Accounts => true,
            Topic::LedgerSearch
            | Topic::SavingsSearch
            | Topic::RecurringGoalsSearch
            | Topic::WorksheetSearch
            | Topic::DestinationSearch
            | Topic::Worksheet
            | Topic::Picker
            | Topic::Destination
            | Topic::Details
            | Topic::Confirm
            | Topic::Form
            | Topic::SuggestForm
            | Topic::PlanTransfers => false,
        }
    }

    /// Whether `?` is a character the reader may mean to type here.
    ///
    /// True only where a printable key reaches a text field. The worksheet is
    /// deliberately false: two of its three focuses consume digits and drop
    /// everything else, and the third is a date, which no one writes a question
    /// mark into. It has more keys to explain than any other context, and
    /// reaching them only by F1 would be the wrong trade. `PlanTransfers` is
    /// false for the same reason as the worksheet's date focus: its one field
    /// is a date, and no date holds a literal `?`.
    pub(super) fn takes_typed_chars(self) -> bool {
        match self {
            Topic::Form
            | Topic::SuggestForm
            | Topic::LedgerSearch
            | Topic::SavingsSearch
            | Topic::RecurringGoalsSearch
            | Topic::WorksheetSearch
            | Topic::DestinationSearch => true,
            Topic::Overview
            | Topic::Ledger
            | Topic::Savings
            | Topic::Planning
            | Topic::Funds
            | Topic::RecurringTxns
            | Topic::RecurringGoals
            | Topic::Accounts
            | Topic::Worksheet
            | Topic::Picker
            | Topic::Destination
            | Topic::Details
            | Topic::Confirm
            | Topic::PlanTransfers => false,
        }
    }

    /// Whether there is text under a caret here, which is what makes the
    /// `Ctrl` editing keys mean anything.
    ///
    /// Wider than [`takes_typed_chars`]: the worksheet drops all but digits
    /// on two of its three focuses and `PlanTransfers` has no field but a
    /// date, yet both hand a key to a buffer, so both answer these. Narrower
    /// than "any modal": a confirm dialog, the picker, the details panel and
    /// the destination chooser with its box shut hold no text at all, and
    /// `App::dispatch` is where a combination they cannot use stops rather
    /// than falling through to the bare letter's operator.
    ///
    /// [`takes_typed_chars`]: Topic::takes_typed_chars
    pub(super) fn takes_editing_keys(self) -> bool {
        match self {
            Topic::Form
            | Topic::SuggestForm
            | Topic::LedgerSearch
            | Topic::SavingsSearch
            | Topic::RecurringGoalsSearch
            | Topic::WorksheetSearch
            | Topic::DestinationSearch
            | Topic::Worksheet
            | Topic::PlanTransfers => true,
            Topic::Overview
            | Topic::Ledger
            | Topic::Savings
            | Topic::Planning
            | Topic::Funds
            | Topic::RecurringTxns
            | Topic::RecurringGoals
            | Topic::Accounts
            | Topic::Picker
            | Topic::Destination
            | Topic::Details
            | Topic::Confirm => false,
        }
    }

    /// The footer line: this context's own labelled entries, joined.
    ///
    /// Not the app-wide keys, which are [`chrome`] and are drawn against the
    /// right edge rather than after the last of these.
    pub(super) fn footer(self) -> String {
        self.footer_without(&[])
    }

    /// The same, minus the entries whose key is listed.
    ///
    /// One caller: the Credit ledger, which shares [`Topic::Ledger`] with Cash
    /// and has no `t`. The panel still shows `t` on Credit, with a detail
    /// saying why it is cash-only -- which is more use than its absence.
    ///
    /// Omitting happens before grouping, so dropping a key from inside a
    /// `Shared` run shrinks the group rather than splitting or dropping it.
    ///
    /// Table entries only: the chrome is not omittable, since every screen
    /// that shows a footer shows all of it.
    pub(super) fn footer_without(self, omit: &[&str]) -> String {
        let live: Vec<Entry> = self
            .keys()
            .iter()
            .filter(|entry| !omit.contains(&entry.key))
            .copied()
            .collect();
        footer_items(&live).join(SEPARATOR)
    }
}

/// The app-wide keys, as the footer's right edge says them.
///
/// One string for the whole app rather than a list per topic: `1-9` and `q`
/// are answered in `App::dispatch` above every screen handler, so every
/// screen offers exactly these and no screen has a say in it. They are drawn
/// against the right edge, which is what makes that true of the widest footer
/// as well as the narrowest -- a screen out of room shortens its own words
/// rather than dropping the two keys every screen has.
///
/// Who *shows* it is still a question, and `Topic::answers_app_wide_keys` is
/// what settles it: only the contexts where `App::dispatch` reaches those two
/// keys at all.
pub(super) fn chrome() -> String {
    [SCREEN_KEYS, QUIT_KEY]
        .iter()
        .map(|chrome| format!("{} {}", chrome.key, chrome.word))
        .collect::<Vec<String>>()
        .join(SEPARATOR)
}

/// What separates one footer item from the next.
const SEPARATOR: &str = " · ";

/// One item per footer word: `Own` entries stand alone, adjacent `Shared`
/// entries of the same word join with `/` under it, and `Hidden` entries
/// contribute nothing.
///
/// Adjacency, not the word, is what groups a `Shared` run -- two runs of the
/// same word with something else between them stay two footer items, so a
/// caller cannot merge entries by reordering the table.
fn footer_items(entries: &[Entry]) -> Vec<String> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        match entries[i].label {
            Label::Hidden => i += 1,
            Label::Own(word) => {
                items.push(format!("{} {word}", entries[i].key));
                i += 1;
            }
            Label::Shared(word) => {
                let start = i;
                while i < entries.len() && entries[i].label == Label::Shared(word) {
                    i += 1;
                }
                let keys: Vec<&str> = entries[start..i].iter().map(|entry| entry.key).collect();
                items.push(format!("{} {word}", keys.join("/")));
            }
        }
    }
    items
}

/// How wide the panel is drawn, or the terminal less a margin when that is
/// narrower.
const WIDTH: u16 = 66;

/// The key column. Wide enough for `Backspace`, the longest key any table
/// names, plus the two spaces that separate it from the detail.
const KEY_COLUMN: usize = 11;

/// The open panel: which topic, and how far down it.
///
/// The extent is written back out of the draw, exactly as the worksheet and the
/// picker take their page height, and for the same reason: only the draw knows
/// how many rows a wrap produced or how many of them fitted.
pub(super) struct Help {
    topic: Topic,
    offset: u16,
    lines: u16,
    page: u16,
}

impl Help {
    /// Opens at the top. The topic is fixed here rather than re-derived each
    /// frame, so the panel cannot change identity under itself.
    pub(super) fn new(topic: Topic) -> Help {
        Help {
            topic,
            offset: 0,
            lines: 0,
            page: 1,
        }
    }

    pub(super) fn topic(&self) -> Topic {
        self.topic
    }

    pub(super) fn offset(&self) -> u16 {
        self.offset
    }

    /// A screenful, for `PageUp` and `PageDown`. At least one line, so a panel
    /// drawn into a terminal too short to fit its own border still moves.
    pub(super) fn page(&self) -> u16 {
        self.page.max(1)
    }

    /// How far the panel may scroll: the lines that do not fit, and no more.
    fn limit(&self) -> u16 {
        self.lines.saturating_sub(self.page())
    }

    pub(super) fn scroll(&mut self, delta: i32) {
        let next = i64::from(self.offset) + i64::from(delta);
        self.offset = next.clamp(0, i64::from(self.limit())) as u16;
    }

    pub(super) fn top(&mut self) {
        self.offset = 0;
    }

    pub(super) fn bottom(&mut self) {
        self.offset = self.limit();
    }

    /// What the last draw produced and fitted. Re-clamps the offset, so a
    /// terminal that grows does not leave the panel scrolled past its end.
    pub(super) fn set_extent(&mut self, lines: u16, page: u16) {
        self.lines = lines;
        self.page = page;
        self.offset = self.offset.min(self.limit());
    }
}

/// One entry becomes one or more lines: the key in a fixed left column, the
/// detail wrapped into what is left.
///
/// Wrapped here rather than by `Paragraph::wrap` because the scroll clamp needs
/// the line count *before* the draw, and a `Paragraph` will not say how many
/// rows it produced.
fn wrap(entries: &[Entry], width: u16) -> Vec<TextLine<'static>> {
    let text_width = (width as usize).saturating_sub(KEY_COLUMN).max(1);
    let mut lines = Vec::new();
    for entry in entries {
        let mut gutter = format!("{:<KEY_COLUMN$}", entry.key);
        for chunk in wrap_words(entry.detail, text_width) {
            lines.push(TextLine::from(format!("{gutter}{chunk}")));
            gutter = " ".repeat(KEY_COLUMN);
        }
    }
    lines
}

/// Greedy word wrap. A word longer than the width gets its own line and
/// overflows rather than being split: every long token in these tables is a key
/// name or an identifier, and half of one reads as a different thing.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match out.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => out.push(word.to_string()),
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Draw the panel over whatever is behind it, returning the wrapped line count
/// and the height that fitted.
///
/// Drawn last of everything, so it sits above an open form rather than under
/// one.
pub(super) fn render(frame: &mut Frame, help: &Help) -> (u16, u16) {
    let full = frame.area();
    let width = WIDTH.min(full.width.saturating_sub(4));
    let lines = wrap(help.topic().keys(), width.saturating_sub(2));
    // Taken before the Vec moves into the Paragraph.
    let count = lines.len() as u16;
    let height = (count + 2).min(full.height.saturating_sub(2));
    let area = form::centered(full, width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).scroll((help.offset(), 0)).block(
            Block::bordered()
                .title(format!("Help · {}", help.topic().title()))
                .title_bottom("↑/↓ scroll · Esc close"),
        ),
        area,
    );
    (count, height.saturating_sub(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::MIN_WIDTH;

    /// What a topic's table alone joins to, before its chrome is appended --
    /// the half of `footer_without` these grouping tests are about.
    fn join_footer(entries: &[Entry]) -> String {
        footer_items(entries).join(SEPARATOR)
    }

    /// Every topic there is. `SCREENS` stays separate because only those eight
    /// join a footer.
    const ALL: [Topic; 21] = [
        Topic::Overview,
        Topic::Ledger,
        Topic::Savings,
        Topic::Planning,
        Topic::Funds,
        Topic::RecurringTxns,
        Topic::RecurringGoals,
        Topic::Accounts,
        Topic::LedgerSearch,
        Topic::SavingsSearch,
        Topic::RecurringGoalsSearch,
        Topic::WorksheetSearch,
        Topic::DestinationSearch,
        Topic::Worksheet,
        Topic::Picker,
        Topic::Destination,
        Topic::Details,
        Topic::Confirm,
        Topic::Form,
        Topic::SuggestForm,
        Topic::PlanTransfers,
    ];

    #[test]
    fn no_topic_anywhere_names_the_same_key_twice() {
        for topic in ALL {
            let mut seen: Vec<&str> = Vec::new();
            for entry in topic.keys() {
                assert!(
                    !seen.contains(&entry.key),
                    "{:?} names {:?} twice",
                    topic,
                    entry.key
                );
                seen.push(entry.key);
            }
        }
    }

    #[test]
    fn every_topic_anywhere_has_keys_a_title_and_details() {
        for topic in ALL {
            assert!(!topic.keys().is_empty(), "{topic:?} has no keys");
            assert!(!topic.title().is_empty(), "{topic:?} has no title");
            for entry in topic.keys() {
                assert!(!entry.detail.is_empty(), "{:?} {:?}", topic, entry.key);
            }
        }
    }

    /// The whole footer line -- a screen's own keys, the gap, and the
    /// app-wide keys against the right edge -- has to fit, or the two halves
    /// meet in the middle and the left one is truncated into the right. The
    /// keys lost are the ones at the end of the screen's own list, since the
    /// chrome holds its own width; the lever when a screen runs out of room
    /// is `Label::Shared`, which puts several keys under one word.
    ///
    /// This measures every screen at once, where `app`'s two width tests
    /// measure the footers `App::footer` composes at runtime.
    #[test]
    fn every_screen_footer_fits_the_minimum_width() {
        for topic in SCREENS {
            let footer = [topic.footer(), chrome()].join(SEPARATOR);
            let width = footer.chars().count();
            assert!(
                width <= MIN_WIDTH as usize,
                "{topic:?} footer is {width} wide: {footer}"
            );
        }
    }

    /// A screen may not name a shared filter key itself. `Entry::filter` is
    /// the only way to label one, so a hand-written `Label::Own` beside `Tab`
    /// or `Esc` is what this catches -- the way one filter starts being
    /// called two things.
    ///
    /// Over every topic, not just the eight with footers: a modal that grew a
    /// labelled `Esc` would be the same drift arriving by another door.
    /// `Label::Hidden` is exempt, which is how the forms and the worksheet
    /// answer these keys without advertising them.
    #[test]
    fn every_filter_key_is_labelled_with_its_shared_word() {
        for topic in ALL {
            for entry in topic.keys() {
                let Some(filter) = FILTERS.iter().find(|filter| filter.key == entry.key) else {
                    continue;
                };
                let allowed = [Label::Hidden, Label::Own(filter.word)];
                assert!(
                    allowed.contains(&entry.label),
                    "{topic:?} labels {} something other than {:?}",
                    entry.key,
                    filter.word
                );
            }
        }
    }

    /// Whichever of the shared filters a screen offers lead its footer, in
    /// `FILTERS` order, before the first key the screen owns alone. The hand
    /// reaching for a filter then finds it in the same place on every screen
    /// that has one, rather than wherever that screen's table happened to put
    /// it.
    #[test]
    fn the_shared_filters_lead_every_screen_footer_in_one_order() {
        for topic in SCREENS {
            let items = footer_items(topic.keys());
            let places: Vec<Option<usize>> = items
                .iter()
                .map(|item| {
                    FILTERS
                        .iter()
                        .position(|filter| *item == format!("{} {}", filter.key, filter.word))
                })
                .collect();
            let filters: Vec<usize> = places.iter().flatten().copied().collect();
            assert_eq!(
                places[..filters.len()].iter().flatten().count(),
                filters.len(),
                "{topic:?} footer has a filter after a key of its own: {items:?}"
            );
            assert!(
                filters.windows(2).all(|pair| pair[0] < pair[1]),
                "{topic:?} footer takes the filters out of order: {items:?}"
            );
        }
    }

    /// Only a modal or a search box may join no footer. A screen topic with no
    /// labelled entries would render an empty footer line.
    #[test]
    fn every_screen_topic_joins_a_non_empty_footer() {
        for topic in SCREENS {
            assert!(!topic.footer().is_empty(), "{topic:?}");
        }
    }

    /// Every screen topic, so a new one must be added to the footer
    /// assertions below.
    const SCREENS: [Topic; 8] = [
        Topic::Overview,
        Topic::Ledger,
        Topic::Savings,
        Topic::Planning,
        Topic::Funds,
        Topic::RecurringTxns,
        Topic::RecurringGoals,
        Topic::Accounts,
    ];

    /// Every footer as it reads, with Planning's leading `↑/↓ constant`
    /// deliberately absent: the scroll keys are uniform across every list,
    /// so no footer names them.
    #[test]
    fn each_screen_topic_joins_the_footer_it_always_showed() {
        assert_eq!(Topic::Overview.footer(), "←/→ scrub · Shift+←/→ week");
        assert_eq!(
            Topic::Ledger.footer(),
            "Tab acct · [ ] month · Esc clear · / search · r target · a/t/p money · e edit · d delete"
        );
        assert_eq!(
            Topic::Savings.footer(),
            "Tab acct · [ ] month · Esc clear · / search · a/A/i allocate · n/e/c/K/J goal · f fave · U undo"
        );
        assert_eq!(
            Topic::Planning.footer(),
            "e edit · E/a/d bill · t transfers · Enter why · p pin · P unpin"
        );
        assert_eq!(
            Topic::RecurringTxns.footer(),
            "a add · e edit · d delete · g regen · G all · x extend · P paycheck"
        );
        assert_eq!(
            Topic::RecurringGoals.footer(),
            "[ ] month · Esc clear · / search · a add · e edit · d delete · s savings"
        );
        assert_eq!(Topic::Accounts.footer(), "a add · e edit");
    }

    #[test]
    fn the_funds_footer_names_every_key_the_screen_answers() {
        assert_eq!(Topic::Funds.footer(), "a add · e value · E edit · d delete");
    }

    /// The Credit ledger shares the Ledger topic with Cash but has no `t`:
    /// a transfer leaves an account you hold, so there is nothing on a card for
    /// it to start from.
    #[test]
    fn the_credit_footer_is_the_ledger_footer_without_transfer() {
        assert_eq!(
            Topic::Ledger.footer_without(&["t"]),
            "Tab acct · [ ] month · Esc clear · / search · r target · a/p money · e edit · d delete"
        );
    }

    /// Omitting a key from inside a `Shared` run shrinks the group rather than
    /// splitting it into two or dropping it outright -- the group's word still
    /// covers whichever of its keys are left.
    #[test]
    fn omitting_a_key_inside_a_shared_group_shrinks_it() {
        assert_eq!(
            Topic::Planning.footer_without(&["a"]),
            "e edit · E/d bill · t transfers · Enter why · p pin · P unpin"
        );
    }

    /// A `Shared` run at the very start of the table joins the same as one
    /// anywhere else: nothing about position zero is special-cased.
    #[test]
    fn a_shared_run_at_the_start_of_the_table_still_joins() {
        let entries = [
            Entry {
                key: "E",
                label: Label::Shared("bill"),
                detail: "d",
            },
            Entry {
                key: "a",
                label: Label::Shared("bill"),
                detail: "d",
            },
            Entry {
                key: "x",
                label: Label::Own("other"),
                detail: "d",
            },
        ];
        assert_eq!(join_footer(&entries), "E/a bill · x other");
    }

    /// A `Shared` run at the very end of the table closes it out with no
    /// trailing separator.
    #[test]
    fn a_shared_run_at_the_end_of_the_table_still_joins() {
        let entries = [
            Entry {
                key: "x",
                label: Label::Own("other"),
                detail: "d",
            },
            Entry {
                key: "E",
                label: Label::Shared("bill"),
                detail: "d",
            },
            Entry {
                key: "a",
                label: Label::Shared("bill"),
                detail: "d",
            },
        ];
        assert_eq!(join_footer(&entries), "x other · E/a bill");
    }

    /// A `Shared` run reduced to one entry -- by omission, upstream of this
    /// function -- renders plainly, with no `/` left over from a group of
    /// one.
    #[test]
    fn a_shared_run_of_one_entry_renders_without_a_slash() {
        let entries = [Entry {
            key: "E",
            label: Label::Shared("bill"),
            detail: "d",
        }];
        assert_eq!(join_footer(&entries), "E bill");
    }

    /// A `Shared` run emptied entirely by omission -- upstream of this
    /// function -- contributes nothing: no stray separator where the group
    /// used to be.
    #[test]
    fn a_shared_run_emptied_by_omission_leaves_no_stray_separator() {
        let entries = [
            Entry {
                key: "e",
                label: Label::Own("first"),
                detail: "d",
            },
            Entry {
                key: "q",
                label: Label::Own("second"),
                detail: "d",
            },
        ];
        assert_eq!(join_footer(&entries), "e first · q second");
    }

    /// Two runs of the same word, separated by an intervening entry, stay two
    /// footer items rather than merging: adjacency in the table is what
    /// groups a `Shared` run, not the word alone.
    #[test]
    fn two_separated_runs_of_the_same_word_stay_two_footer_items() {
        let entries = [
            Entry {
                key: "E",
                label: Label::Shared("bill"),
                detail: "d",
            },
            Entry {
                key: "m",
                label: Label::Own("mid"),
                detail: "d",
            },
            Entry {
                key: "a",
                label: Label::Shared("bill"),
                detail: "d",
            },
        ];
        assert_eq!(join_footer(&entries), "E bill · m mid · a bill");
    }

    /// A `Hidden` entry reaches the panel and contributes nothing to the
    /// footer, which is what every entry of a topic that joins no footer at
    /// all -- the modals and the search boxes -- is.
    #[test]
    fn a_hidden_entry_reaches_the_panel_but_not_the_footer() {
        let entries = [
            Entry {
                key: "Esc",
                label: Label::Hidden,
                detail: "d",
            },
            Entry {
                key: "e",
                label: Label::Own("edit"),
                detail: "d",
            },
        ];
        assert_eq!(join_footer(&entries), "e edit");

        // The other half of the property, which needs a real topic: every
        // entry reaches the panel, so a `Hidden` label withholds a key from
        // the footer without withholding its sentence.
        let drawn = wrap(Topic::Worksheet.keys(), 60);
        assert!(
            drawn.iter().any(|line| line.to_string().starts_with("Esc")),
            "a Hidden entry should still draw a panel row"
        );
    }

    /// A context that takes a typed character necessarily has a caret to edit,
    /// so the wider predicate has to cover the narrower one. The reverse does
    /// not hold: the worksheet and the transfer confirmation answer the
    /// editing keys over a date without taking a `?`.
    #[test]
    fn every_topic_that_takes_typed_chars_takes_the_editing_keys() {
        for topic in ALL {
            assert!(
                !topic.takes_typed_chars() || topic.takes_editing_keys(),
                "{topic:?} takes typed characters but not the keys that edit them"
            );
        }
    }

    /// Every table that names the editing keys is a context that answers
    /// them, and every context that answers them names them: the panel is
    /// where a key nobody could guess is advertised, and `App::dispatch`
    /// reads the same predicate to decide whether a `Ctrl` reaches a caret at
    /// all.
    #[test]
    fn a_topic_names_the_editing_keys_exactly_when_it_answers_them() {
        for topic in ALL {
            let named = topic.keys().iter().any(|e| e.key == EDITING_KEYS);
            assert_eq!(named, topic.takes_editing_keys(), "{topic:?}");
        }
    }

    /// The worksheet is the one context that answers these keys on some of
    /// its focuses and not others, so its entry has to say which -- and it
    /// must still be the shared sentence with a clause on the end rather than
    /// a second account of the eight keys.
    #[test]
    fn the_worksheets_editing_entry_qualifies_the_shared_one_rather_than_restating_it() {
        let entry = Topic::Worksheet
            .keys()
            .iter()
            .find(|e| e.key == EDITING_KEYS)
            .expect("the worksheet answers the editing keys");
        assert!(entry.detail.starts_with(EDITING.detail), "{}", entry.detail);
        assert!(
            entry.detail.contains("date is the one focus here"),
            "{}",
            entry.detail
        );
    }

    /// The app-wide keys are footer chrome, not panel rows: they mean the same
    /// thing on every screen, so no table names them and the panel does
    /// not repeat them in every table.
    #[test]
    fn no_topic_names_an_app_wide_key_in_its_table() {
        for topic in ALL {
            for entry in topic.keys() {
                assert_ne!(entry.key, SCREEN_KEYS.key, "{topic:?}");
                assert_ne!(entry.key, QUIT_KEY.key, "{topic:?}");
            }
        }
    }

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Draw the panel and hand back what landed in the buffer.
    ///
    /// Takes `&mut Help` and writes the extent back, exactly as `App::render`
    /// does: the panel cannot know how many rows a wrap produced until it has
    /// drawn once, and `bottom()` is meaningless before then.
    fn drawn(help: &mut Help, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut extent = (0, 0);
        terminal.draw(|frame| extent = render(frame, help)).unwrap();
        help.set_extent(extent.0, extent.1);
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// A detail longer than the panel is wide wraps onto a second line rather than
    /// truncating. Nothing here is right-aligned, so a wrap stays legible where a
    /// truncation would be a lost sentence.
    #[test]
    fn a_long_detail_wraps_instead_of_truncating() {
        let entries = [Entry {
            key: "a",
            label: Label::Hidden,
            detail: "one two three four five six seven eight nine ten",
        }];
        assert!(wrap(&entries, 30).len() > 1);
    }

    /// The key column is written once and the continuation lines are blank there,
    /// so a wrapped detail reads as one entry rather than two.
    #[test]
    fn only_the_first_line_of_a_wrapped_entry_carries_its_key() {
        let entries = [Entry {
            key: "a",
            label: Label::Hidden,
            detail: "one two three four five six seven eight nine ten",
        }];
        let rendered: Vec<String> = wrap(&entries, 30).iter().map(|l| l.to_string()).collect();
        assert!(rendered[0].starts_with("a "), "{rendered:?}");
        assert!(rendered[1].starts_with("  "), "{rendered:?}");
    }

    /// `Backspace` is nine characters, the longest key any table names. If the
    /// gutter were sized for `BackTab` (seven) the detail would abut it with no
    /// separating space at all.
    #[test]
    fn backspace_still_leaves_a_gap_before_its_detail() {
        let rendered: Vec<String> = wrap(&SEARCH, 60).iter().map(|l| l.to_string()).collect();
        let line = rendered
            .iter()
            .find(|l| l.starts_with("Backspace"))
            .expect("SEARCH names Backspace");
        assert!(line.starts_with("Backspace  "), "{line:?}");
    }

    /// The longest topic must be readable on the terminal this app is drawn for --
    /// scrolled, not clipped, so the last entry is reachable.
    #[test]
    fn the_longest_topic_is_readable_at_the_minimum_width_by_twenty_four() {
        let mut help = Help::new(Topic::Savings);
        let top = drawn(&mut help, MIN_WIDTH, 24);
        assert!(top.contains("Help · Savings"), "{top}");
        assert!(top.contains("Esc close"), "{top}");

        help.bottom();
        let bottom = drawn(&mut help, MIN_WIDTH, 24);
        assert!(bottom.contains("Undo the most recent batch"), "{bottom}");
    }

    /// The scroll stops at both ends: past the last line there is nothing to show,
    /// and above the first there is nothing to scroll to.
    #[test]
    fn the_panel_scroll_stops_at_both_ends() {
        let mut help = Help::new(Topic::Savings);
        help.set_extent(40, 10);
        help.scroll(-5);
        assert_eq!(help.offset(), 0);
        help.scroll(500);
        assert_eq!(help.offset(), 30);
    }

    /// A topic that fits does not scroll at all, or End would push its only screen
    /// off the top.
    #[test]
    fn a_topic_shorter_than_the_panel_never_scrolls() {
        let mut help = Help::new(Topic::Overview);
        help.set_extent(6, 20);
        help.bottom();
        assert_eq!(help.offset(), 0);
    }
    /// The panel is a reference, not an essay: no key gets more than a
    /// glance's worth of it.
    ///
    /// Eight lines is what the tallest entry -- Planning's `e`, which
    /// answers for four different kinds of editable row -- comes to. An
    /// entry that outgrows that is one explaining *why* the key is the key
    /// it is, and that reason belongs in `src/tui/CLAUDE.md`, where a
    /// maintainer looks for it, rather than in front of an owner who pressed
    /// `?` to find out what a key does.
    #[test]
    fn no_panel_entry_runs_longer_than_a_glance() {
        for topic in ALL {
            for entry in topic.keys() {
                let lines = wrap(std::slice::from_ref(entry), WIDTH - 2).len();
                assert!(lines <= 8, "{topic:?} {} takes {lines} lines", entry.key);
            }
        }
    }
}
