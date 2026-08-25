//! `App` -- the one state the whole application is drawn from, and the
//! dispatch that decides which screen a keystroke reaches.
//!
//! The screens' handlers live one module per screen beside this one, as
//! `impl App` blocks of their own: a screen owns its keys, its `open_*`
//! forms, its `commit_*` writes and its `reload_*`, and nothing else names
//! them. What stays here is what is *about the app* rather than about a
//! screen -- the struct itself, `dispatch`, `render`, `footer`, `reload`,
//! and the modal and form plumbing every screen borrows.
//!
//! **The exhaustive `match self.screen` blocks are the point of keeping them
//! here.** A tenth screen has to be answered in each one before the crate
//! compiles, which is a guarantee a trait object per screen would give up --
//! so the split is by file, never by dyn.

use super::accounts::{self as accounts_screen, Accounts};
use super::autocomplete::Autocomplete;
use super::cursor::Scroll;
use super::form::Step;
use super::fund::{self as fund_screen, Funds};
use super::help::{self, Help, Topic};
use super::ledger::{self as ledger_screen, Ledger};
use super::modal::{self, Confirm, Modal};
use super::planning::{self as planning_screen, Planning};
use super::recurring_goal::{self as recurring_goal_screen, RecurringGoals};
use super::recurring_txn::{self as recurring_txn_screen, RecurringTxns};
use super::savings::{self as savings_screen, Savings};
use super::search::{self, Search};
use super::text::{self, Edit};
use super::{Account, Label, destination, overview};
use crate::db::account::{self, Kind};
use crate::db::setting::{self, key};
use crate::db::{AccountId, Db, GoalId, txn};
use crate::money::Cents;
use crate::overview::Overview;
use crate::projection::{self, Dates};
use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Paragraph, Tabs};
use std::time::{Duration, Instant};

mod accounts;
mod funds;
mod ledger;
mod planning;
mod recurring;
mod savings;
#[cfg(test)]
mod test_support;
mod worksheet;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Overview = 0,
    Cash = 1,
    Credit = 2,
    Savings = 3,
    Planning = 4,
    Funds = 5,
    RecurringGoals = 6,
    RecurringTxns = 7,
    Accounts = 8,
}

impl Screen {
    /// Every screen, in the order the tab bar draws them.
    ///
    /// That order is the discriminants', because the bar selects the current
    /// tab by `self.screen as usize` -- a list in any other order would
    /// highlight the wrong label.
    const ALL: [Screen; 9] = [
        Screen::Overview,
        Screen::Cash,
        Screen::Credit,
        Screen::Savings,
        Screen::Planning,
        Screen::Funds,
        Screen::RecurringGoals,
        Screen::RecurringTxns,
        Screen::Accounts,
    ];

    /// What the tab bar calls this screen, digit included.
    ///
    /// Abbreviated on purpose. The bar is a row of shortcuts, not a set of
    /// headings: spelled out, "7 Recurring Goals" and "8 Recurring Txns"
    /// spend fourteen columns restating what the screen's own title and
    /// footer say the moment it is opened.
    ///
    /// Beside the discriminants for `from_key`'s reason, and because a label
    /// written out at the one call site that draws it is checked by nothing:
    /// a screen added to the enum without one is a compile error here.
    fn tab_label(self) -> &'static str {
        match self {
            Screen::Overview => "1 Overview",
            Screen::Cash => "2 Cash",
            Screen::Credit => "3 Credit",
            Screen::Savings => "4 Savings",
            Screen::Planning => "5 Planning",
            Screen::Funds => "6 Funds",
            Screen::RecurringGoals => "7 Goals",
            Screen::RecurringTxns => "8 Txns",
            Screen::Accounts => "9 Accounts",
        }
    }

    /// The screen a top-row digit switches to, or `None` for any other key.
    ///
    /// Beside the discriminants because it is the other half of the same
    /// fact: the tab bar selects by `self.screen as usize` and labels each tab
    /// with the digit that reaches it, so the digit and the discriminant have
    /// to agree. Written out rather than derived from the digit's value so
    /// that a screen inserted in the middle is one edit here and not an
    /// arithmetic puzzle.
    fn from_key(c: char) -> Option<Screen> {
        Some(match c {
            '1' => Screen::Overview,
            '2' => Screen::Cash,
            '3' => Screen::Credit,
            '4' => Screen::Savings,
            '5' => Screen::Planning,
            '6' => Screen::Funds,
            '7' => Screen::RecurringGoals,
            '8' => Screen::RecurringTxns,
            '9' => Screen::Accounts,
            _ => return None,
        })
    }
}

/// Which way `K` and `J` move a goal in its container's manual order.
///
/// A direction rather than a signed delta because the two ends are not
/// symmetric arithmetic: the top of the block and the bottom are each a place
/// there is nothing beyond, and `applied` returning `None` there is what lets
/// the caller stop before it writes rather than after it clamps.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Move {
    Up,
    Down,
}

impl Move {
    /// The position `from` moves to among `len` goals, or `None` when the
    /// move would leave the block.
    fn applied(self, from: usize, len: usize) -> Option<usize> {
        match self {
            Move::Up => from.checked_sub(1),
            Move::Down => (from + 1 < len).then_some(from + 1),
        }
    }
}

/// Which step an arrow press stands for: `week` while `Shift` is held, `day`
/// otherwise.
///
/// The three handlers that answer an arrow themselves -- the Overview scrub,
/// the worksheet, and `t`'s confirmation -- read the modifier through this
/// rather than each testing it, so `Shift` cannot come to mean a week on two
/// of them and nothing on the third. `form_key` inlines the same test because
/// it already holds the answer for the `Tab` arms beside it.
fn week_step(key: KeyEvent, week: Step, day: Step) -> Step {
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        week
    } else {
        day
    }
}

/// How long a status message holds the footer before the screen's own keys
/// come back.
///
/// Long enough to read a written-transfer count or a parse error, short enough
/// that the keys are back before the next thing is typed. The event loop
/// checks for expiry once per `tui::TICK`, so the fade lands within a quarter
/// second of this.
pub const STATUS_TTL: Duration = Duration::from_secs(4);

/// What a key that acts on a row says when the screen has no row under the
/// cursor. Written once so every screen refuses in the same words -- an
/// empty list is the same state whichever list it is.
const NOTHING_SELECTED: &str = "nothing selected";

/// One queued worksheet: the container it opens on, the date its transfer
/// was written for, its pot, and the shares it opens with.
type WorksheetPrefill = (AccountId, NaiveDate, Cents, Vec<(GoalId, Cents)>);

/// The whole application: which screen is showing, what was last queried, and
/// what the status line says.
///
/// Owns the `Db` and re-queries after every write, caching the result until
/// the next one. Deliberately dumb: at 1,600 rows a year nothing cleverer
/// earns its complexity, and a stale cache is the one bug class here that
/// would be genuinely hard to see.
pub struct App {
    db: Db,
    /// Rows dated after this render dim on the Cash and Credit ledgers.
    today: NaiveDate,
    /// The three projection dates as stored. `adhoc` below is the scrubbed
    /// view of the middle one.
    dates: Dates,
    /// The scrubbed ad-hoc date. **View state**: restarting discards it, and
    /// there is nothing that commits it -- moving the baseline means editing
    /// the paycheck recurring transaction.
    adhoc: NaiveDate,
    /// The date the last row *added* this session was written for, which is
    /// what the next `a`, `t` or `p` opens on. **View state**, like `adhoc`:
    /// restarting is what returns the form to today.
    ///
    /// `None` is "nothing added yet" rather than a date, so today stays the
    /// answer for the first row of a session without a second rule saying so.
    /// One field for both ledgers and all three keys: it records the day
    /// being entered for, which a statement, the card rows on it, and the
    /// payment settling them all share.
    entry_date: Option<NaiveDate>,
    screen: Screen,
    overview: Overview,
    cash: Ledger,
    credit: Ledger,
    savings: Savings,
    planning: Planning,
    funds: Funds,
    recurring_txn: RecurringTxns,
    recurring_goal: RecurringGoals,
    accounts: Accounts,
    /// `Constants!H2` in the workbook — the number of days between paychecks,
    /// what `calc::per_paycheck` divides the runway into.
    period_days: i64,
    /// The suggestion list under whichever form is open. Lives on `App`
    /// rather than on the forms because `App` owns the `Db` the query needs.
    popup: Autocomplete,
    modal: Option<Modal>,
    /// Worksheets waiting to open, in order, after a payday's transfers are
    /// written. Each is a container, the date its transfer was written for,
    /// its pot, and the shares it opens with.
    ///
    /// A queue rather than one modal because a payday funds two containers
    /// and a worksheet is scoped to one: the second opens as the first
    /// closes, whether the first was committed or cancelled.
    pending_worksheets: Vec<WorksheetPrefill>,
    /// The open Help panel, drawn above everything including a modal.
    help: Option<Help>,
    status: String,
    /// When the current status message stops being shown, or `None` when
    /// there is none. Set once, in `on_key`, from whatever `dispatch` left in
    /// `status` -- the fifty-odd places that write a message do not each have
    /// to remember to start its clock.
    status_until: Option<Instant>,
    quit: bool,
}

/// How far either side of the transfer date `t` looks for a payday it has
/// already written, in business days.
///
/// Wide enough to reach the day a run being corrected landed on -- a re-run
/// is rarely more than a day or two later -- and narrow enough that it
/// cannot reach the payday before this one, which is ten business days back.
const DUPLICATE_SCAN_DAYS: i64 = 2;

/// Add `amount` to whatever `goal` is already down for.
///
/// Two Planning lines may name one goal -- `open_destination` offers
/// every open goal, claimed or not -- and `transfer::plan` merges such
/// lines into a single transfer. A second entry under the same id would
/// be dropped by `Worksheet::set_lines`, which resolves each of its
/// one-per-goal lines by the first match, leaving the sheet short by the
/// second line and that difference indistinguishable from the remainder
/// `calc::fit` leaves unallocated on purpose.
fn add_share(shares: &mut Vec<(GoalId, Cents)>, goal: GoalId, amount: Cents) {
    match shares.iter_mut().find(|(id, _)| *id == goal) {
        Some((_, c)) => *c += amount,
        None => shares.push((goal, amount)),
    }
}

/// The footer a screen shows while its `/` box is open: the needle with the
/// caret on it, and what the two keys leaving the box do.
///
/// One function for both screen-level boxes, for the reason `search_key`
/// answers both: a box that said this in its own words could come to say
/// something else.
fn search_footer(target: &impl Search) -> TextLine<'static> {
    let mut spans = vec![Span::raw("/")];
    spans.extend(target.search_spans());
    spans.push(Span::raw("  · Enter to keep · Esc to clear"));
    TextLine::from(spans)
}

impl App {
    pub fn new(db: Db, today: NaiveDate) -> Result<App> {
        let dates = projection::dates(&db, today)?;
        let range = txn::date_range(&db)?;
        let period_days = setting::get_or(&db, key::PAY_PERIOD_DAYS, 14)?;
        let mut app = App {
            overview: Overview::load(&db, dates)?,
            cash: Ledger::new(
                Kind::Cash,
                account::list_by_kind(&db, Kind::Cash)?,
                range,
                today,
            ),
            credit: Ledger::new(
                Kind::Credit,
                account::list_by_kind(&db, Kind::Credit)?,
                range,
                today,
            ),
            savings: Savings::new(account::list(&db)?, today, period_days),
            planning: Planning::new(),
            funds: Funds::new(),
            recurring_txn: RecurringTxns::new(account::list(&db)?),
            recurring_goal: RecurringGoals::new(i64::from(today.month())),
            accounts: Accounts::new(),
            period_days,
            db,
            today,
            dates,
            adhoc: dates.adhoc,
            entry_date: None,
            screen: Screen::Overview,
            popup: Autocomplete::default(),
            modal: None,
            pending_worksheets: Vec::new(),
            help: None,
            status: String::new(),
            status_until: None,
            quit: false,
        };
        app.reload()?;
        app.cash.select_at_or_before(today);
        app.credit.select_at_or_before(today);
        Ok(app)
    }

    /// Give the database back at the end of the run. Consuming rather than
    /// borrowing: the application is over, and a borrow would keep it alive.
    pub fn into_db(self) -> Db {
        self.db
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// A failed write renders in the status line and the app keeps running:
    /// losing an hour of navigation because a date did not parse is not
    /// acceptable.
    pub fn on_key(&mut self, key: KeyEvent) {
        self.status.clear();
        if let Err(err) = self.dispatch(key) {
            self.status = format!("{err:#}");
        }
        // A message under an open modal keeps the footer until a key takes it
        // away. It is there to qualify the question the modal is asking -- the
        // duplicate rows a payday would land on top of, the field a form
        // refused -- and the modal does not repeat it, so a message that faded
        // out from under an unanswered question would leave the answer to be
        // given without it. Under a modal the next key press is exactly what
        // is being waited for, which is what makes the timeout unnecessary
        // there rather than merely unwanted.
        self.status_until =
            (!self.status.is_empty() && self.modal.is_none()).then(|| Instant::now() + STATUS_TTL);
    }

    /// Say [`NOTHING_SELECTED`] and do nothing else -- what a key acting on a
    /// row does when there is none.
    ///
    /// `Ok(())` rather than an error: an empty list is an ordinary state, not
    /// a failure to report. The handlers that call it are the ones a key press
    /// reaches directly, so the whole refusal is `return
    /// self.nothing_selected();` -- the handful that answer with `()` set
    /// [`NOTHING_SELECTED`] themselves rather than grow a `Result` to say the
    /// same thing.
    fn nothing_selected(&mut self) -> Result<()> {
        self.status = NOTHING_SELECTED.to_string();
        Ok(())
    }

    /// Drop a status message that has outlived [`STATUS_TTL`], and say
    /// whether one was dropped.
    ///
    /// Called by the event loop rather than by `footer`, which only reads:
    /// the message is gone from the app, not merely hidden, so the next thing
    /// to consult `status` sees what the footer shows. A key press clears it
    /// sooner -- this is only what happens when none arrives.
    ///
    /// The answer is what the loop redraws on: an expiry is the one thing
    /// that changes the footer with no event behind it, so a loop that drew
    /// only on events would leave a faded message on screen until the next
    /// keystroke.
    pub fn expire_status(&mut self) -> bool {
        self.expire_status_at(Instant::now())
    }

    fn expire_status_at(&mut self, now: Instant) -> bool {
        let expired = self.status_until.is_some_and(|until| now >= until);
        if expired {
            self.status.clear();
            self.status_until = None;
        }
        expired
    }

    fn dispatch(&mut self, key: KeyEvent) -> Result<()> {
        if self.help.is_some() {
            self.help_key(key);
            return Ok(());
        }
        // Before the modal check, which is what gives the confirm dialogs their
        // one exception to "any key but y cancels".
        let topic = self.topic();
        if key.code == KeyCode::F(1)
            || (key.code == KeyCode::Char('?') && !topic.takes_typed_chars())
        {
            self.help = Some(Help::new(topic));
            return Ok(());
        }
        // `Ctrl` means editing the text under the caret, and `Alt` means
        // nothing at all -- so where there is no caret, a modified character
        // is dropped rather than read as the bare letter it arrives as. A
        // hand reaching for `Ctrl`+`F` a beat after `Esc` closed a form must
        // not find the `f` that toggles a favorite, and `Ctrl`+`D` must not
        // raise the delete a `Ctrl`+`U` was meant to unmake. Where there *is*
        // a caret, `text::edit_key` is what drops the combinations it has no
        // binding for.
        if matches!(key.code, KeyCode::Char(_))
            && !text::is_bare(key)
            && !topic.takes_editing_keys()
        {
            return Ok(());
        }
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        // Both boxes narrow a list already in memory, so a keystroke is the
        // screen's own `refilter` and nothing else -- no re-query, and no
        // handler of their own to hold the difference that used to be here.
        match self.screen {
            Screen::Cash | Screen::Credit if self.ledger().is_searching() => {
                search::search_key(self.ledger_mut(), key);
                return Ok(());
            }
            Screen::Savings if self.savings.is_searching() => {
                search::search_key(&mut self.savings, key);
                return Ok(());
            }
            _ => {}
        }
        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                return Ok(());
            }
            // The one screen key that does more than switch: entering Funds
            // is what raises the birth-date prompt.
            KeyCode::Char('6') => {
                self.open_funds();
                return Ok(());
            }
            KeyCode::Char(c) => {
                if let Some(screen) = Screen::from_key(c) {
                    self.screen = screen;
                    return Ok(());
                }
            }
            _ => {}
        }
        match self.screen {
            Screen::Overview => self.overview_key(key),
            Screen::Savings => self.savings_key(key),
            Screen::Planning => self.planning_key(key),
            Screen::Funds => self.funds_key(key),
            Screen::RecurringTxns => self.recurring_txn_key(key),
            Screen::RecurringGoals => self.recurring_goal_key(key),
            Screen::Accounts => self.accounts_key(key),
            _ => self.ledger_key(key),
        }
    }

    fn overview_key(&mut self, key: KeyEvent) -> Result<()> {
        // Shift is the same nudge, a week at a time. `←`/`→` step a date a
        // day wherever there is one, and this is the one date in the app
        // that is *scrubbed* rather than typed -- a horizon several paydays
        // out is a plausible question here, and nowhere else, so the bigger
        // step is a modifier on the key that already means "move this date"
        // rather than a second letter for the same action.
        let step = week_step(key, Step::NEXT_WEEK, Step::NEXT).days();
        match key.code {
            KeyCode::Left => self.scrub(-step)?,
            KeyCode::Right => self.scrub(step)?,
            _ => {}
        }
        Ok(())
    }

    /// Move the Paycheck-Eve date without touching the paycheck recurring
    /// transaction. To-Date and Month-End are derived from today and cannot
    /// scrub.
    fn scrub(&mut self, days: i64) -> Result<()> {
        self.adhoc = self
            .adhoc
            .checked_add_signed(chrono::Duration::days(days))
            .context("the ad-hoc date ran off the end of the calendar")?;
        self.reload_overview()?;
        // The drift is what the press did, so it is a message rather than a
        // property of the screen: it takes `STATUS_TTL` from `on_key` like
        // every other one and gives the Overview's keys back. A scrub is
        // meant to be left standing while the columns are read, and a footer
        // that reported it for that whole time would cost the keys for as
        // long as the question is open. What outlives the message is the `*`
        // on the column header, which is drawn from the scrub itself.
        self.status = match self.scrubbed_days() {
            0 => "back to Paycheck-Eve".to_string(),
            drift => format!("scrubbed {drift:+}d"),
        };
        // The Overview is not the only screen quoting a balance at this date:
        // Planning's `Excess (Actual)` is the checking balance there, and the
        // payday `t` writes is computed from it.
        self.reload_planning()
    }

    fn dates(&self) -> Dates {
        Dates {
            adhoc: self.adhoc,
            ..self.dates
        }
    }

    fn reload_overview(&mut self) -> Result<()> {
        self.overview = Overview::load(&self.db, self.dates())?;
        Ok(())
    }

    /// Re-query everything after a write, a filter change, or a scrub.
    fn reload(&mut self) -> Result<()> {
        // Derived from the paycheck recurring transaction, so `P`, `e` and `d`
        // all move it. The scrub is an offset from the baseline, so it moves
        // with it rather than being silently reinterpreted against a new one.
        let previous = self.dates.adhoc;
        let offset = self.adhoc - previous;
        self.dates = projection::dates(&self.db, self.today)?;
        self.adhoc = self
            .dates
            .adhoc
            .checked_add_signed(offset)
            .context("the ad-hoc date ran off the end of the calendar")?;
        self.reload_overview()?;
        // Ahead of every screen that holds its own copy of the list: a Savings
        // row caches its container's name at `set_goals` time, and a ledger's
        // `filter` reads whichever accounts it was last handed.
        self.reload_accounts()?;
        let range = txn::date_range(&self.db)?;
        for ledger in self.ledgers_mut() {
            ledger.set_date_range(range);
        }
        let cash_rows = txn::list(&self.db, &self.cash.filter())?;
        self.cash.set_rows(cash_rows);
        let cash_total = self.ledger_total(&self.cash)?;
        self.cash.set_total(cash_total);
        let credit_rows = txn::list(&self.db, &self.credit.filter())?;
        self.credit.set_rows(credit_rows);
        let credit_total = self.ledger_total(&self.credit)?;
        self.credit.set_total(credit_total);
        self.reload_savings()?;
        self.reload_planning()?;
        self.reload_funds()?;
        self.reload_recurring_txns()?;
        self.reload_recurring_goals()?;
        Ok(())
    }

    /// Re-query after a change that replaced the visible rows wholesale — a
    /// slid window or a switched account — and put the cursor back on today.
    ///
    /// A plain [`App::reload`] keeps the cursor where it is, which is what a
    /// write wants and what a new row set cannot honor: the old index means
    /// nothing against rows the user has not seen.
    fn reload_and_anchor(&mut self) -> Result<()> {
        self.reload()?;
        let today = self.today;
        self.ledger_mut().select_at_or_before(today);
        Ok(())
    }

    /// Put both ledgers on the month the active one just stepped to.
    ///
    /// `[` and `]` are one control over a month Cash and Credit share, so
    /// switching between `2` and `3` compares the same weeks rather than
    /// whichever month each screen was last left on. Both cursors re-anchor,
    /// since the rows moved out from under each of them.
    ///
    /// **View state only**: nothing persists it, and both ledgers reopen on
    /// the window around today.
    fn sync_month(&mut self) -> Result<()> {
        let window = self.ledger().window();
        for ledger in self.ledgers_mut() {
            ledger.set_window(window);
        }
        self.reload()?;
        let today = self.today;
        for ledger in self.ledgers_mut() {
            ledger.select_at_or_before(today);
        }
        Ok(())
    }

    /// How far the view has been scrubbed from the derived Paycheck-Eve date.
    ///
    /// **View state only**: restarting discards it. There is nothing to save
    /// -- the baseline is derived from the paycheck recurring transaction, and
    /// moving it means editing the recurring transaction.
    fn scrubbed_days(&self) -> i64 {
        (self.adhoc - self.dates.adhoc).num_days()
    }

    /// The status line, or the current screen's keys.
    ///
    /// Joined from `help::Topic` rather than written here, so a key cannot
    /// appear in the footer with one name and in the Help panel with another.
    /// Two cases stay hand-written because they are not key lists: both search
    /// boxes echo what has been typed.
    ///
    /// A `match` over the screen, guarded the way [`App::topic`] is, so a ninth
    /// screen is a compile error here rather than a footer that silently reads
    /// as the ledger's.
    fn footer(&self) -> TextLine<'static> {
        if !self.status.is_empty() {
            return TextLine::from(self.status.clone());
        }
        match self.screen {
            Screen::Overview => TextLine::from(Topic::Overview.footer()),
            Screen::Cash | Screen::Credit if self.ledger().is_searching() => {
                search_footer(self.ledger())
            }
            Screen::Cash => TextLine::from(Topic::Ledger.footer()),
            // `t` opens a transfer, which the Credit ledger does not offer: a
            // footer must not name a key its screen refuses.
            Screen::Credit => TextLine::from(Topic::Ledger.footer_without(&["t"])),
            Screen::Savings if self.savings.is_searching() => search_footer(&self.savings),
            Screen::Savings => TextLine::from(Topic::Savings.footer()),
            // `P` is live either way -- it says "nothing pinned" rather than
            // failing silently -- but naming it on an unpinned screen would
            // offer to clear something that is not there.
            Screen::Planning => TextLine::from(match self.planning.is_pinned() {
                true => Topic::Planning.footer(),
                false => Topic::Planning.footer_without(&["P"]),
            }),
            // The other dynamic footer, but a prefix rather than a replaced
            // word: a screen showing a row with no target must say why rather
            // than leave a dash unexplained.
            Screen::Funds => TextLine::from(match self.funds.needs_birth_date() {
                true => format!("birth date unset · {}", Topic::Funds.footer()),
                false => Topic::Funds.footer(),
            }),
            Screen::RecurringTxns => TextLine::from(Topic::RecurringTxns.footer()),
            Screen::RecurringGoals => TextLine::from(Topic::RecurringGoals.footer()),
            Screen::Accounts => TextLine::from(Topic::Accounts.footer()),
        }
    }

    /// The app-wide keys, drawn against the footer's right edge -- or nothing,
    /// wherever they would be a lie.
    ///
    /// Three things withhold them, and the first two are the same rule: show
    /// them only where `dispatch` answers them. `Topic::answers_app_wide_keys`
    /// is that rule, and `App::topic` is what resolves the context, so a modal
    /// and the two screen-level search boxes are covered by one question
    /// rather than a list of screens re-derived here. The open Help panel is
    /// the third: it is not a `Topic` -- `dispatch` returns into `help_key`
    /// above everything, which takes Esc and the scroll keys and drops the
    /// rest -- so it is asked about separately.
    ///
    /// A status message withholds them too, for a different reason: it borrows
    /// the whole line for `STATUS_TTL` and gives it back, and a line half
    /// message and half key list reads as neither.
    fn footer_chrome(&self) -> String {
        let live = self.help.is_none() && self.topic().answers_app_wide_keys();
        match self.status.is_empty() && live {
            true => help::chrome(),
            false => String::new(),
        }
    }

    fn kind(&self) -> Kind {
        match self.screen {
            Screen::Credit => Kind::Credit,
            _ => Kind::Cash,
        }
    }

    /// The keys that are live right now.
    ///
    /// A modal wins over the screen it is drawn on, because a modal takes
    /// every key -- which is why the modal's own answer is [`Modal::topic`]
    /// and this only asks it. Below that, the `is_searching` checks come
    /// first for the same reason: while a search box is up, the screen's own
    /// operators are not reachable.
    fn topic(&self) -> Topic {
        if let Some(modal) = &self.modal {
            return modal.topic();
        }
        match self.screen {
            Screen::Overview => Topic::Overview,
            Screen::Cash | Screen::Credit if self.ledger().is_searching() => Topic::LedgerSearch,
            Screen::Cash | Screen::Credit => Topic::Ledger,
            Screen::Savings if self.savings.is_searching() => Topic::SavingsSearch,
            Screen::Savings => Topic::Savings,
            Screen::Planning => Topic::Planning,
            Screen::Funds => Topic::Funds,
            Screen::RecurringTxns => Topic::RecurringTxns,
            Screen::RecurringGoals => Topic::RecurringGoals,
            Screen::Accounts => Topic::Accounts,
        }
    }

    /// Every key while the panel is up, including the ones it ignores.
    ///
    /// Returns nothing because nothing here can fail: the panel reads a static
    /// table and touches no database.
    fn help_key(&mut self, key: KeyEvent) {
        let Some(help) = &mut self.help else {
            return;
        };
        let page = i32::from(help.page());
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1) => self.help = None,
            KeyCode::Up => help.scroll(-1),
            KeyCode::Down => help.scroll(1),
            KeyCode::PageUp => help.scroll(-page),
            KeyCode::PageDown => help.scroll(page),
            KeyCode::Home => help.top(),
            KeyCode::End => help.bottom(),
            _ => {}
        }
    }

    fn close_modal(&mut self) {
        self.modal = None;
        self.popup.clear();
    }

    /// Any key but `y` cancels, and the dialog closes either way.
    ///
    /// The write runs while the modal is still up, so a refusal --
    /// `recurring_goal::delete` while a goal still references the entry --
    /// leaves the question on screen with the reason under it, rather than
    /// closing the dialog on an error the owner has not read yet.
    fn confirm_key(&mut self, key: KeyEvent, action: Confirm) -> Result<()> {
        if key.code != KeyCode::Char('y') {
            self.close_modal();
            self.status = action.cancelled().to_string();
            return Ok(());
        }
        let status = action.commit(&self.db)?;
        self.close_modal();
        self.status = status;
        self.reload()
    }

    /// Which handler answers the open modal's keys.
    ///
    /// The one match over `Modal` that is not in [`super::modal`]: every arm
    /// is a call into a handler on `App`, so it sits beside those rather than
    /// beside the enum.
    fn modal_key(&mut self, key: KeyEvent) -> Result<()> {
        match self.modal {
            None => Ok(()),
            Some(Modal::Confirm { action, .. }) => self.confirm_key(key, action),
            Some(Modal::Txn(_)) => self.form_key(key, App::commit_txn_form),
            Some(Modal::Transfer(_)) => self.form_key(key, App::commit_transfer_form),
            Some(Modal::Allocation(_)) => self.form_key(key, App::commit_allocation),
            Some(Modal::Goal(_)) => self.form_key(key, App::commit_goal),
            Some(Modal::CloseOut(_)) => self.form_key(key, App::commit_close_out),
            Some(Modal::Worksheet(_)) => self.worksheet_key(key),
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Destination(_)) => self.destination_key(key),
            Some(Modal::Details(..)) => {
                if key.code == KeyCode::Esc {
                    self.close_modal();
                }
                Ok(())
            }
            Some(Modal::Value(..)) => self.form_key(key, App::commit_value),
            Some(Modal::Reconcile(..)) => self.form_key(key, App::commit_reconcile),
            Some(Modal::Bill(_)) => self.form_key(key, App::commit_bill),
            Some(Modal::RecurringTxn(_)) => self.form_key(key, App::commit_recurring_txn),
            Some(Modal::RecurringGoalEntry(_)) => self.form_key(key, App::commit_recurring_goal),
            Some(Modal::Account(_)) => self.form_key(key, App::commit_account),
            Some(Modal::PlanTransfers(_)) => {
                match key.code {
                    KeyCode::Esc => {
                        self.close_modal();
                        self.status = "cancelled".to_string();
                    }
                    KeyCode::Enter => self.commit_plan_transfers()?,
                    KeyCode::Left => {
                        if let Some(Modal::PlanTransfers(c)) = &mut self.modal {
                            c.step_date(week_step(key, Step::PREVIOUS_WEEK, Step::PREVIOUS));
                        }
                    }
                    KeyCode::Right => {
                        if let Some(Modal::PlanTransfers(c)) = &mut self.modal {
                            c.step_date(week_step(key, Step::NEXT_WEEK, Step::NEXT));
                        }
                    }
                    // The date is a text field, and the editing keys reach it
                    // here as they do in every form.
                    _ => {
                        if let Some(Modal::PlanTransfers(confirm)) = &mut self.modal {
                            confirm.edit(key);
                        }
                    }
                }
                Ok(())
            }
            Some(Modal::Fund(_)) => self.form_key(key, App::commit_fund),
            Some(Modal::FundValue(..)) => self.form_key(key, App::commit_fund_value),
            Some(Modal::BirthDate(_)) => self.form_key(key, App::commit_birth_date),
        }
    }

    /// `↑`/`↓` select and `Enter` or `Tab` accepts, but only while the popup
    /// has rows on screen — otherwise `Tab` moves fields and `Enter` commits.
    ///
    /// Gated on what the last draw fitted, not on the list being non-empty: a
    /// popup clipped off the bottom of a short terminal must not capture the
    /// `Enter` its form's title bar advertises as "save".
    ///
    /// Returns whether the key was consumed by the popup.
    fn popup_key(&mut self, key: KeyEvent) -> bool {
        if self.popup.visible() == 0 {
            return false;
        }
        match key.code {
            KeyCode::Up => self.popup.previous(),
            KeyCode::Down => self.popup.next(),
            KeyCode::Enter | KeyCode::Tab => {
                if let (Some(fields), Some(hit)) = (
                    self.modal.as_mut().and_then(Modal::fields_mut),
                    self.popup.selected(),
                ) {
                    fields.apply_suggestion(hit);
                }
                self.popup.clear();
            }
            _ => return false,
        }
        true
    }

    /// Every field form's keys. `commit` is what `Enter` runs, which is the
    /// only part that differs between them.
    ///
    /// `Esc` with the popup up dismisses the popup and keeps the form;
    /// otherwise it reaches the modal below and discards everything typed.
    /// Gated on `visible` rather than `is_open` for the same reason
    /// `popup_key` is: a popup whose rows were all clipped away captures no
    /// keys, so `Esc` must still close the modal there.
    fn form_key(&mut self, key: KeyEvent, commit: fn(&mut App) -> Result<()>) -> Result<()> {
        if self.popup.visible() > 0 && key.code == KeyCode::Esc {
            self.popup.clear();
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            self.close_modal();
            return Ok(());
        }
        if self.popup_key(key) {
            return Ok(());
        }
        if key.code == KeyCode::Enter {
            return commit(self);
        }
        let Some(fields) = self.modal.as_mut().and_then(Modal::fields_mut) else {
            return Ok(());
        };
        // Shift is the same nudge with a bigger step, on the key that already
        // means "move this", rather than a second key for one action. A
        // selector has no week to move and takes the direction alone, which is
        // `Step`'s to decide rather than this handler's.
        let week = key.modifiers.contains(KeyModifiers::SHIFT);
        let mut edited = false;
        match key.code {
            KeyCode::Tab => fields.next_field(),
            KeyCode::BackTab => fields.previous_field(),
            KeyCode::Left => fields.choice(if week {
                Step::PREVIOUS_WEEK
            } else {
                Step::PREVIOUS
            }),
            KeyCode::Right => fields.choice(if week { Step::NEXT_WEEK } else { Step::NEXT }),
            // Everything a text field answers -- the character itself, and
            // the `Ctrl` editing keys -- in one place, so a suggestion is
            // re-asked for on the presses that changed the text and no other.
            _ => edited = fields.edit(key) == Edit::Changed,
        }
        if edited {
            self.refresh_suggestions()?;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
            self.popup.clear();
        }
        Ok(())
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let [tab_area, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(
            Tabs::new(Screen::ALL.map(Screen::tab_label))
                .select(self.screen as usize)
                .divider("│"),
            tab_area,
        );

        match self.screen {
            Screen::Overview => {
                overview::render(frame, body, &self.overview, self.scrubbed_days() != 0)
            }
            // The viewport comes back out of the draw -- its height, so
            // `PageUp` and `PageDown` move by a screenful, and the row it
            // started at, so the next draw carries on from where this one left
            // the list. The autocomplete popup's drawn-row count comes back
            // the same way below.
            Screen::Cash | Screen::Credit => {
                let viewport = ledger_screen::render(frame, body, self.ledger(), self.today);
                self.ledger_mut().record_viewport(viewport);
            }
            Screen::Savings => {
                let viewport = savings_screen::render(frame, body, &self.savings);
                self.savings.record_viewport(viewport);
            }
            Screen::Planning => {
                let viewport = planning_screen::render(frame, body, &self.planning);
                self.planning.record_viewport(viewport);
            }
            Screen::Funds => {
                let viewport = fund_screen::render(frame, body, &self.funds);
                self.funds.record_viewport(viewport);
            }
            Screen::RecurringTxns => {
                let viewport = recurring_txn_screen::render(frame, body, &self.recurring_txn);
                self.recurring_txn.record_viewport(viewport);
            }
            Screen::RecurringGoals => {
                let viewport = recurring_goal_screen::render(frame, body, &self.recurring_goal);
                self.recurring_goal.record_viewport(viewport);
            }
            Screen::Accounts => {
                let viewport = accounts_screen::render(frame, body, &self.accounts);
                self.accounts.record_viewport(viewport);
            }
        }

        // Two paragraphs rather than one string: the app-wide keys sit
        // against the right edge, so they hold one place on every screen
        // whatever the screen's own keys cost. The split gives them exactly
        // their own width and leaves the rest to the left half, which is what
        // ratatui truncates from when a terminal is narrower than MIN_WIDTH --
        // an over-wide footer drops the last key the *screen* names rather
        // than `q quit`.
        let chrome = self.footer_chrome();
        let [keys, app_wide] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(chrome.chars().count() as u16),
        ])
        .areas(footer);
        frame.render_widget(Paragraph::new(self.footer()), keys);
        frame.render_widget(Paragraph::new(chrome), app_wide);

        // Runs before the next key is read, which is what keeps the popup's
        // cursor off a row this draw clipped away.
        let drawn = modal::render(frame, &mut self.modal, &self.popup);
        self.popup.set_visible(drawn);

        // Last, so the panel is above the modal rather than under it. The
        // extent comes back out of the draw for the same reason the worksheet's
        // page height does: only the draw knows how many rows a wrap produced.
        if let Some(help) = &mut self.help {
            let (lines, page) = help::render(frame, help);
            help.set_extent(lines, page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::setting::{self, key};
    use crate::db::{account, fund, recurring_goal, recurring_txn};
    use crate::money::Cents;
    use crate::test_support::day;
    use crate::tui::app::test_support::*;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::TxnField;
    use crate::tui::help::{self, Topic};
    use crate::tui::modal::{Confirm, Modal};
    use crate::tui::search::Search;
    use crate::tui::{MIN_WIDTH, worksheet as worksheet_screen};
    use chrono::Datelike;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Instant;

    /// The character the footer draws its caret on, which is the `/` box's.
    fn footer_caret(app: &App) -> String {
        use ratatui::style::Modifier;

        app.footer()
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// The same key with Shift held, which crossterm reports as the arrow
    /// plus a modifier rather than a code of its own.
    fn shift_press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::SHIFT));
    }

    /// The footer is the screen's keys, and a message borrows it rather than
    /// owning it: once it has been up long enough to read, the keys come
    /// back without the owner having to press anything to dismiss it.
    #[test]
    fn a_status_message_gives_the_footer_back_once_it_has_outlived_its_ttl() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));
        let keys = Topic::Savings.footer();
        assert_ne!(footer(&app), keys);

        app.expire_status_at(Instant::now() + STATUS_TTL);

        assert_eq!(app.status, "");
        assert_eq!(footer(&app), keys);
    }

    #[test]
    fn a_status_message_holds_the_footer_until_its_ttl_is_up() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));

        app.expire_status_at(Instant::now());

        assert!(app.status.contains("nothing to undo"), "{}", app.status);
    }

    /// The event loop redraws on this answer and on nothing else the clock
    /// can offer, so an expiry that reported nothing would leave a faded
    /// message on the footer until the next keystroke -- and an idle app
    /// reporting one every tick would be the unconditional draw back again.
    #[test]
    fn only_the_tick_that_drops_a_message_reports_a_change() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));

        assert!(!app.expire_status_at(Instant::now()), "not up yet");
        assert!(
            app.expire_status_at(Instant::now() + STATUS_TTL),
            "the message was dropped"
        );
        assert!(
            !app.expire_status_at(Instant::now() + STATUS_TTL),
            "nothing left to drop"
        );
    }

    /// A form's refusal is the only account of why it stayed open -- the
    /// modal does not repeat it -- so it holds the footer for as long as the
    /// question does.
    #[test]
    fn a_message_under_an_open_modal_does_not_fade() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "/0");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_some(), "the form must stay open");

        app.expire_status_at(Instant::now() + STATUS_TTL * 10);

        assert!(app.status.contains("divide by 0"), "{}", app.status);
    }

    /// The clock starts on the message, not on the key: a key that says
    /// nothing must not leave the previous message's deadline behind for the
    /// next expiry to act on.
    #[test]
    fn a_key_that_says_nothing_leaves_no_deadline_behind() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('1'));

        assert_eq!(app.status, "");
        assert!(app.status_until.is_none());
    }

    /// A week per press, both ways, and the drift the footer reports counts
    /// in days rather than presses.
    #[test]
    fn shift_and_an_arrow_scrub_the_overview_a_week_at_a_time() {
        let mut app = app();
        let baseline = app.dates.adhoc;

        shift_press(&mut app, KeyCode::Right);
        assert_eq!(app.adhoc, baseline + chrono::Duration::days(7));
        assert_eq!(app.scrubbed_days(), 7);

        shift_press(&mut app, KeyCode::Left);
        assert_eq!(app.adhoc, baseline);
        assert_eq!(app.scrubbed_days(), 0);

        shift_press(&mut app, KeyCode::Left);
        assert_eq!(app.adhoc, baseline - chrono::Duration::days(7));
        assert_eq!(app.scrubbed_days(), -7);
    }

    /// The unmodified arrow is untouched: one is a nudge and the other a
    /// bigger nudge, and they compose because both move the same date.
    #[test]
    fn a_plain_arrow_still_scrubs_a_single_day() {
        let mut app = app();
        let baseline = app.dates.adhoc;

        press(&mut app, KeyCode::Right);
        shift_press(&mut app, KeyCode::Right);
        assert_eq!(app.adhoc, baseline + chrono::Duration::days(8));
        assert_eq!(app.scrubbed_days(), 8);
    }

    /// The week scrub is the day scrub with a bigger step, so it reaches
    /// Planning the same way: `Excess (Actual)` is the checking balance at
    /// this date, and a screen quoting a different day than the column the
    /// owner just moved is the failure the shared `App::adhoc` prevents.
    #[test]
    fn a_week_scrub_moves_the_date_planning_quotes_too() {
        let mut app = planning_app_with_a_row_after_today();
        let before = app.planning.excess_actual();

        // One press clears the 18th, where the fixture's Rent lands.
        shift_press(&mut app, KeyCode::Right);
        assert_eq!(app.adhoc, day(2026, 8, 22));

        assert_eq!(app.planning.excess_actual(), before - Cents(1_000_000));
    }

    /// The scrub is view state and always was; with `ADHOC_DATE` retired
    /// there is nothing left for `Enter` to save.
    #[test]
    fn scrubbing_moves_only_the_view_and_enter_saves_nothing() {
        let mut app = app();
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.scrubbed_days(), 2);
        assert_eq!(app.adhoc, app.dates.adhoc + chrono::Duration::days(2));

        press(&mut app, KeyCode::Enter);

        assert_eq!(app.scrubbed_days(), 2, "Enter must not reset the scrub");
        assert_eq!(app.status, "");
    }

    /// Scrubbing back onto the baseline is still a press that did something,
    /// and `scrubbed +0d` is a drift that is not one. Saying where the date
    /// landed is what an arrow that undoes a scrub has to report.
    #[test]
    fn scrubbing_back_onto_the_baseline_says_where_the_date_landed() {
        let mut app = app();

        shift_press(&mut app, KeyCode::Right);
        shift_press(&mut app, KeyCode::Left);

        assert_eq!(app.scrubbed_days(), 0);
        assert_eq!(app.status, "back to Paycheck-Eve");
    }

    /// The drift reports the press that moved the date; it is not a property
    /// of the screen. So it borrows the footer the way every other message
    /// does and hands the Overview's keys back on its own -- a scrub left
    /// standing must not cost the keys for as long as it stands. What says
    /// the view is still off the baseline is the `*` on the column header,
    /// which is drawn from the scrub itself rather than from the message.
    #[test]
    fn the_scrub_drift_gives_the_footer_back_once_it_has_outlived_its_ttl() {
        let mut app = app();
        let keys = Topic::Overview.footer();

        shift_press(&mut app, KeyCode::Right);
        assert_eq!(footer(&app), "scrubbed +7d");

        app.expire_status_at(Instant::now() + STATUS_TTL);

        assert_eq!(footer(&app), keys);
        assert_eq!(app.scrubbed_days(), 7, "the message faded, not the scrub");
    }

    /// The derived date is the baseline the marker and the counter compare
    /// against, so an unscrubbed Overview reports no drift.
    #[test]
    fn an_unscrubbed_overview_shows_no_drift_against_the_derived_date() {
        let app = app();
        assert_eq!(app.scrubbed_days(), 0);
    }

    /// The tab that goes first when the bar overflows is the last one — so a
    /// longer label loses navigation rather than looking cramped. Asserting
    /// the last tab is fully drawn is what keeps that from happening quietly.
    #[test]
    fn every_tab_fits_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        let bar: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        for tab in Screen::ALL.map(Screen::tab_label) {
            assert!(bar.contains(tab), "{tab:?} is cut off: {bar:?}");
        }
    }

    /// The bar highlights the current tab by `self.screen as usize`, so a
    /// screen out of discriminant order in `ALL` would draw one label and
    /// underline another. The match is exhaustive, so a tenth screen stops
    /// this compiling until it is listed and counted here too.
    #[test]
    fn every_screen_is_listed_in_discriminant_order() {
        for (i, screen) in Screen::ALL.into_iter().enumerate() {
            match screen {
                Screen::Overview
                | Screen::Cash
                | Screen::Credit
                | Screen::Savings
                | Screen::Planning
                | Screen::Funds
                | Screen::RecurringGoals
                | Screen::RecurringTxns
                | Screen::Accounts => {}
            }
            assert_eq!(screen as usize, i, "{screen:?} is out of order in ALL");
        }
        assert_eq!(Screen::ALL.len(), 9);
    }

    /// The net under the whole application: every screen, drawn in a demo,
    /// with none of the fixture's own figures anywhere in the buffer. A new
    /// screen, or a new `format!` that formats a `Cents` itself instead of
    /// asking `tui::demo`, fails here rather than on a shared terminal.
    ///
    /// The fixture is the one with rows on every list, because an absence
    /// check over an empty table passes for free: `app()` has no funds, no
    /// recurring goals and no recurring transactions, which left three of the
    /// nine screens drawing nothing to catch. What holds that shut is the
    /// guard beside it, and it is asked of the **real** draw: a screen must be
    /// shown to have drawn one of these figures before being asked not to draw
    /// it scrambled. Asking it of the scrambled draw instead -- that the two
    /// differ -- says nothing, because every screen also masks the names on
    /// it, so the two would differ on a screen whose figures had stopped being
    /// masked entirely. Accounts is the one screen exempt: it draws a name, a
    /// band and a position, and no figure at all.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_leaves_no_figure_on_any_screen() {
        // A row's text is built once, when the screen loads, not redrawn per
        // frame -- so the two apps below have to load every screen under
        // their own demo state rather than sharing one app and flipping the
        // salt between draws of it.
        crate::demo::install(false);
        let mut real_app = app_with_two_rows_on_every_list();
        let reals: Vec<String> = "123456789"
            .chars()
            .map(|screen| {
                press(&mut real_app, KeyCode::Char(screen));
                drawn(&mut real_app)
            })
            .collect();

        crate::demo::install_with_salt(7);
        let mut app = app_with_two_rows_on_every_list();
        for (screen, real) in "123456789".chars().zip(reals) {
            press(&mut app, KeyCode::Char(screen));
            let scrambled = drawn(&mut app);
            for figure in DEMO_FIXTURE_FIGURES {
                assert!(
                    !scrambled.contains(figure),
                    "{figure} survived on screen {screen}:\n{scrambled}"
                );
            }
            assert!(
                screen == '9' || DEMO_FIXTURE_FIGURES.iter().any(|f| real.contains(f)),
                "screen {screen} drew none of the fixture's figures, so the check above passed for free:\n{real}"
            );
        }
    }

    /// Every figure `app_with_two_rows_on_every_list` puts in the database, as
    /// the screens would print it unscrambled. One list rather than one per
    /// sweep, so a row added to that fixture is covered by both.
    #[cfg(feature = "demo")]
    const DEMO_FIXTURE_FIGURES: [&str; 10] = [
        "1,000", "1,200", "14.99", "25.99", "15,000", "10,000", "100.00", "128", "30,000", "90,000",
    ];

    /// Every name and description `app_with_two_rows_on_every_list` puts in the
    /// database, as an ordinary run would print it.
    ///
    /// `Paycheck` and `Transfer` are deliberately absent: each is also one of
    /// the app's own words -- a Planning row, a form title -- so an absence
    /// check on either would fail on vocabulary rather than on a leak.
    #[cfg(feature = "demo")]
    const DEMO_FIXTURE_NAMES: [&str; 18] = [
        "Everyday",
        "Rainy Day",
        "Card One",
        "Card Two",
        "CHK",
        "SAV",
        "CC1",
        "CC2",
        "Whole Foods",
        "Rent",
        "Movies",
        "Batteries",
        "Vacation 2027",
        "Couch",
        "Utilities",
        "Gym",
        "Bonds",
        "Domestic",
    ];

    /// The net for names, and the Accounts screen is in it: it draws no figure
    /// at all, which is exactly why the figure sweep exempts it and this one
    /// must not.
    ///
    /// A fixed salt rather than a drawn one, so a failure is reproducible and a
    /// pseudonym cannot collide its way to a pass on one run in a thousand.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_leaves_no_name_on_any_screen() {
        crate::demo::install_with_salt(7);
        let mut app = app_with_two_rows_on_every_list();
        for screen in "123456789".chars() {
            press(&mut app, KeyCode::Char(screen));
            let drawn = drawn(&mut app);
            for name in DEMO_FIXTURE_NAMES {
                assert!(
                    !drawn.contains(name),
                    "{name} survived on screen {screen}:\n{drawn}"
                );
            }
        }
    }

    /// The other half of the sweep above: an absence check passes for free over
    /// a screen that drew nothing, so every screen must be shown to have drawn
    /// a pseudonym for an account it holds.
    ///
    /// Screen 7, Recurring Goals, is not in the sweep: a recurring goal has no
    /// account column at all, so no account name, real or masked, is ever on
    /// that screen -- an absence check there would pass on the strength of a
    /// screen that never had anything to hide. And the set checked against is
    /// every one of the fixture's four cash/credit accounts, in the name a
    /// screen names it under and the code one names it under -- Credit never
    /// draws the checking account and Savings never draws either card, so a
    /// check pinned to one account would fail on a screen that is masking
    /// correctly, and Recurring Transactions draws a code where the rest draw
    /// a name.
    #[cfg(feature = "demo")]
    #[test]
    fn every_screen_draws_the_pseudonym_of_an_account_it_holds() {
        crate::demo::install_with_salt(7);
        let mut app = app_with_two_rows_on_every_list();
        let pseudonyms: Vec<String> = [
            "Everyday",
            "Rainy Day",
            "Card One",
            "Card Two",
            "CHK",
            "SAV",
            "CC1",
            "CC2",
        ]
        .into_iter()
        .map(|name| crate::demo::text(name).to_string())
        .collect();
        for screen in "123489".chars() {
            press(&mut app, KeyCode::Char(screen));
            let drawn = drawn(&mut app);
            assert!(
                pseudonyms.iter().any(|p| drawn.contains(p)),
                "screen {screen} named no account, so the sweep passed for free:\n{drawn}"
            );
        }
    }

    /// A write reports what it wrote on the status line, which sits in the
    /// footer of whatever screen is open -- both the amount and the
    /// description it landed under.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amount_a_written_row_reports() {
        crate::demo::install_with_salt(7);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "76.54");
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Hardware");
        press(&mut app, KeyCode::Enter);

        assert!(!app.status.contains("76.54"), "{}", app.status);
        assert!(
            app.status.contains(&crate::demo::figure(Cents(7_654))),
            "{}",
            app.status
        );
        assert!(!app.status.contains("Hardware"), "{}", app.status);
        assert!(
            app.status
                .contains(&crate::demo::text("Hardware").to_string()),
            "{}",
            app.status
        );
    }

    /// The transfer form's write reports through the same seam
    /// `commit_txn_form`'s does: a transfer's description lands in the same
    /// `txn.description` column as an ordinary row's, and it is owner-typed
    /// rather than fixed to the `Transfer` prefill.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_transfers_own_description_on_the_status_line() {
        crate::demo::install_with_salt(7);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'a');
        ctrl_press(&mut app, 'k');
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "50");
        press(&mut app, KeyCode::Enter);

        assert!(!app.status.contains("Nest Egg"), "{}", app.status);
        assert!(
            app.status
                .contains(&crate::demo::text("Nest Egg").to_string()),
            "{}",
            app.status
        );
    }

    /// A confirmation names the row it is about to delete, and what makes a
    /// ledger row recognisable is its amount. The modal is drawn over the
    /// screen, so a figure that survives here survives in the middle of the
    /// terminal.
    ///
    /// A fixed salt rather than a drawn one, for the reason the name sweeps
    /// use one: a scrambled figure can land on `200.00` by chance, and a test
    /// whose answer changes run to run is one nobody can reproduce.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_leaves_no_figure_on_a_delete_confirmation() {
        crate::demo::install_with_salt(7);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));

        let drawn = drawn(&mut app);
        assert!(drawn.contains("Delete"), "no confirmation opened:\n{drawn}");
        // Every ledger row behind the modal is scrambled already, so the only
        // thing that can put this figure on screen is the label itself.
        assert!(!drawn.contains("200.00"), "the amount survived:\n{drawn}");
        assert!(drawn.contains("2026-08-12"), "the date must stay:\n{drawn}");
    }

    /// The same net one layer in: every key that opens a form or a worksheet
    /// over a row that carries a figure, with the scramble on and none of the
    /// fixture's figures reaching the buffer.
    ///
    /// The screen sweep above cannot see any of this -- it only ever draws
    /// screens `1`-`9` -- which is exactly how `BillField::Amount` came to be
    /// the one amount field this feature missed. A form prefills from the row
    /// it opens on, so a form is where a real figure is most likely to reach
    /// the screen, not least.
    ///
    /// Every pair asserts a modal actually opened first. A key that finds
    /// nothing to open on leaves the screen as it was and passes an absence
    /// check without ever drawing a form -- which is what `('2', 'r')` did,
    /// silently, until it was given the account filter `r` needs.
    ///
    /// The list is the name sweep's, pair for pair. Every modal that can
    /// draw one of the owner's words can draw one of the owner's figures
    /// beside it, and two lists free to drift apart are two chances for a
    /// modal to be opened by one sweep and not the other -- which is how the
    /// payday confirmation, `('5', 't')`, went unswept by either.
    ///
    /// A fixed salt rather than a drawn one, for the reason the name sweeps
    /// use one: a scrambled figure can land on `1,000` as easily as a
    /// pseudonym can land on a name, and a sweep whose answer changes run to
    /// run is one nobody can reproduce.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_leaves_no_figure_on_any_form_a_row_opens() {
        crate::demo::install_with_salt(7);
        for (screen, key) in [
            ('2', 'a'),
            ('2', 'e'),
            ('2', 'r'),
            ('2', 't'),
            ('4', 'e'),
            ('4', 'a'),
            ('4', 'A'),
            ('4', 'n'),
            ('4', 'c'),
            ('5', 'e'),
            ('5', 'E'),
            ('5', 'a'),
            ('5', 't'),
            ('6', 'e'),
            ('6', 'E'),
            ('7', 'a'),
            ('7', 's'),
            ('8', 'a'),
            ('9', 'e'),
        ] {
            // Screen 6 draws off the `fund` table and `s` on screen 7 off
            // `recurring_goal`; `planning_app` fills neither. Every other
            // screen here has its rows on the fixture that carries the bills
            // screen 5 needs.
            let mut app = match (screen, key) {
                ('6', _) | ('7', 's') => app_with_two_rows_on_every_list(),
                _ => planning_app(),
            };
            press(&mut app, KeyCode::Char(screen));
            match (screen, key) {
                // The bill rows are where the other keys on this screen
                // open; `t` needs no row under the cursor and is unharmed by
                // the cursor landing there.
                ('5', _) => {
                    select_first_bill(&mut app);
                }
                // `r` reconciles the one account a ledger is narrowed to, and
                // a ledger opens on `All`.
                ('2', 'r') => press(&mut app, KeyCode::Tab),
                _ => {}
            }
            press(&mut app, KeyCode::Char(key));

            assert!(
                app.modal.is_some(),
                "{key} opened nothing on screen {screen}, so the check below \
                 would pass without a form ever being drawn: {}",
                app.status
            );
            let drawn = drawn(&mut app);
            let figures: &[&str] = match (screen, key) {
                ('6', _) | ('7', 's') => &DEMO_FIXTURE_FIGURES,
                _ => &["1,200", "300.00", "1,000", "50,000", "5,000"],
            };
            for figure in figures {
                assert!(
                    !drawn.contains(figure),
                    "{figure} survived {key} on screen {screen}:\n{drawn}"
                );
            }
        }
    }

    /// The one thing a demo must never do: reach a write.
    ///
    /// `App::open_goal_edit` prefills the form from the *row the screen is
    /// holding*, so a pseudonym written into view state is a pseudonym `Enter`
    /// commits. The field draws one and the buffer holds the name.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_draws_a_pseudonym_in_a_name_field_and_commits_the_name() {
        use crate::tui::goal_form::GoalField;

        crate::demo::install_with_salt(7);
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // The undated goal, `Couch`, leads the container; `Vacation 2027` is
        // dated and follows it.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Goal(form)) = &app.modal else {
            panic!("e opened nothing: {}", app.status);
        };
        let drawn = form.display(GoalField::Name).plain_text();
        assert_ne!(drawn, "Vacation 2027");
        assert_eq!(drawn.chars().count(), "Vacation 2027".chars().count());
        assert_eq!(form.commit().unwrap().name, "Vacation 2027");
    }

    /// The name sweep one layer in. A form prefills from the row it opens on,
    /// so a form is where a real name is most likely to reach the screen, not
    /// least -- which is how `BillField::Amount` came to be the one amount
    /// field the figures first missed.
    ///
    /// `Mortgage` and `HOA` are dropped from `planning_app`'s list of names to
    /// check for: screen 5 draws the app's own heading `Mortgage + HOA`
    /// (`src/plan_rows.rs`, a literal, never masked) behind the modal, so an
    /// absence check on either would fail on vocabulary rather than on a
    /// leak -- the same reason `Paycheck` and `Transfer` are absent from
    /// `DEMO_FIXTURE_NAMES`. `Coworking`, the fixture's third bill, stays on
    /// the list: `select_first_bill` always lands on `Mortgage`, so no
    /// iteration here ever opens a form on `Coworking` -- what actually
    /// covers it is `tui::planning::build`'s `bills` closure, which masks
    /// every bill's label before the table behind the modal is ever drawn,
    /// `Coworking` included. `BillField::Label`'s own form-field masking is
    /// covered by `a_demo_scrambles_the_amount_capital_e_opens_a_bill_on` and
    /// `a_demo_scrambles_the_label_lowercase_e_opens_a_bill_on`
    /// (`src/tui/app/planning.rs`), both against `Mortgage`.
    ///
    /// Two pairs open a modal that is not a form, and both are here because a
    /// modal is a modal to a viewer: `('5', 't')` is the payday confirmation,
    /// which names the destination account every transfer lands in and is the
    /// last thing drawn before real money moves, and `('7', 's')` is the
    /// recurring-goal picker, which lists the catalog by name.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_leaves_no_name_on_any_form_a_row_opens() {
        crate::demo::install_with_salt(7);
        for (screen, key) in [
            ('2', 'a'),
            ('2', 'e'),
            ('2', 'r'),
            ('2', 't'),
            ('4', 'e'),
            ('4', 'a'),
            ('4', 'A'),
            ('4', 'n'),
            ('4', 'c'),
            ('5', 'e'),
            ('5', 'E'),
            ('5', 'a'),
            ('5', 't'),
            ('6', 'e'),
            ('6', 'E'),
            ('7', 'a'),
            ('7', 's'),
            ('8', 'a'),
            ('9', 'e'),
        ] {
            // Screen 6 draws off the `fund` table and `s` on screen 7 off
            // `recurring_goal`; `planning_app` fills neither.
            let mut app = match (screen, key) {
                ('6', _) | ('7', 's') => app_with_two_rows_on_every_list(),
                _ => planning_app(),
            };
            press(&mut app, KeyCode::Char(screen));
            match (screen, key) {
                // The bill rows are where the other keys on this screen
                // open; `t` needs no row under the cursor and is unharmed by
                // the cursor landing there.
                ('5', _) => {
                    select_first_bill(&mut app);
                }
                ('2', 'r') => press(&mut app, KeyCode::Tab),
                _ => {}
            }
            press(&mut app, KeyCode::Char(key));
            assert!(
                app.modal.is_some(),
                "{key} opened nothing on screen {screen}, so the check below \
                 would pass without a form ever being drawn: {}",
                app.status
            );
            let drawn = drawn(&mut app);
            // `planning_app`'s own names, and the fixture names for screen 6.
            //
            // `Housing` and `Mom & Dad` are goals in the fixture too, and are
            // deliberately absent for the same reason `Mortgage`/`HOA` are:
            // each is also a Planning line's own word --
            // `Line::CurrentHousing.label()` is `"Housing"`
            // (`src/plan_line.rs`) and `Line::MomAndDad.label()` is
            // `"Mom & Dad"`, drawn literally by `plan_rows::rows` -- so an
            // absence check on either would fail on vocabulary rather than on
            // a leak. `Roth IRA` has no such collision: the gate's own word
            // is the shorter `Gate::Roth.label()`, `"Roth"`
            // (`src/gate.rs`), which does not contain the goal's full name,
            // so it is checked here rather than excluded.
            let names: &[&str] = match (screen, key) {
                ('6', _) | ('7', 's') => &DEMO_FIXTURE_NAMES,
                _ => &[
                    "Everyday",
                    "Rainy Day",
                    "Brokerage",
                    "Coworking",
                    "Bill Payments",
                    "Roth IRA",
                    "Home Down Payment",
                    "Emergency Savings",
                    "Dropbox",
                ],
            };
            for name in names {
                assert!(
                    !drawn.contains(name),
                    "{name} survived {key} on screen {screen}:\n{drawn}"
                );
            }
        }
    }

    /// A status line is on screen as surely as a column is, and this one
    /// quotes the figure that was just typed back at the owner -- and names
    /// the account it was typed for, bypassing `Account`, so the mask has to
    /// be applied at the call site rather than inherited from a colored
    /// display.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_target_a_reconciliation_reports() {
        crate::demo::install_with_salt(7);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert!(!app.status.contains("1,200"), "{}", app.status);
        assert!(
            app.status
                .contains(&crate::demo::figure(Cents::from_dollars(1_200))),
            "{}",
            app.status
        );
        assert!(!app.status.contains("Everyday"), "{}", app.status);
        assert!(
            app.status
                .contains(&crate::demo::text("Everyday").to_string()),
            "{}",
            app.status
        );

        // The buffer still holds the real figure: reopening and pressing
        // Enter must not write the scrambled one back.
        press(&mut app, KeyCode::Char('r'));
        let Some(Modal::Reconcile(_, form)) = &app.modal else {
            panic!("r must reopen the form: {:?}", app.status);
        };
        assert_eq!(form.value(), "1,200.00");
    }

    /// The whole footer line as it is drawn: the screen's own keys, the gap,
    /// and the app-wide keys against the right edge. `App::footer` is only
    /// the left half now, so a width test that measured it alone would stop
    /// seeing the twenty columns the chrome holds.
    fn footer_line(app: &App) -> String {
        [footer(app), app.footer_chrome()].join(" · ")
    }

    /// The footer row as the terminal receives it, trailing spaces and all.
    fn footer_row(app: &mut App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..MIN_WIDTH).map(|x| buffer[(x, 23)].symbol()).collect()
    }

    /// `1-9 screens · q quit` ends the line rather than following the last key
    /// the screen names, so the two keys every screen answers are found in one
    /// place whatever the screen in front of them costs.
    #[test]
    fn the_app_wide_keys_sit_against_the_right_edge_of_every_screen() {
        let mut app = app();
        for screen in ['1', '2', '3', '4', '5', '6', '7', '8'] {
            press(&mut app, KeyCode::Char(screen));
            // A screen key can leave a status message, which takes the line.
            press(&mut app, KeyCode::Down);
            let row = footer_row(&mut app);
            assert!(
                row.ends_with("1-9 screens · q quit"),
                "screen {screen}: {row:?}"
            );
            assert!(
                row.contains("  1-9 screens"),
                "screen {screen} has no gap before the chrome: {row:?}"
            );
        }
    }

    /// A search box must not advertise `1-9`: a digit typed there is part of
    /// the needle -- 1234 finds a row of $1,234.56 -- and naming a screen
    /// switch beside a box that answers digits with text is the one place the
    /// chrome would be a lie.
    #[test]
    fn a_search_box_shows_no_app_wide_keys() {
        let mut app = app();
        for screen in ['2', '3', '4'] {
            press(&mut app, KeyCode::Char(screen));
            press(&mut app, KeyCode::Char('/'));
            assert_eq!(app.footer_chrome(), "", "screen {screen}");
            let row = footer_row(&mut app);
            assert!(!row.contains("1-9 screens"), "screen {screen}: {row:?}");
            press(&mut app, KeyCode::Esc);
            assert_eq!(app.footer_chrome(), help::chrome(), "screen {screen}");
        }
    }

    /// The chrome may only be shown where `App::dispatch` actually answers
    /// those keys, and it answers neither under a modal: the `modal_key`
    /// return sits above the `q` and `1-9` arms, so every form, confirm,
    /// worksheet, picker and chooser makes both of them dead. The footer row
    /// is drawn before the popup and no popup covers it, so a chrome that
    /// stayed would be read off a line the modal is sitting on top of.
    #[test]
    fn no_modal_shows_the_app_wide_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        assert!(app.modal.is_some());
        assert_eq!(app.footer_chrome(), "");
        assert!(!footer_row(&mut app).contains("1-9 screens"));
    }

    /// The worksheet and the destination chooser host the app's other two
    /// search boxes, and a digit typed in either goes into the needle -- in
    /// the worksheet's case `/` then a digit is a whole other key, the one
    /// that takes 1/N of the pot.
    #[test]
    fn a_modal_hosted_search_box_shows_no_app_wide_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('C'));
        assert!(worksheet(&app).is_searching());
        assert_eq!(app.footer_chrome(), "");
        assert!(!footer_row(&mut app).contains("1-9 screens"));
    }

    /// The panel is the third context whose keys `dispatch` answers ahead of
    /// the app-wide ones -- it takes Esc and the scroll keys and drops the
    /// rest, so `q` there quits nothing.
    #[test]
    fn the_open_help_panel_shows_no_app_wide_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help.is_some());
        assert_eq!(app.footer_chrome(), "");
        assert!(!footer_row(&mut app).contains("1-9 screens"));
    }

    /// A status message borrows the whole line for `STATUS_TTL` and gives it
    /// back. Half a message and half a key list reads as neither, and the keys
    /// are live either way.
    #[test]
    fn a_status_message_takes_the_whole_footer_line() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        assert!(!app.status.is_empty());
        assert_eq!(app.footer_chrome(), "");
        assert!(!footer_row(&mut app).contains("1-9 screens"));

        // The next key clears the message, and the keys come back with it.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.footer_chrome(), help::chrome());
    }

    /// Nothing pinned the Planning footer's width before, and it drifted
    /// fifteen columns past the width it was laid out for without anyone
    /// noticing. `p unpin` is two
    /// characters longer than `p pin`, so both states have to be checked --
    /// the longer one is the one that can silently push the line over.
    #[test]
    fn the_planning_footers_key_hints_fit_the_minimum_width_in_both_pin_states() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let unpinned = footer_line(&app);
        assert!(
            unpinned.chars().count() <= usize::from(MIN_WIDTH),
            "{} ({})",
            unpinned,
            unpinned.chars().count()
        );

        press(&mut app, KeyCode::Char('p'));
        // `p` leaves a "pinned ..." message in the status line, which
        // `footer` shows ahead of the key hints -- a harmless no-op key
        // clears it without touching the pin, so the hint line underneath is
        // what gets measured.
        press(&mut app, KeyCode::Down);
        let pinned = footer_line(&app);
        assert!(pinned.contains("unpin"), "{pinned}");
        assert!(
            pinned.chars().count() <= usize::from(MIN_WIDTH),
            "{} ({})",
            pinned,
            pinned.chars().count()
        );
    }

    /// The ledger footer is the longest of the eight, and grouping `a/t/p`
    /// under one word is what bought it room for the app-wide keys. Cash is
    /// the state that gets measured: it is the Credit footer plus `t`, which
    /// is one key inside that group.
    #[test]
    fn the_ledger_footers_key_hints_fit_the_minimum_width() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        let footer = footer_line(&app);
        assert!(footer.contains("r target"), "{footer}");
        assert!(
            footer.chars().count() <= usize::from(MIN_WIDTH),
            "{} ({})",
            footer,
            footer.chars().count()
        );
    }

    /// A key nothing advertises is a key nobody presses.
    #[test]
    fn the_savings_footer_advertises_the_month_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert!(footer(&app).contains("[ ] month"));
        assert!(footer(&app).contains("Esc clear"));
    }

    /// The ledgers answer Esc with the rest of them, under the word every
    /// screen with a filter uses. What it clears *to* differs -- today's
    /// window here, All on Savings -- and that is the panel's to say.
    #[test]
    fn the_ledger_footer_advertises_esc_as_clearing() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        assert!(footer(&app).contains("Esc clear"));
    }

    /// Press a screen key and read the footer it produces.
    ///
    /// `App::footer` returns the status line when one is set, so the press has to
    /// be the last thing that happened.
    fn footer_of(app: &mut App, screen: char) -> String {
        press(app, KeyCode::Char(screen));
        footer(app)
    }

    /// The eight footers as they read, with Planning's leading `↑/↓ constant`
    /// deliberately absent: the six scroll keys are uniform across every list,
    /// so no footer names them. The app-wide keys are absent too -- they are
    /// `help::chrome`, drawn against the right edge rather than joined onto
    /// the end of these. Every line here is user-visible and must survive byte
    /// for byte.
    #[test]
    fn every_screen_footer_reads_as_it_always_has() {
        let mut app = app();
        assert_eq!(footer_of(&mut app, '1'), "←/→ scrub · Shift+←/→ week");
        assert_eq!(
            footer_of(&mut app, '2'),
            "Tab acct · [ ] month · Esc clear · / search · r target · a/t/p money · e edit · d delete"
        );
        assert_eq!(
            footer_of(&mut app, '3'),
            "Tab acct · [ ] month · Esc clear · / search · r target · a/p money · e edit · d delete"
        );
        assert_eq!(
            footer_of(&mut app, '4'),
            "Tab acct · [ ] month · Esc clear · / search · a/A/i allocate · n/e/c/K/J goal · f fave · U undo"
        );
        assert_eq!(
            footer_of(&mut app, '5'),
            "e edit · E/a/d bill · t transfers · Enter why · p pin"
        );
        assert_eq!(
            footer_of(&mut app, '6'),
            "a add · e value · E edit · d delete"
        );
        assert_eq!(
            footer_of(&mut app, '7'),
            "[ ] month · Esc clear · a add · e edit · d delete · s savings"
        );
        assert_eq!(
            footer_of(&mut app, '8'),
            "a add · e edit · d delete · g regen · G all · x extend · P paycheck"
        );
    }

    /// The pin is the one footer entry whose word changes with state. The table
    /// carries "pin"; this is what turns it into "unpin", and nothing else in the
    /// line may be touched on the way.
    #[test]
    fn the_planning_footer_offers_unpin_only_once_a_plan_is_pinned() {
        let mut app = app();
        press(&mut app, KeyCode::Char('5'));
        let unpinned = footer(&app);
        assert!(unpinned.contains("p pin"), "{unpinned}");
        assert!(
            !unpinned.contains("P unpin"),
            "offers to clear a pin that is not there: {unpinned}"
        );

        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('5'));
        let pinned = footer(&app);
        assert_eq!(
            pinned,
            unpinned.replacen("p pin", "p pin · P unpin", 1),
            "{pinned}"
        );
    }

    #[test]
    fn question_mark_opens_the_panel_on_every_screen() {
        for (key, topic) in [
            ('1', Topic::Overview),
            ('2', Topic::Ledger),
            ('3', Topic::Ledger),
            ('4', Topic::Savings),
            ('5', Topic::Planning),
            ('6', Topic::Funds),
            ('7', Topic::RecurringGoals),
            ('8', Topic::RecurringTxns),
        ] {
            let mut app = app();
            press(&mut app, KeyCode::Char(key));
            press(&mut app, KeyCode::Char('?'));
            assert!(
                open_on(&app, topic),
                "screen {key} opened {:?}",
                app.help.as_ref().map(|h| h.topic())
            );
        }
    }

    #[test]
    fn esc_closes_the_panel_and_leaves_the_screen_where_it_was() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Esc);

        assert!(app.help.is_none());
        assert_eq!(app.screen, Screen::Savings);
    }

    /// A panel that `q` quits out of, or that `3` navigates away from, loses your
    /// place for a keystroke that was almost certainly meant for the screen behind
    /// it.
    #[test]
    fn the_open_panel_swallows_every_key_it_does_not_use() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('?'));

        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('a'));

        assert!(!app.should_quit());
        assert_eq!(app.screen, Screen::Savings);
        assert!(app.help.is_some());
        assert!(app.modal.is_none(), "a swallowed key opened a form");
    }

    /// The box has to show where the next keystroke lands, or `Ctrl`+`A` is
    /// a key with nothing on screen to say what it did.
    #[test]
    fn a_search_box_draws_its_caret_where_the_caret_is() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "rent");
        ctrl_press(&mut app, 'a');

        assert!(footer(&app).starts_with("/rent"), "{}", footer(&app));
        assert_eq!(footer_caret(&app), "r");
    }

    /// The footer is the box; the title only reports what the list is
    /// narrowed to. Two carets on one screen would leave the box ambiguous,
    /// so the echo in the title carries none even while the box is open.
    ///
    /// At the end of the needle the caret sits on the space past it, which is
    /// the one place it costs a column.
    #[test]
    fn the_title_echoes_the_needle_without_a_caret() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "rent");

        let title = app.ledger().title().plain_text();
        assert!(title.ends_with("/rent"), "{title}");
        assert!(footer(&app).starts_with("/rent "), "{}", footer(&app));
        assert_eq!(footer_caret(&app), " ");
    }

    /// A search box is a text field, and a search may legitimately be for a
    /// question mark.
    #[test]
    fn a_search_box_types_a_question_mark_and_opens_the_panel_only_on_f1() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "?");

        assert!(app.help.is_none());
        assert_eq!(app.ledger().search(), "?");

        press(&mut app, KeyCode::F(1));
        assert!(open_on(&app, Topic::LedgerSearch));
    }

    /// `?` is a character in a form, so F1 is the way in. Both halves matter: the
    /// typed one must reach the field, and F1 must not.
    #[test]
    fn a_form_types_a_question_mark_and_opens_the_panel_only_on_f1() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "why?");

        assert!(app.help.is_none(), "? opened the panel instead of typing");
        assert_eq!(form(&app).description(), "why?");

        press(&mut app, KeyCode::F(1));
        assert!(open_on(&app, Topic::SuggestForm));
    }

    /// `Ctrl` means editing the text under the caret, so on a screen -- where
    /// there is no caret -- it must reach nothing at all.
    ///
    /// The hazard is a hand reaching for `Ctrl`+`F` a beat after `Esc` closed
    /// a form: as a bare `Char` it is the `f` that marks a goal, and the
    /// write would be in the database before anything on screen said so.
    #[test]
    fn a_ctrl_letter_reaches_no_screen_operator() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        ctrl_press(&mut app, 'f');
        assert_eq!(
            savings_favorites(&app),
            vec![false, false],
            "Ctrl+F marked a goal"
        );

        for c in ['a', 'e', 'c', 'n', 'i', 'A', 'U'] {
            ctrl_press(&mut app, c);
            assert!(app.modal.is_none(), "Ctrl+{c} opened a modal");
        }
    }

    /// The same rule over the keys `dispatch` answers itself, which are the
    /// ones live on every screen at once: `Ctrl`+`Q` is a hand short of the
    /// quit that discards nothing but is still not what was asked for.
    #[test]
    fn a_ctrl_letter_reaches_none_of_the_app_wide_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        ctrl_press(&mut app, 'q');
        assert!(!app.should_quit(), "Ctrl+Q quit the app");

        ctrl_press(&mut app, '2');
        ctrl_press(&mut app, '6');
        assert_eq!(app.screen, Screen::Savings, "a Ctrl+digit switched screens");
    }

    /// `Alt` is bound nowhere, and an unbound modifier is dropped for the
    /// reason a `Ctrl` is -- on a screen as in a buffer. `Alt`+`F` is the
    /// word motion a hand that has just learned `Ctrl`+`F` reaches for next.
    #[test]
    fn an_alt_letter_reaches_no_screen_operator() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));

        assert_eq!(
            savings_favorites(&app),
            vec![false, false],
            "Alt+F marked a goal"
        );
    }

    /// The confirm dialogs' second carve-out from "any key but `y` cancels",
    /// beside the `?` above: a modified character is not a key the app reads
    /// anywhere, so it neither commits the delete nor throws it away. `d` and
    /// `y` sit a finger apart, and the row a `Ctrl`+`Y` deleted would have no
    /// undo.
    #[test]
    fn a_ctrl_y_neither_commits_a_confirm_dialog_nor_cancels_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));

        ctrl_press(&mut app, 'y');

        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"]
        );
        assert!(
            matches!(
                app.modal,
                Some(Modal::Confirm {
                    action: Confirm::DeleteTxn(_),
                    ..
                })
            ),
            "the dialog is gone"
        );
    }

    /// The `Ctrl` editing keys, in the box they are hardest to get right: a
    /// form field, where the same arrow that moves this caret steps a date
    /// one field away.
    #[test]
    fn ctrl_a_puts_the_next_keystroke_at_the_start_of_a_form_field() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "grocery");
        ctrl_press(&mut app, 'a');
        type_str(&mut app, "big ");

        assert_eq!(form(&app).description(), "big grocery");
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_caret_in_a_form_field() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "weekly grocery run");
        ctrl_press(&mut app, 'w');

        assert_eq!(form(&app).description(), "weekly grocery ");
    }

    /// A `Ctrl` nobody has bound used to arrive as its bare letter, so
    /// `Ctrl`+`C` typed a `c` into whatever field had the focus.
    #[test]
    fn a_ctrl_letter_with_no_binding_does_not_type_its_letter_into_a_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "rent");
        ctrl_press(&mut app, 'c');

        assert_eq!(form(&app).description(), "rent");
    }

    /// `←`/`→` act on the field under the caret: a text field moves the
    /// caret, where a date field steps a day and a selector cycles.
    #[test]
    fn an_arrow_moves_the_caret_in_a_text_field() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "rent");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        type_str(&mut app, "!");

        assert_eq!(form(&app).description(), "re!nt");
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_caret_in_a_search_box() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "weekly grocery run");
        ctrl_press(&mut app, 'w');

        assert_eq!(app.ledger().search(), "weekly grocery ");
    }

    /// A search box has no date and no selector, so the arrows are the
    /// caret's there with nothing to share them with.
    #[test]
    fn an_arrow_moves_the_caret_in_a_search_box() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "rent");
        press(&mut app, KeyCode::Left);
        type_str(&mut app, "!");

        assert_eq!(app.ledger().search(), "ren!t");
    }

    /// The worksheet is not a text field: two of its three focuses drop everything
    /// but digits, and the third is a date. It has more keys to explain than any
    /// other context, so `?` reaches them directly -- from every focus, since the
    /// topic does not depend on which one is active.
    #[test]
    fn the_worksheet_opens_the_panel_on_a_plain_question_mark_from_every_focus() {
        for tabs in 0..3 {
            let mut app = app();
            press(&mut app, KeyCode::Char('4'));
            press(&mut app, KeyCode::Char('A'));
            assert!(
                matches!(app.modal, Some(Modal::Worksheet(_))),
                "no worksheet opened"
            );
            for _ in 0..tabs {
                press(&mut app, KeyCode::Tab);
            }

            press(&mut app, KeyCode::Char('?'));
            assert!(open_on(&app, Topic::Worksheet), "after {tabs} tabs");
        }
    }

    /// `/` waits for the next key to decide between a fraction and a filter, so
    /// that key is data either way.
    #[test]
    fn a_worksheet_waiting_on_a_slash_types_the_question_mark_into_a_filter() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        // `/` is the slash operator only with the line list focused. On Amount
        // it is a typed character that is not a digit, and so dropped; on Date
        // it lands in the date.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('?'));

        assert!(
            app.help.is_none(),
            "? opened the panel instead of filtering"
        );
        let sheet = worksheet(&app);
        assert_eq!(sheet.search(), "?");
    }

    /// The confirm dialogs cancel on any key but `y`. `?` is the one carve-out: a
    /// question mark that silently threw away a pending delete would be a worse
    /// surprise than an exception in the rule.
    #[test]
    fn question_mark_on_a_confirm_dialog_opens_help_without_cancelling() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(
            app.modal,
            Some(Modal::Confirm {
                action: Confirm::DeleteTxn(_),
                ..
            })
        ));

        press(&mut app, KeyCode::Char('?'));
        assert!(open_on(&app, Topic::Confirm));

        press(&mut app, KeyCode::Esc);
        assert!(app.help.is_none());
        assert!(
            matches!(
                app.modal,
                Some(Modal::Confirm {
                    action: Confirm::DeleteTxn(_),
                    ..
                })
            ),
            "Esc closed the dialog as well as the panel"
        );
    }

    /// Opened over a form, the panel describes the form -- what is live -- rather
    /// than the screen it is drawn over.
    #[test]
    fn the_panel_describes_the_modal_not_the_screen_behind_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::F(1));

        assert!(open_on(&app, Topic::Form));
    }

    /// Every key a screen handler matches on must appear in that screen's table,
    /// and every key a table names must be one a handler matches.
    ///
    /// Written by reading the `match` arms of `overview_key`, `ledger_key`,
    /// `savings_key`, `planning_key`, `funds_key`, `recurring_txn_key` and
    /// `recurring_goal_key`. Nothing but this test ties the two together --
    /// the same guarantee the schema's `CHECK` lists have -- so a key added
    /// to a handler must be added here and to its table.
    ///
    /// `q` and `1`-`8` are absent because no screen handler matches them:
    /// `dispatch` answers them for all eight screens at once, and
    /// `the_app_wide_keys_work_from_every_screen` is what holds them up.
    #[test]
    fn every_key_a_screen_handler_matches_appears_in_its_table() {
        let handlers: [(Topic, &[&str]); 8] = [
            (Topic::Overview, &["←/→", "Shift+←/→"]),
            (
                Topic::Ledger,
                &[
                    "[ ]", "Esc", "Tab", "BackTab", "/", "r", "a", "t", "p", "e", "d",
                ],
            ),
            (
                Topic::Savings,
                &[
                    "Tab", "BackTab", "[ ]", "Esc", "/", "a", "A", "i", "e", "c", "n", "K", "J",
                    "f", "U",
                ],
            ),
            (
                Topic::Planning,
                &["e", "a", "E", "d", "t", "Enter", "p", "P"],
            ),
            (Topic::Funds, &["a", "e", "E", "d"]),
            (Topic::RecurringTxns, &["a", "e", "d", "g", "G", "x", "P"]),
            (Topic::RecurringGoals, &["[ ]", "Esc", "a", "e", "d", "s"]),
            (Topic::Accounts, &["a", "e"]),
        ];
        assert_documented(&handlers);
    }

    /// `q` and `1`-`9` are app-wide chrome: `dispatch` matches them above every
    /// screen handler, so they reach the same effect from all nine screens and
    /// no table repeats them. Delete either arm and the keys do nothing while
    /// nine footers still offer them -- which is what this catches.
    #[test]
    fn the_app_wide_keys_work_from_every_screen() {
        let screens: [(char, Screen); 9] = [
            ('1', Screen::Overview),
            ('2', Screen::Cash),
            ('3', Screen::Credit),
            ('4', Screen::Savings),
            ('5', Screen::Planning),
            ('6', Screen::Funds),
            ('7', Screen::RecurringGoals),
            ('8', Screen::RecurringTxns),
            ('9', Screen::Accounts),
        ];
        let mut roaming = app();
        for (from, _) in screens {
            for (to, screen) in screens {
                press(&mut roaming, KeyCode::Char(from));
                press(&mut roaming, KeyCode::Char(to));
                assert_eq!(roaming.screen, screen, "{from} then {to}");
            }
        }
        for (from, _) in screens {
            let mut app = app();
            press(&mut app, KeyCode::Char(from));
            assert!(!app.should_quit(), "screen {from} quit on its own");
            press(&mut app, KeyCode::Char('q'));
            assert!(app.should_quit(), "q does not quit from screen {from}");
        }
    }

    /// The tab bar highlights by `Screen as usize` and labels each tab with the
    /// digit that reaches it, so the digit and the discriminant are one fact
    /// stated in two places. A screen inserted without renumbering the other
    /// half highlights the wrong tab, which nothing else here would catch.
    #[test]
    fn each_screen_key_selects_the_tab_the_bar_labels_with_it() {
        for (index, digit) in ('1'..='9').enumerate() {
            let screen = Screen::from_key(digit).expect("no screen for a tab-bar digit");
            assert_eq!(screen as usize, index, "key {digit}");
        }
        assert_eq!(Screen::from_key('0'), None);
        assert_eq!(Screen::from_key('a'), None);
    }

    /// The same check for the contexts a screen key cannot reach: the two modals
    /// with a cursor, the confirm dialogs, the two field forms, the three
    /// search boxes, and the transfer-confirmation dialog.
    ///
    /// Written by reading the `match` arms of `worksheet_key`, `picker_key`,
    /// `destination_key`, `form_key` -- with `popup_key`, which it delegates to --
    /// `modal_key`'s confirm arms and its `Modal::PlanTransfers` arm, and the
    /// three `search::search_key` calls: the two branches `dispatch` opens with
    /// and the one `destination_key` does. Three families of arm are
    /// deliberately not listed:
    ///
    /// - the six scroll keys, which `cursor::scroll_key` answers uniformly on
    ///   every list and no table names;
    /// - the `KeyCode::Char(c)` catch-alls that type into a field, which are what
    ///   a form or a search box is *for* rather than keys with names. The
    ///   worksheet's `KeyCode::Backspace` arm joins this family too, narrowly:
    ///   typing a digit into an amount and backspacing one out of it are the
    ///   same text-editing mechanics as typing into a form or a search box, and
    ///   the panel does not spend a row on either. Backspace keeps its own
    ///   entry everywhere else it appears -- the two field-form topics, the
    ///   three search topics, and the transfer confirmation -- where it is a
    ///   named, explained key rather than typing. The `Ctrl` editing keys those
    ///   same catch-alls now carry into `text::edit_key` *are* listed, under
    ///   `help::EDITING`'s one key string: eight keys nobody would guess from
    ///   watching a character appear;
    /// - `?` and `F1`, which `dispatch` answers before any of these handlers see
    ///   them. A table names the one that is surprising: `F1` where `?` would be
    ///   typed instead, and `?` on the confirm dialogs, where every other key
    ///   cancels.
    #[test]
    fn every_key_a_modal_handler_matches_appears_in_its_table() {
        let handlers: [(Topic, &[&str]); 11] = [
            (
                Topic::Worksheet,
                &[
                    "Ctrl+A/E/B/F/W/U/K/D",
                    "Tab",
                    "BackTab",
                    "←/→",
                    "Shift+←/→",
                    "Space",
                    "*",
                    "-",
                    "z",
                    "s",
                    "w",
                    "/N",
                    "Enter",
                    "Esc",
                ],
            ),
            (Topic::Picker, &["Space", "Enter", "Esc"]),
            (Topic::Destination, &["/", "Enter", "Esc"]),
            (
                Topic::DestinationSearch,
                &["Ctrl+A/E/B/F/W/U/K/D", "Enter", "Esc", "Backspace", "F1"],
            ),
            (Topic::Confirm, &["y", "any", "?"]),
            (
                Topic::Form,
                &[
                    "Ctrl+A/E/B/F/W/U/K/D",
                    "Tab",
                    "BackTab",
                    "Backspace",
                    "←/→",
                    "Shift+←/→",
                    "Enter",
                    "Esc",
                    "F1",
                ],
            ),
            (
                Topic::SuggestForm,
                &[
                    "Ctrl+A/E/B/F/W/U/K/D",
                    "Tab",
                    "BackTab",
                    "Backspace",
                    "↑/↓",
                    "←/→",
                    "Shift+←/→",
                    "Enter",
                    "Esc",
                    "F1",
                ],
            ),
            (
                Topic::LedgerSearch,
                &["Ctrl+A/E/B/F/W/U/K/D", "Enter", "Esc", "Backspace", "F1"],
            ),
            (
                Topic::SavingsSearch,
                &["Ctrl+A/E/B/F/W/U/K/D", "Enter", "Esc", "Backspace", "F1"],
            ),
            (
                Topic::WorksheetSearch,
                &["Ctrl+A/E/B/F/W/U/K/D", "Enter", "Esc", "Backspace", "F1"],
            ),
            (
                Topic::PlanTransfers,
                &[
                    "Ctrl+A/E/B/F/W/U/K/D",
                    "Esc",
                    "←/→",
                    "Shift+←/→",
                    "Enter",
                    "Backspace",
                ],
            ),
        ];
        assert_documented(&handlers);
    }

    /// Both directions, for each topic: a handled key its table omits, and a
    /// documented key no handler matches.
    fn assert_documented(handlers: &[(Topic, &[&str])]) {
        for (topic, matched) in handlers {
            let named: Vec<&str> = topic.keys().iter().map(|e| e.key).collect();
            for key in *matched {
                assert!(
                    named.contains(key),
                    "{topic:?} handles {key:?} but does not explain it"
                );
            }
            for key in &named {
                assert!(
                    matched.contains(key),
                    "{topic:?} explains {key:?} but no handler matches it"
                );
            }
        }
    }

    /// The six scroll keys appear in no table, no footer and no panel, because they
    /// mean the same thing on every list in the app. That is a promise, and this is
    /// what holds it up: the moment a new list forgets its `cursor::scroll_key`
    /// call, undocumented keys stop working with nothing on screen to say they ever
    /// did.
    ///
    /// The Overview is absent on purpose. It holds no list -- only a fixed stack of
    /// bands and subtotals -- so there is no cursor to move.
    #[test]
    fn the_scroll_keys_work_on_every_list_in_the_app() {
        /// A screen key, and the cursor that screen scrolls.
        type ScreenCursor = (char, fn(&App) -> usize);

        let screens: [ScreenCursor; 7] = [
            ('2', |app| app.cash.selected_index()),
            ('3', |app| app.credit.selected_index()),
            ('4', |app| app.savings.selected_index()),
            ('5', |app| app.planning.selected_index()),
            ('6', |app| app.funds.selected_index()),
            ('7', |app| app.recurring_goal.selected_index()),
            ('8', |app| app.recurring_txn.selected_index()),
        ];
        for (key, index) in screens {
            let mut app = app_with_two_rows_on_every_list();
            press(&mut app, KeyCode::Char(key));
            press(&mut app, KeyCode::Home);
            let first = index(&app);
            press(&mut app, KeyCode::End);
            let last = index(&app);
            press(&mut app, KeyCode::Home);
            assert_eq!(index(&app), first, "Home does not work on screen {key}");
            assert!(
                last > first,
                "End does not work on screen {key}: {first} -> {last}"
            );
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Up);
            assert_eq!(index(&app), first, "↑/↓ do not work on screen {key}");
        }
    }

    /// Two rows on all seven lists, which is what makes the test above mean
    /// anything: over an empty list every scroll key leaves the cursor at zero,
    /// so a screen that never calls `cursor::scroll_key` would pass just as
    /// well as one that does. `app()` already fills the two ledgers, Savings
    /// and Planning; the Funds and the two recurring screens start empty and
    /// are filled here.
    fn app_with_two_rows_on_every_list() -> App {
        let mut app = app();
        let checking = account::list(&app.db).unwrap()[0].id;
        for (name, day_of_month) in [("Utilities", 1), ("Gym", 15)] {
            recurring_txn::insert(
                &app.db,
                &recurring_txn::NewRecurringTxn {
                    description: name.to_string(),
                    cents: Cents::from_dollars(-100),
                    account_id: checking,
                    cadence: recurring_txn::Cadence::Monthly,
                    anchor_date: day(2026, 8, day_of_month),
                    horizon: None,
                },
            )
            .unwrap();
            recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month: 9,
                    base_cents: Cents::from_dollars(128),
                    taxed: false,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
        }
        for (name, target, dollars) in [
            ("Bonds", fund::Target::AgeOver30, 30_000),
            (
                "Domestic",
                fund::Target::RemainderShare(crate::rate::BasisPoints(6_000)),
                90_000,
            ),
        ] {
            let ord = fund::next_ord(&app.db).unwrap();
            fund::insert(
                &app.db,
                &fund::NewFund {
                    name: name.to_string(),
                    ord,
                    target,
                    actual: Cents::from_dollars(dollars),
                },
            )
            .unwrap();
        }
        // A literal birth date is refused everywhere in this crate; derive one
        // that is always forty-four years before whatever `today` is, so the
        // age row has a target and screen 6 does not swallow the scroll keys
        // behind a birth-date prompt.
        setting::set(
            &app.db,
            key::BIRTH_DATE,
            app.today.with_year(app.today.year() - 44).unwrap(),
        )
        .unwrap();
        app.reload().unwrap();
        app
    }

    /// `←`/`→` mean the same thing on every date field the app has: step it a
    /// day. On a form they arrive through the shared choice keys, which is
    /// what makes the account selector beside them the thing they must not
    /// touch.
    #[test]
    fn the_arrow_keys_step_a_forms_date_field_by_a_day() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Date);

        press(&mut app, KeyCode::Right);
        assert_eq!(
            form(&app).display(TxnField::Date).plain_text(),
            "2026-08-16"
        );
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(
            form(&app).display(TxnField::Date).plain_text(),
            "2026-08-14"
        );
        assert_eq!(
            form(&app).display(TxnField::Account).plain_text(),
            "CHK — Everyday",
            "the account selector sits on its own field"
        );
    }

    /// The worksheet answers its own keys rather than going through
    /// `form_key`, so its date needs the arrows wiring separately -- and the
    /// two focuses that take digits must stay untouched by them.
    #[test]
    fn the_arrow_keys_step_the_worksheet_date_by_a_day() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);

        press(&mut app, KeyCode::Right);
        let sheet = worksheet(&app);
        assert_eq!(sheet.focus(), worksheet_screen::Focus::Date);
        assert_eq!(sheet.date_text(), "2026-08-16");

        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let sheet = worksheet(&app);
        assert_eq!(sheet.date_text(), "2026-08-14");
    }

    /// `t`'s confirmation is one date field, and it is the date every payday
    /// transfer is stamped with.
    #[test]
    fn the_arrow_keys_step_the_transfer_confirmations_date_by_a_day() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        press(&mut app, KeyCode::Right);
        let confirm = plan_transfers(&app);
        let stepped = confirm.commit().unwrap();

        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let confirm = plan_transfers(&app);
        assert_eq!(
            confirm.commit().unwrap(),
            stepped - chrono::TimeDelta::days(2)
        );
    }

    /// Shift with an arrow is the same nudge with a bigger step wherever
    /// there is a date, not a scrub-only trick. On a form it arrives through
    /// the shared `form_key`, so this pins the one handler that answers every
    /// field-driven modal in the app.
    #[test]
    fn shift_and_an_arrow_step_a_forms_date_a_week() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Date);

        shift_press(&mut app, KeyCode::Right);
        assert_eq!(
            form(&app).display(TxnField::Date).plain_text(),
            "2026-08-22"
        );

        shift_press(&mut app, KeyCode::Left);
        shift_press(&mut app, KeyCode::Left);
        assert_eq!(
            form(&app).display(TxnField::Date).plain_text(),
            "2026-08-08"
        );
    }

    /// A selector has no week to move, so Shift moves it one choice rather
    /// than nothing: a modified arrow the terminal delivers and the app drops
    /// is a dead key with nothing on screen to say why.
    #[test]
    fn shift_and_an_arrow_move_a_selector_one_choice() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Account);

        let first = form(&app).display(TxnField::Account).plain_text();
        shift_press(&mut app, KeyCode::Right);
        let second = form(&app).display(TxnField::Account).plain_text();
        shift_press(&mut app, KeyCode::Left);

        assert_ne!(first, second);
        assert_eq!(form(&app).display(TxnField::Account).plain_text(), first);
    }

    /// The worksheet answers its own keys rather than going through
    /// `form_key`, so its date takes the modifier separately.
    #[test]
    fn shift_and_an_arrow_step_the_worksheet_date_a_week() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);

        shift_press(&mut app, KeyCode::Right);
        let sheet = worksheet(&app);
        assert_eq!(sheet.date_text(), "2026-08-22");

        shift_press(&mut app, KeyCode::Left);
        shift_press(&mut app, KeyCode::Left);
        let sheet = worksheet(&app);
        assert_eq!(sheet.date_text(), "2026-08-08");
    }

    /// `t`'s confirmation answers its own keys too, and its date is the one
    /// every payday transfer is stamped with.
    #[test]
    fn shift_and_an_arrow_step_the_transfer_confirmations_date_a_week() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        let confirm = plan_transfers(&app);
        let opened = confirm.commit().unwrap();

        shift_press(&mut app, KeyCode::Right);
        let confirm = plan_transfers(&app);
        assert_eq!(
            confirm.commit().unwrap(),
            opened + chrono::TimeDelta::days(7)
        );
    }
}
