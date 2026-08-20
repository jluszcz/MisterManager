//! What every key does, in longer form than a footer has room for.
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
//! them and no panel repeats them; `Topic::chrome` appends them to the
//! footers instead, which still name them. Their guard is
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

const OVERVIEW: [Entry; 1] = [Entry {
    key: "←/→",
    label: Label::Own("scrub"),
    detail: "Move the Paycheck-Eve column a day at a time. View state only: the baseline is derived from the paycheck recurring transaction, so nothing here is saved and restarting discards it.",
}];

const LEDGER: [Entry; 11] = [
    Entry {
        key: "[ ]",
        label: Label::Own("month"),
        detail: "Step the month shown. Cash and Credit share one window, so both ledgers move together and always compare the same weeks.",
    },
    Entry {
        key: "Esc",
        label: Label::Own("today"),
        detail: "Return the window to the month containing today. The ledgers have no All to clear to -- the window bounds the query itself, so \"no filter\" would be every transaction ever -- so clearing it can only mean the window the screen opens on. Cash and Credit share one window, so this re-syncs both ledgers, the same as [ ].",
    },
    Entry {
        key: "Tab",
        label: Label::Own("account"),
        detail: "Cycle the account filter, All included.",
    },
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Cycle the account filter the other way. Unlabelled: it is Tab read backwards, and a footer word of its own would say the same thing twice.",
    },
    Entry {
        key: "/",
        label: Label::Own("search"),
        detail: "Filter rows by description as you type. Enter keeps the filter and leaves the box; Esc clears it.",
    },
    Entry {
        key: "r",
        label: Label::Own("target"),
        detail: "Reconcile the filtered account against a statement: type the balance it should hold, and the border carries it beside today's figure with the difference after it -- green above the target, red below it, a dash when they match. Needs an account filter, since under All the border quotes the whole kind's balance and no statement names that. An empty field clears it; Esc leaves it alone. Session state: nothing is written, and quitting forgets every target.",
    },
    Entry {
        key: "a",
        label: Label::Own("add"),
        detail: "Add a transaction, opening on the account the ledger is filtered to, or the first when the filter is All.",
    },
    Entry {
        key: "t",
        label: Label::Own("transfer"),
        detail: "Move money between two cash accounts. Cash ledger only: a transfer leaves an account you hold, so there is nothing on a card for it to start from.",
    },
    Entry {
        key: "p",
        label: Label::Own("pay"),
        detail: "Pay a credit card from a cash account, writing both sides.",
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

const SAVINGS: [Entry; 12] = [
    Entry {
        key: "Tab",
        label: Label::Own("container"),
        detail: "Cycle the container filter: All, then one entry per account that holds goals.",
    },
    Entry {
        key: "BackTab",
        label: Label::Hidden,
        detail: "Cycle the container filter the other way. Unlabelled: it is Tab read backwards, and a footer word of its own would say the same thing twice.",
    },
    Entry {
        key: "[ ]",
        label: Label::Own("month"),
        detail: "Step the goal-date filter, wrapping at either end. Pure view state, unlike the ledgers': every goal is already loaded for the reconciliation line, so there is nothing to re-query.",
    },
    Entry {
        key: "Esc",
        label: Label::Own("all"),
        detail: "Return to All, showing every goal again, undated ones included. The next step re-enters at today's month, or at the nearer end of the dated span when today falls outside it -- never the month you left, so no state crosses the All filter.",
    },
    Entry {
        key: "/",
        label: Label::Own("search"),
        detail: "Filter goals by name as you type, entirely in memory. Enter keeps the filter and leaves the box, so a, c and e stay usable on the narrowed list; Esc clears it.",
    },
    Entry {
        key: "a",
        label: Label::Own("allocate"),
        detail: "Allocate cash to the selected goal. One row, written as its own batch.",
    },
    Entry {
        key: "A",
        label: Label::Own("payday"),
        detail: "Open a payday worksheet for the container, prefilled from per-paycheck. One commit is one batch, so a fumbled payday is one undo rather than dozens of deletions. Payday means running it once per container.",
    },
    Entry {
        key: "i",
        label: Label::Own("interest"),
        detail: "Open an interest worksheet. Brokerage prefills pro rata; Rainy Day rescales its previous Interest batch, falling back to pro rata when there is none.",
    },
    Entry {
        key: "n",
        label: Label::Own("new"),
        detail: "Create a goal from scratch -- a name, a target and a date -- in the container Tab names. Not a, which is taken here by the allocation this screen is mostly used for. Goals created from recurring goal entries are s on screen 8, over on the table those entries live in.",
    },
    Entry {
        key: "c",
        label: Label::Own("close"),
        detail: "End the selected goal: return its value to unallocated, or move it to another goal in the same container. Crossing containers is refused, since no cash moved between the accounts.",
    },
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected goal's name, target and date.",
    },
    Entry {
        key: "U",
        label: Label::Own("undo"),
        detail: "Undo the most recent batch by insert order. Never an Import batch: that one holds every opening balance in the database.",
    },
];

const PLANNING: [Entry; 7] = [
    Entry {
        key: "e",
        label: Label::Own("edit"),
        detail: "Edit the selected row: a constant is typed into a field, a destination is chosen from a list of goals. Barely a third of the rows are editable, and the cursor settles on the nearest one that is after every move. Roth and Emergency Fund are read-only among the destinations: they share their setting key with the gate of the same name, so pointing one somewhere else would decide whether that gate fires rather than where a transfer lands.",
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
        detail: "Delete the selected bill, after a confirmation. A dropped bill inflates the excess the waterfall has left to allocate, which moves every line below it.",
    },
    Entry {
        key: "t",
        label: Label::Own("transfers"),
        detail: "Confirm the computed plan: writes its payday transfers, then opens the allocation worksheets prefilled. The ledger's t moves money between two accounts one row at a time; this one writes every row the plan calls for in a single transaction.",
    },
    Entry {
        key: "Enter",
        label: Label::Own("why"),
        detail: "Explain why the transfers could not be resolved, in full. The screen reports the failure in a cell about fifty columns wide, which is not enough to name the goal in the wrong container -- the one thing needed to act on it. Nothing to open when the transfers resolve.",
    },
    Entry {
        key: "p",
        label: Label::Own("pin"),
        detail: "Record today's excess and the date beside the live figure, so a later edit can be read against what it replaced. Pressing it again clears the pin.",
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
        detail: "Edit the selected row in full: the same form a adds with.",
    },
    Entry {
        key: "d",
        label: Label::Own("delete"),
        detail: "Delete the selected row, after a confirmation. Nothing here holds money -- the values are typed or imported, and no balance moves.",
    },
];

/// One key, because an account exists because the workbook names it: there
/// is nothing to add and nothing to delete, only what to call the one the
/// sheet already gave you.
const ACCOUNTS: [Entry; 1] = [Entry {
    key: "e",
    label: Label::Own("edit"),
    detail: "Edit the selected account: what it is called, which Overview band it sits in, where it sits among the accounts of its kind, and -- for a cash account -- how an interest posting against it is divided and which block of the Savings sheet it is the container for. That last one is what mm import waits on: the sheet names its two blocks by position and carries no account code, so the first import writes the accounts and stops until both blocks have been pointed at a container here. The code and the kind are not editable: both come from the workbook, and they are what the next import matches this row against. Nothing here is imported, so all of it survives mm import --replace.",
}];

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

const RECURRING_GOALS: [Entry; 6] = [
    Entry {
        key: "[ ]",
        label: Label::Own("month"),
        detail: "Step the month filter. Entries carry a month and no date, so the cycle is the calendar: December wraps to January. The screen opens on All, and the first step enters at this month.",
    },
    Entry {
        key: "Esc",
        label: Label::Own("all"),
        detail: "Return to All. The next step re-enters at this month rather than the one you left, so no state crosses the All filter.",
    },
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
        detail: "Open the picker: goals created from these entries, in the container the Savings screen's Tab names. Its own letter because nothing else in the app creates one kind of row out of another. The month filter opens it with those entries ticked and sorted to the top, so the ones about to be created are the ones the list opens on -- every entry is still listed below them, so the filter is a starting point rather than a cage. An entry that already has an open goal is left unticked, and sinks with the rest, since the annual reseed is what the ticks are for. A reseed is for the year ahead, so each goal is dated a year past its month's next occurrence -- a month already gone this year is next-occurring in the next one, and so lands the year after that. A biennial entry that has already had this year's round steps two years instead, skipping the year between rather than filling it.",
    },
];

/// Shared by all three search boxes: the keys are the same and only the list
/// underneath differs.
const SEARCH: [Entry; 4] = [
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Leave the box and keep the filter, so the row operators stay usable on the narrowed list.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Clear the filter and leave the box.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the last character. Every keystroke re-filters.",
    },
    Entry {
        key: "F1",
        label: Label::Hidden,
        detail: "Open this panel. A question mark types here instead, because a search may legitimately be for one.",
    },
];

const WORKSHEET: [Entry; 12] = [
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
        detail: "Step the date back or forward a day, while the date has focus. It stays typeable; this is the nudge. On the amount and the line list there is no date to move, so they do nothing.",
    },
    Entry {
        key: "Space",
        label: Label::Hidden,
        detail: "Select or deselect the line under the cursor. The selection is what s and /N operate on.",
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
        detail: "Spread what is left across the selected lines in the proportions they were prefilled with -- an interest posting's policy split, or what each goal asks of a paycheck.",
    },
    Entry {
        key: "/N",
        label: Label::Hidden,
        detail: "With a digit: divide the selected lines by it. With anything else: begin a name filter.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Commit every line as one batch, so a fumbled payday is one undo rather than dozens of deletions.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Discard the worksheet. Nothing has been written yet.",
    },
];

const PICKER: [Entry; 3] = [
    Entry {
        key: "Space",
        label: Label::Hidden,
        detail: "Select or deselect the entry under the cursor. The Open? column flags entries that already have an open goal -- a hint, not a refusal, since a second goal against one entry is legitimate.",
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
        detail: "Filter the goals by name as you type. Eighty-three open goals is a long way to scroll for a name three keystrokes would find, and it is the same key the ledgers and Savings filter with. The withdrawal row survives every search: clearing a line's destination must not depend on what the goals are called.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Point this line at the goal under the cursor, storing its id -- never its name, which three goals in this database share. The list opens on the suggested goal when there is one, and otherwise on the goal the line already names, so Enter straight away is either agreement or a no-op.",
    },
    Entry {
        key: "Esc",
        label: Label::Hidden,
        detail: "Close without changing where the line lands.",
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
        detail: "Open this panel: the one key besides y that does not cancel. A question mark that silently threw away a pending delete would be a worse surprise than this exception.",
    },
];

const FORM: [Entry; 7] = [
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
        detail: "Cycle a choice field, such as a bill's category or a close-out's destination -- or, on a date field, step it back or forward a day. A date stays typeable; this is the nudge. A field holding no date, such as an undated goal's, has nothing to step.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the last character of the focused text field. A choice field ignores it.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Save. A value that will not parse reports itself in the status line and the form stays open.",
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

const SUGGEST_FORM: [Entry; 8] = [
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
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the last character of the focused text field, re-querying the suggestions: backing a letter out of the description widens them again.",
    },
    Entry {
        key: "Enter",
        label: Label::Hidden,
        detail: "Accept the highlighted suggestion if any are on screen, otherwise save the form.",
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
const PLAN_TRANSFERS: [Entry; 4] = [
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
        key: "Enter",
        label: Label::Hidden,
        detail: "Parse the date and commit: writes the payday transfers, then opens the allocation worksheets prefilled. A date that will not parse reports itself in the status line and leaves the dialog open.",
    },
    Entry {
        key: "Backspace",
        label: Label::Hidden,
        detail: "Delete the last character of the date. Typing appends to the prefill rather than replacing it, so retyping the date means backspacing it out first.",
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

    /// The app-wide keys this context's footer ends with.
    ///
    /// Empty for every modal and search topic, which set no footer at all;
    /// `q` alone for the two ledgers and Savings, whose footers are the
    /// longest in the app and drop `1-9` rather than grow.
    fn chrome(self) -> &'static [Chrome] {
        match self {
            Topic::Overview
            | Topic::Planning
            | Topic::Funds
            | Topic::RecurringTxns
            | Topic::RecurringGoals
            | Topic::Accounts => &[SCREEN_KEYS, QUIT_KEY],
            Topic::Ledger | Topic::Savings => &[QUIT_KEY],
            Topic::LedgerSearch
            | Topic::SavingsSearch
            | Topic::WorksheetSearch
            | Topic::DestinationSearch
            | Topic::Worksheet
            | Topic::Picker
            | Topic::Destination
            | Topic::Details
            | Topic::Confirm
            | Topic::Form
            | Topic::SuggestForm
            | Topic::PlanTransfers => &[],
        }
    }

    /// The footer line: the labelled entries, joined, then the chrome.
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
    /// that shows a footer shows all of its own.
    pub(super) fn footer_without(self, omit: &[&str]) -> String {
        let live: Vec<Entry> = self
            .keys()
            .iter()
            .filter(|entry| !omit.contains(&entry.key))
            .copied()
            .collect();
        let mut items = footer_items(&live);
        items.extend(
            self.chrome()
                .iter()
                .map(|chrome| format!("{} {}", chrome.key, chrome.word)),
        );
        items.join(SEPARATOR)
    }
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
    const ALL: [Topic; 20] = [
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
        assert_eq!(Topic::Overview.footer(), "←/→ scrub · 1-9 screens · q quit");
        assert_eq!(
            Topic::Ledger.footer(),
            "[ ] month · Esc today · Tab account · / search · r target · a add · t transfer · p pay · e edit · d delete · q quit"
        );
        assert_eq!(
            Topic::Savings.footer(),
            "Tab container · [ ] month · Esc all · / search · a allocate · A payday · i interest · n new · c close · e edit · U undo · q quit"
        );
        assert_eq!(
            Topic::Planning.footer(),
            "e edit · E/a/d bill · t transfers · Enter why · p pin · 1-9 screens · q quit"
        );
        assert_eq!(
            Topic::RecurringTxns.footer(),
            "a add · e edit · d delete · g regen · G all · x extend · P paycheck · 1-9 screens · q quit"
        );
        assert_eq!(
            Topic::RecurringGoals.footer(),
            "[ ] month · Esc all · a add · e edit · d delete · s savings · 1-9 screens · q quit"
        );
        assert_eq!(Topic::Accounts.footer(), "e edit · 1-9 screens · q quit");
    }

    #[test]
    fn the_funds_footer_names_every_key_the_screen_answers() {
        assert_eq!(
            Topic::Funds.footer(),
            "a add · e value · E edit · d delete · 1-9 screens · q quit"
        );
    }

    /// The Credit ledger shares the Ledger topic with Cash but has no `t`:
    /// a transfer leaves an account you hold, so there is nothing on a card for
    /// it to start from.
    #[test]
    fn the_credit_footer_is_the_ledger_footer_without_transfer() {
        assert_eq!(
            Topic::Ledger.footer_without(&["t"]),
            "[ ] month · Esc today · Tab account · / search · r target · a add · p pay · e edit · d delete · q quit"
        );
    }

    /// Omitting a key from inside a `Shared` run shrinks the group rather than
    /// splitting it into two or dropping it outright -- the group's word still
    /// covers whichever of its keys are left.
    #[test]
    fn omitting_a_key_inside_a_shared_group_shrinks_it() {
        assert_eq!(
            Topic::Planning.footer_without(&["a"]),
            "e edit · E/d bill · t transfers · Enter why · p pin · 1-9 screens · q quit"
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
}
