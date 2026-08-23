use super::accounts::{self as accounts_screen, AccountForm, Accounts};
use super::autocomplete::Autocomplete;
use super::cursor::{self, Scroll};
use super::destination;
use super::form::{self, DateField, Step, TransferForm, TxnForm, ValueForm};
use super::fund::{self as fund_screen, FundForm, Funds};
use super::goal_form::{AllocationForm, CloseForm, GoalForm, GoalTarget};
use super::help::{self, Help, Topic};
use super::ledger::{self, Ledger, Window};
use super::modal::{self, Confirm, Modal};
use super::overview;
use super::picker::{self, Picker};
use super::planning::{self, BillForm, Planning, Target, TransferConfirm};
use super::recurring_goal::{self as recurring_goal_screen, RecurringGoalForm, RecurringGoals};
use super::recurring_txn::{self as recurring_txn_screen, RecurringTxnForm, RecurringTxns};
use super::savings::{self, Savings};
use super::search::{self, Search};
use super::text::{self, Edit};
use super::worksheet::{self, Worksheet};
use super::{Account, Label};
use crate::calc;
use crate::db::account::{self, Kind};
use crate::db::bill;
use crate::db::fund;
use crate::db::goal;
use crate::db::recurring_goal::{self, Entry};
use crate::db::recurring_txn;
use crate::db::setting::{self, key};
use crate::db::txn;
use crate::db::{AccountId, Db, GoalId, RecurringGoalId};
use crate::description;
use crate::fund as fund_engine;
use crate::goal as goal_engine;
use crate::money::Cents;
use crate::overview::Overview;
use crate::plan;
use crate::plan_line::{Destination, Line};
use crate::projection::{self, Dates};
use crate::recurring_txn::{self as recurring_engine, Extended};
use crate::savings_block::Block as SavingsBlock;
use crate::transfer;
use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Paragraph, Tabs};
use std::collections::HashSet;
use std::time::{Duration, Instant};

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

/// One queued worksheet: the container it opens on, its pot, and the shares
/// it opens with.
type WorksheetPrefill = (AccountId, Cents, Vec<(GoalId, Cents)>);

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
    /// written. Each is a container, its pot, and the shares it opens with.
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

    /// Drop a status message that has outlived [`STATUS_TTL`].
    ///
    /// Called by the event loop rather than by `footer`, which only reads:
    /// the message is gone from the app, not merely hidden, so the next thing
    /// to consult `status` sees what the footer shows. A key press clears it
    /// sooner -- this is only what happens when none arrives.
    pub fn expire_status(&mut self) {
        self.expire_status_at(Instant::now());
    }

    fn expire_status_at(&mut self, now: Instant) {
        if self.status_until.is_some_and(|until| now >= until) {
            self.status.clear();
            self.status_until = None;
        }
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

    fn savings_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.savings, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Tab => self.savings.next_container(),
            KeyCode::BackTab => self.savings.previous_container(),
            // Pure view state, like the Recurring Goals screen's and unlike
            // the ledgers': every goal is already loaded for the footer's
            // reconciliation, so there is nothing to re-query.
            KeyCode::Char('[') => self.savings.previous_month(),
            KeyCode::Char(']') => self.savings.next_month(),
            KeyCode::Esc => {
                if !search::escape_kept_filter(&mut self.savings) {
                    self.savings.clear_filters();
                }
            }
            KeyCode::Char('/') => self.savings.begin_search(),
            KeyCode::Char('a') => self.open_allocate()?,
            KeyCode::Char('A') => self.open_payday()?,
            KeyCode::Char('i') => self.open_interest()?,
            KeyCode::Char('e') => self.open_goal_edit()?,
            KeyCode::Char('c') => self.open_close_out()?,
            KeyCode::Char('n') => self.open_new_goal()?,
            KeyCode::Char('K') => self.move_goal(Move::Up)?,
            KeyCode::Char('J') => self.move_goal(Move::Down)?,
            KeyCode::Char('f') => self.toggle_favorite()?,
            KeyCode::Char('U') => self.open_undo()?,
            _ => {}
        }
        Ok(())
    }

    fn planning_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.planning, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('e') => self.open_value_edit()?,
            KeyCode::Char('a') => self.modal = Some(Modal::Bill(BillForm::add())),
            KeyCode::Char('E') => self.open_bill_edit()?,
            KeyCode::Char('d') => self.open_bill_delete()?,
            KeyCode::Char('p') => self.pin()?,
            KeyCode::Char('P') => self.unpin()?,
            KeyCode::Char('t') => self.open_plan_transfers()?,
            KeyCode::Enter => self.open_plan_details(),
            _ => {}
        }
        Ok(())
    }

    /// `t` for the same reason the cash ledger's transfer is `t`: the key
    /// names the kind of action and not its size, and a letter meaning "move
    /// money between accounts" on one screen and something else on the next
    /// costs the owner more than the distinction would buy. What is particular
    /// to this one -- every row the plan calls for in a single transaction,
    /// and the allocation worksheets opened on top of them -- is said in its
    /// Help detail, where a reader who wants it will be.
    fn open_plan_transfers(&mut self) -> Result<()> {
        let plan = plan::compute_from_db(&self.db, self.adhoc)?;
        let rows = match transfer::plan(&self.db, &plan.lines) {
            Ok(rows) => rows,
            Err(e) => {
                self.status = format!("{e:#}");
                return Ok(());
            }
        };
        let from = transfer::source(&self.db)?;
        let date = calc::business_day::add(self.today, 2)?;
        // Business days either side, not the one date the form opens on: the
        // date is editable before the write, so a run correcting a wrongly
        // dated first one steps off this default and onto the day that one
        // landed.
        let scanned = calc::business_day::window(date, DUPLICATE_SCAN_DAYS, DUPLICATE_SCAN_DAYS)?;
        let clashing = transfer::already_written(&self.db, from, &scanned, &rows)?;
        if !clashing.is_empty() {
            // A warning, not a block: these are ordinary ledger rows and a
            // second run with a corrected date is a real case. The dates are
            // named because they are days the form is not showing.
            let days: Vec<String> = clashing.iter().map(|d| d.to_string()).collect();
            let carry = if days.len() == 1 { "carries" } else { "carry" };
            self.status = format!("{} already {carry} matching rows", transfer::joined(&days));
        }
        self.modal = Some(Modal::PlanTransfers(TransferConfirm::new(
            rows, self.today, date,
        )));
        Ok(())
    }

    fn commit_plan_transfers(&mut self) -> Result<()> {
        let Some(Modal::PlanTransfers(confirm)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected date keeps the form.
        let date = confirm.commit()?;
        let from = transfer::source(&self.db)?;
        let rows = confirm.rows().to_vec();
        // Built before the write: `worksheet_prefills` only reads goals and
        // settings, so this is behaviour-preserving, and it turns a
        // container-spanning plug -- the one case it can refuse -- into a
        // refusal before anything is written rather than an error stranded
        // after the payday is already on the ledger.
        let prefills = self.worksheet_prefills(&rows)?;
        transfer::execute(&self.db, from, date, &rows)?;
        let total: Cents = rows.iter().map(|r| r.cents()).sum();
        self.status = format!(
            "wrote {} transfers, {}",
            rows.len(),
            crate::demo::figure(total)
        );
        self.close_modal();
        self.reload()?;
        // Only stored once the reload has succeeded: a queue assigned before
        // a fallible step and abandoned by its `?` would sit non-empty with
        // no worksheet on screen, ready for some unrelated `Esc` or `Enter`
        // to resurrect it later.
        self.pending_worksheets = prefills;
        self.open_next_worksheet()
    }

    /// One prefilled worksheet per container a transfer landed in.
    ///
    /// Each container's lines are the goals its own Planning lines name, at
    /// those lines' amounts; the container holding the unclaimed goals also
    /// carries the plug, spread equally over them the way the worksheet's own
    /// `s` spreads.
    fn worksheet_prefills(&self, rows: &[transfer::Row]) -> Result<Vec<WorksheetPrefill>> {
        let spread_container = transfer::spread_container(&self.db)?;
        let spread = self.spread_asks()?;
        let mut out = Vec::new();
        for row in rows {
            let transfer::Row::Transfer {
                to, cents, lines, ..
            } = row
            else {
                continue;
            };
            let mut shares: Vec<(GoalId, Cents)> = Vec::new();
            for (line, amount) in lines {
                match line.destination() {
                    Destination::Goal(key) => {
                        if let Some(id) = setting::get(&self.db, key)? {
                            add_share(&mut shares, id, *amount);
                        }
                    }
                    Destination::Spread => {
                        if spread_container == Some(*to) && *amount != Cents::ZERO {
                            let asks: Vec<(i64, Cents)> = spread
                                .iter()
                                .filter(|(goal, _)| goal.container_account_id == *to)
                                .map(|(goal, ask)| (goal.id.0, *ask))
                                .collect();
                            // Asks that fit are met in full and the rest of
                            // the plug is left unallocated, on purpose: it is
                            // money to place by hand rather than money this
                            // has to find a home for.
                            for (id, share) in calc::fit(*amount, &asks)? {
                                add_share(&mut shares, GoalId(id), share);
                            }
                        }
                    }
                    Destination::Account(_) => {}
                }
            }
            // A row with no goal- or spread-backed line at all -- every line
            // landing in `to` is account-destination -- has nothing for a
            // worksheet to allocate; queuing one would open a container with
            // no goals and the whole pot sitting in `remaining`.
            if !shares.is_empty() {
                out.push((*to, *cents, shares));
            }
        }
        Ok(out)
    }

    /// `e` on Planning: a constant is typed into a one-field form, a
    /// destination is chosen from the goals that exist.
    ///
    /// `Planning::selected` only ever returns a row carrying one or the
    /// other, so the row's own values come out together with it or not at
    /// all.
    fn open_value_edit(&mut self) -> Result<()> {
        let opened = self
            .planning
            .selected()
            .map(|row| (row.editable, row.label.clone(), row.edit.clone()));
        match opened {
            None | Some((None, _, _)) => self.status = NOTHING_SELECTED.to_string(),
            Some((Some(planning::Editable::Constant(target)), label, prefill)) => {
                // A percentage and a count of pay periods open the same
                // modal a target does, and only one of the three is money.
                let label = label.trim().to_string();
                let form = if target.is_money() {
                    ValueForm::money(label, &prefill)
                } else {
                    ValueForm::new(label, &prefill)
                };
                self.modal = Some(Modal::Value(target, form));
            }
            Some((Some(planning::Editable::Destination(line)), _, _)) => {
                self.open_destination(line)?
            }
        }
        Ok(())
    }

    /// Why the transfers are unresolved, in full.
    ///
    /// The screen reports the failure in a cell about fifty columns wide,
    /// which is not enough to name the goal in the wrong container -- the one
    /// thing the owner needs in order to act on it. This is the same failure
    /// with room to explain itself.
    fn open_plan_details(&mut self) {
        let detail = self.planning.transfer_detail().to_vec();
        if detail.is_empty() {
            self.status = "the transfers resolve; nothing to explain".to_string();
            return;
        }
        self.modal = Some(Modal::Details("Transfers unresolved", detail));
    }

    /// What each goal the plug spreads over asks of this paycheck.
    ///
    /// The two dates the figure depends on are `App`'s, which is the whole of
    /// what this adds to [`transfer::spread_asks`]: the set and its pricing
    /// are that function's, so the Planning screen's coverage check and the
    /// prefill `t` writes cannot come to disagree about either.
    fn spread_asks(&self) -> Result<Vec<(goal::Goal, Cents)>> {
        transfer::spread_asks(&self.db, self.today, self.period_days)
    }

    /// The goals this line could point at, with the withdrawal among them.
    ///
    /// Every open goal, not the ones in some container the line already
    /// favours: which container a line lands in is a consequence of the goal
    /// chosen, not a constraint on choosing it.
    fn open_destination(&mut self, line: Line) -> Result<()> {
        let accounts = account::list(&self.db)?;
        let offer = |goal: goal::Goal| destination::Offered {
            container: accounts
                .iter()
                .find(|a| a.id == goal.container_account_id)
                .map_or("?", |a| a.name.as_str())
                .to_string(),
            id: goal.id,
            name: goal.name,
        };
        let offered = goal::all_with_balances(&self.db)?
            .into_iter()
            .map(|g| offer(g.goal))
            .collect();
        // Resolved through `goal::get` rather than looked up in `offered`:
        // `offered` holds open goals, and a line pointing at a goal that has
        // since been closed still points somewhere real. The list has to open
        // on it, or a stray `Enter` clears a destination nobody questioned.
        let current = match line.destination() {
            Destination::Goal(key) => match setting::get(&self.db, key)? {
                Some(id) => goal::get(&self.db, id)?.map(offer),
                None => None,
            },
            _ => None,
        };
        let suggestion = transfer::suggest(&self.db, line)?.map(offer);
        self.modal = Some(Modal::Destination(destination::Chooser::new(
            line, offered, current, suggestion,
        )));
        Ok(())
    }

    fn destination_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(Modal::Destination(chooser)) = &mut self.modal
            && chooser.is_searching()
        {
            search::search_key(chooser, key);
            return Ok(());
        }
        if key.code == KeyCode::Enter {
            return self.commit_destination();
        }
        let Some(Modal::Destination(chooser)) = &mut self.modal else {
            return Ok(());
        };
        if cursor::scroll_key(chooser, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if !search::escape_kept_filter(chooser) {
                    self.close_modal();
                }
            }
            KeyCode::Char('/') => chooser.begin_search(),
            _ => {}
        }
        Ok(())
    }

    /// Write the chosen goal's **id** under this line's key, or clear it.
    ///
    /// The id, never the name: goal names are not unique, and the whole
    /// reason this is a list is that what the owner picked is a specific row
    /// rather than a string that might match three of them.
    fn commit_destination(&mut self) -> Result<()> {
        let Some(Modal::Destination(chooser)) = &self.modal else {
            return Ok(());
        };
        let line = chooser.line();
        let chosen = chooser.selected().cloned();
        let Destination::Goal(key) = line.destination() else {
            bail!("{} holds no goal to point", line.label());
        };
        let status = match chosen {
            None => return self.nothing_selected(),
            Some(destination::Choice::Unset) => {
                setting::clear(&self.db, key)?;
                format!("{} now leaves the tracked system", line.label())
            }
            Some(destination::Choice::Goal { id, name, .. }) => {
                setting::set(&self.db, key, id)?;
                format!("{} → {name}", line.label())
            }
        };
        self.close_modal();
        self.status = status;
        self.reload()
    }

    fn commit_value(&mut self) -> Result<()> {
        let Some(Modal::Value(target, form)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected edit keeps the form
        // and everything typed into it.
        target.write(&self.db, self.today, form.value())?;
        self.status = format!("{} saved", form.label().plain_text().trim());
        self.close_modal();
        self.reload()
    }

    fn commit_bill(&mut self) -> Result<()> {
        let Some(Modal::Bill(form)) = &self.modal else {
            return Ok(());
        };
        let edit = form.commit()?;
        match form.editing {
            Some(id) => bill::update(&self.db, id, &edit)?,
            None => {
                let sort = bill::next_sort(&self.db, edit.category)?;
                bill::insert(
                    &self.db,
                    &bill::NewBill {
                        label: edit.label.clone(),
                        cents: edit.cents,
                        category: edit.category,
                        sort,
                    },
                )?;
            }
        }
        self.status = format!("{} {}", edit.label, crate::demo::figure(edit.cents));
        self.close_modal();
        self.reload()
    }

    /// A bill's label and category live nowhere else on the screen, so `e`'s
    /// one-field amount editor cannot reach them: `E` is the whole row. Editing
    /// in place also keeps the bill's `sort`, which deleting and re-adding
    /// would not.
    fn open_bill_edit(&mut self) -> Result<()> {
        let Some(Target::Bill(id)) = self.planning.selected_target() else {
            self.status = "only a bill has fields to edit".to_string();
            return Ok(());
        };
        let found = bill::get(&self.db, id)?;
        self.modal = Some(Modal::Bill(BillForm::edit(&found)));
        Ok(())
    }

    /// Only a bill can be deleted. Every other row is a constant, and the one
    /// constant that can be absent is unset with `p`.
    fn open_bill_delete(&mut self) -> Result<()> {
        let Some(Target::Bill(id)) = self.planning.selected_target() else {
            self.status = "only a bill can be deleted".to_string();
            return Ok(());
        };
        let found = bill::get(&self.db, id)?;
        let label = format!("{}  {}", found.label, crate::demo::figure(found.cents));
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteBill(id),
            label,
        });
        Ok(())
    }

    /// Entering the screen is what asks for a birth date, so the question
    /// comes back the next time it is entered and never once the setting
    /// exists.
    fn open_funds(&mut self) {
        self.screen = Screen::Funds;
        if self.funds.needs_birth_date() {
            self.modal = Some(Modal::BirthDate(ValueForm::date("Birth Date", "")));
        }
    }

    fn funds_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.funds, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.modal = Some(Modal::Fund(FundForm::add())),
            KeyCode::Char('e') => self.open_fund_value_edit(),
            KeyCode::Char('E') => self.open_fund_edit()?,
            KeyCode::Char('d') => self.open_fund_delete(),
            _ => {}
        }
        Ok(())
    }

    fn open_fund_value_edit(&mut self) {
        let Some(row) = self.funds.selected() else {
            self.status = NOTHING_SELECTED.to_string();
            return;
        };
        // The stored cents, not the whole dollars the row prints: opening a
        // value and pressing Enter must not quietly round it.
        self.modal = Some(Modal::FundValue(
            row.fund_id,
            ValueForm::money("Actual Value", &row.actual.to_string()),
        ));
    }

    fn open_fund_edit(&mut self) -> Result<()> {
        let Some(row) = self.funds.selected() else {
            return self.nothing_selected();
        };
        let found = fund::get(&self.db, row.fund_id)?;
        self.modal = Some(Modal::Fund(FundForm::edit(&found)));
        Ok(())
    }

    fn open_fund_delete(&mut self) {
        let Some(row) = self.funds.selected() else {
            self.status = NOTHING_SELECTED.to_string();
            return;
        };
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteFund(row.fund_id),
            label: format!("{} — {}", row.name, crate::demo::whole_figure(row.actual)),
        });
    }

    fn commit_fund(&mut self) -> Result<()> {
        let Some(Modal::Fund(form)) = &self.modal else {
            return Ok(());
        };
        let edit = form.commit()?;
        match form.editing {
            Some(id) => fund::update(&self.db, id, &edit)?,
            None => {
                let ord = fund::next_ord(&self.db)?;
                fund::insert(
                    &self.db,
                    &fund::NewFund {
                        name: edit.name.clone(),
                        ord,
                        target: edit.target,
                        actual: edit.actual,
                    },
                )?;
            }
        }
        self.status = format!("{} saved", edit.name);
        self.close_modal();
        self.reload()
    }

    /// Insert the account `a` describes, appended to whatever its kind
    /// already holds.
    ///
    /// The sort is computed the way `import::constants` computes it, and for
    /// the same reason: `sort` is only ever read through an `ORDER BY` that
    /// breaks ties by code, so "after the ones already there" is the only
    /// placement that does not depend on rows nobody has seen. Moving it
    /// anywhere else is `e`'s `Order` selector, which renumbers the kind.
    fn commit_new_account(&mut self) -> Result<()> {
        let Some(Modal::Account(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit_new()?;
        let sort = account::list_by_kind(&self.db, new.kind)?.len() as i64;
        account::insert(&self.db, &new.code, &new.name, new.kind, sort)?;
        self.status = format!("{} added", new.name);
        self.close_modal();
        self.reload()
    }

    fn commit_fund_value(&mut self) -> Result<()> {
        let Some(Modal::FundValue(id, form)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected edit keeps the form
        // and everything typed into it.
        let actual = form::parse_whole_amount(form.value())?;
        fund::set_actual(&self.db, *id, actual)?;
        self.status = "value saved".to_string();
        self.close_modal();
        self.reload()
    }

    fn commit_birth_date(&mut self) -> Result<()> {
        let Some(Modal::BirthDate(form)) = &self.modal else {
            return Ok(());
        };
        let birth = form::parse_date(form.value())?;
        setting::set(&self.db, key::BIRTH_DATE, birth)?;
        self.status = "birth date saved".to_string();
        self.close_modal();
        self.reload()
    }

    fn reload_funds(&mut self) -> Result<()> {
        self.funds
            .set_allocation(fund_engine::compute_from_db(&self.db, self.today)?);
        Ok(())
    }

    fn recurring_txn_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.recurring_txn, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.open_recurring_txn_add()?,
            KeyCode::Char('e') => self.open_recurring_txn_edit()?,
            KeyCode::Char('d') => self.open_recurring_txn_delete(),
            KeyCode::Char('g') => self.regenerate_selected()?,
            KeyCode::Char('G') => self.regenerate_every()?,
            KeyCode::Char('x') => self.extend_selected()?,
            KeyCode::Char('P') => self.make_paycheck()?,
            _ => {}
        }
        Ok(())
    }

    fn open_recurring_txn_add(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::RecurringTxn(RecurringTxnForm::add(
            accounts, self.today,
        )?));
        Ok(())
    }

    fn open_recurring_txn_edit(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let found = recurring_txn::get(&self.db, row.recurring_txn_id)?;
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::RecurringTxn(RecurringTxnForm::edit(
            accounts, self.today, &found,
        )?));
        Ok(())
    }

    fn commit_recurring_txn(&mut self) -> Result<()> {
        let Some(Modal::RecurringTxn(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                recurring_txn::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                recurring_txn::insert(&self.db, &new)?;
                "added"
            }
        };
        self.status = format!("{verb} {} · g generates its rows", new.description);
        self.close_modal();
        self.reload()
    }

    /// The confirmation says what actually happens: the rows are released, not
    /// deleted, so no balance moves.
    fn open_recurring_txn_delete(&mut self) {
        match self.recurring_txn.selected() {
            None => self.status = NOTHING_SELECTED.to_string(),
            Some(row) => {
                let label = format!(
                    "{} · {} · {} ledger rows will be released, not deleted",
                    row.description,
                    crate::demo::figure(row.cents),
                    row.owned
                );
                self.modal = Some(Modal::Confirm {
                    action: Confirm::DeleteRecurringTxn(row.recurring_txn_id),
                    label,
                });
            }
        }
    }

    fn regenerate_selected(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (row.recurring_txn_id, row.description.clone());
        let report = recurring_engine::regenerate(&self.db, id, self.today)?;
        self.status = format!(
            "{description}: removed {} · released {} · adopted {} · inserted {}",
            report.removed, report.released, report.adopted, report.inserted
        );
        self.reload()
    }

    fn regenerate_every(&mut self) -> Result<()> {
        let report = recurring_engine::regenerate_all(&self.db, self.today)?;
        self.status = format!(
            "every recurring transaction: removed {} · released {} · adopted {} · inserted {}",
            report.removed, report.released, report.adopted, report.inserted
        );
        self.reload()
    }

    /// `x`: push the selected recurring transaction one rolling horizon
    /// further out and regenerate it.
    ///
    /// Both refusals name the date that binds, because "nothing happened" on
    /// a screen whose whole subject is future rows reads as a bug.
    fn extend_selected(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (row.recurring_txn_id, row.description.clone());
        self.status = match recurring_engine::extend(&self.db, id, self.today)? {
            Extended::Through { through, report } => format!(
                "{description} extended through {through}: removed {} · released {} · adopted {} · inserted {}",
                report.removed, report.released, report.adopted, report.inserted
            ),
            Extended::Ends(end) => {
                format!("{description} ends {end} — change that with e")
            }
            Extended::Ceiling(reach) => {
                format!("{description} already reaches {reach}, ten years out")
            }
        };
        self.reload()
    }

    fn make_paycheck(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (row.recurring_txn_id, row.description.clone());
        recurring_txn::set_paycheck(&self.db, id)?;
        self.status = format!("{description} is now the paycheck transaction");
        self.reload()
    }

    fn recurring_goal_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.recurring_goal, key.code) {
            return Ok(());
        }
        match key.code {
            // Pure view state: the screen holds every entry already, so
            // unlike the ledgers' `[` and `]` there is nothing to re-query.
            KeyCode::Char('[') => self.recurring_goal.previous_month(),
            KeyCode::Char(']') => self.recurring_goal.next_month(),
            KeyCode::Esc => self.recurring_goal.clear_month(),
            KeyCode::Char('a') => self.open_recurring_goal_add()?,
            KeyCode::Char('e') => self.open_recurring_goal_edit()?,
            KeyCode::Char('d') => self.open_recurring_goal_delete()?,
            KeyCode::Char('s') => self.open_recurring_goals()?,
            _ => {}
        }
        Ok(())
    }

    fn open_recurring_goal_add(&mut self) -> Result<()> {
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::add(
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }

    fn open_recurring_goal_edit(&mut self) -> Result<()> {
        let Some(row) = self.recurring_goal.selected() else {
            return self.nothing_selected();
        };
        let found = recurring_goal::get(&self.db, row.recurring_goal_id)?;
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::edit(
            &found,
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }

    fn commit_recurring_goal(&mut self) -> Result<()> {
        let Some(Modal::RecurringGoalEntry(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                recurring_goal::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                recurring_goal::insert(&self.db, &new)?;
                "added"
            }
        };
        self.status = format!("{verb} {}", new.name);
        self.close_modal();
        self.reload()
    }

    /// The confirmation names the entry and how many goals -- open or closed
    /// -- reference it: that is the number `recurring_goal::delete` actually
    /// gates on, which is not the same as the screen's "Open" column. Querying
    /// it fresh rather than reading `row.open_goals` is what keeps the
    /// confirmation from promising a delete the gate is about to refuse.
    fn open_recurring_goal_delete(&mut self) -> Result<()> {
        let Some(row) = self.recurring_goal.selected() else {
            return self.nothing_selected();
        };
        let goals = recurring_goal::goal_count(&self.db, row.recurring_goal_id)?;
        let label = format!(
            "{} · {} · {goals} goal(s), open or closed, reference it",
            row.name,
            crate::demo::figure(row.base_cents)
        );
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteRecurringGoal(row.recurring_goal_id),
            label,
        });
        Ok(())
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

    fn ledger(&self) -> &Ledger {
        match self.screen {
            Screen::Credit => &self.credit,
            _ => &self.cash,
        }
    }

    fn ledger_mut(&mut self) -> &mut Ledger {
        match self.screen {
            Screen::Credit => &mut self.credit,
            _ => &mut self.cash,
        }
    }

    /// Both ledgers, for the things that are not one screen's state: the
    /// shared month, the cursor re-anchoring that follows it, and the date
    /// range that bounds them. Iterating is what keeps "both, always" from
    /// decaying into a pair of lines one of which is later forgotten.
    fn ledgers_mut(&mut self) -> [&mut Ledger; 2] {
        [&mut self.cash, &mut self.credit]
    }

    fn ledger_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(self.ledger_mut(), key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('[') => {
                self.ledger_mut().previous_month();
                self.sync_month()?;
            }
            KeyCode::Char(']') => {
                self.ledger_mut().next_month();
                self.sync_month()?;
            }
            // The ledgers have no All to clear to: the window bounds the
            // query itself, so "no filter" would be every transaction ever.
            // Clearing it means the window the screen opens on -- but only
            // once a kept needle is gone, and only on this ledger: the window
            // is shared with the other one and the needle is not.
            KeyCode::Esc => {
                if !search::escape_kept_filter(self.ledger_mut()) {
                    let opening = Window::containing(self.today);
                    self.ledger_mut().set_window(opening);
                    self.sync_month()?;
                }
            }
            KeyCode::Tab => {
                self.ledger_mut().next_account();
                self.reload_and_anchor()?;
            }
            KeyCode::BackTab => {
                self.ledger_mut().previous_account();
                self.reload_and_anchor()?;
            }
            KeyCode::Char('/') => self.ledger_mut().begin_search(),
            KeyCode::Char('r') => self.open_reconcile(),
            KeyCode::Char('a') => self.open_add()?,
            // Cash-only: a transfer leaves an account you hold, so there is
            // nothing on the Credit ledger for it to start from. `p` is the
            // move that belongs to a card -- cash settling it.
            KeyCode::Char('t') if self.screen == Screen::Cash => self.open_transfer()?,
            KeyCode::Char('p') => self.open_payment()?,
            KeyCode::Char('e') => self.open_edit()?,
            KeyCode::Char('d') => self.open_delete(),
            _ => {}
        }
        Ok(())
    }

    /// `r`: the balance a statement says the filtered account holds.
    ///
    /// Only under an account filter. Under All the border quotes the whole
    /// kind's balance, which is not a figure any statement names, so there
    /// would be nothing for the target to be compared against.
    ///
    /// Opens on the target already set, so a figure being corrected is edited
    /// rather than retyped.
    fn open_reconcile(&mut self) {
        let ledger = self.ledger();
        let Some(id) = ledger.selected_account() else {
            self.status = "reconcile needs an account filter".to_string();
            return;
        };
        let label = Label::plain("Target · ").account(Account::named(ledger.accounts(), id));
        let prefill = ledger.target().map(|t| t.to_string()).unwrap_or_default();
        self.modal = Some(Modal::Reconcile(id, ValueForm::money(label, &prefill)));
    }

    /// An empty field clears the target: that is how one goes away, since
    /// `Esc` means "leave the figure alone" everywhere else in the app.
    ///
    /// Parsed before the modal closes, so a rejected figure keeps the form
    /// and everything typed into it. Nothing is written and nothing is
    /// reloaded -- the target lives on the `Ledger` until the app quits.
    fn commit_reconcile(&mut self) -> Result<()> {
        let Some(Modal::Reconcile(id, form)) = &self.modal else {
            return Ok(());
        };
        let (id, raw) = (*id, form.value().trim().to_string());
        let target = if raw.is_empty() {
            None
        } else {
            Some(form::parse_amount(&raw)?)
        };
        let name = self.ledger().account_name(id).to_string();
        self.ledger_mut().set_target(target);
        self.status = match target {
            Some(cents) => format!("{name} target {}", crate::demo::figure(cents)),
            None => format!("{name} target cleared"),
        };
        self.close_modal();
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

    /// The balance a ledger's `Tab` filter names: the whole kind under All,
    /// one account when narrowed to one.
    ///
    /// Quoted at `today` and not at the window, which is what makes it the
    /// same figure as the Overview's To-Date column -- the two screens showing
    /// one balance under two numbers is the failure this exists to avoid. It
    /// is also as *stored*, so the Credit ledger's total is debt-positive like
    /// the column above it; the Overview is the one screen that negates.
    fn ledger_total(&self, ledger: &Ledger) -> Result<Cents> {
        match ledger.selected_account() {
            Some(id) => txn::balance_at(&self.db, id, self.today),
            None => txn::balance_at_by_kind(&self.db, ledger.kind(), self.today),
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

    /// Rebuild the Accounts screen, and hand the refreshed list to every
    /// screen that shows an account's name.
    ///
    /// Those three hold their own `Vec<Account>` -- a rename made here would
    /// otherwise not reach the Overview's neighbours until a restart.
    fn reload_accounts(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        let mut rows = Vec::with_capacity(accounts.len());
        for account in &accounts {
            rows.push(accounts_screen::Row {
                account: super::Account::named(&accounts, account.id),
                code: account.code.as_str().to_string(),
                kind: account.kind,
                group: account.group,
                policy: account::interest_policy(&self.db, account.id)?,
                block: self.savings_block_of(account.id)?,
            });
        }
        self.accounts.set_rows(rows);
        self.savings.set_accounts(accounts);
        // Named rather than reached through `ledgers_mut`: each ledger holds
        // only its own kind's accounts -- that list *is* its `Tab` filter --
        // so the two take different arguments, exactly as `Ledger::new` does.
        self.cash
            .set_accounts(account::list_by_kind(&self.db, Kind::Cash)?);
        self.credit
            .set_accounts(account::list_by_kind(&self.db, Kind::Credit)?);
        Ok(())
    }

    /// Point this account at `block`, and off whichever block it held.
    ///
    /// The form carries one value, so an account cannot claim both blocks;
    /// what it *can* do is move off the one it had, and that has to clear the
    /// key rather than leave it naming an account that no longer answers for
    /// it. A key is only ever cleared when this account is the one it names,
    /// so editing one account never disturbs the other block's mapping.
    fn set_savings_block(&mut self, id: AccountId, block: Option<SavingsBlock>) -> Result<()> {
        for candidate in SavingsBlock::ALL {
            let key = candidate.key();
            match Some(candidate) == block {
                true => setting::set(&self.db, key, id)?,
                false if setting::get(&self.db, key)? == Some(id) => setting::clear(&self.db, key)?,
                false => {}
            }
        }
        Ok(())
    }

    fn accounts_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.accounts, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.modal = Some(Modal::Account(AccountForm::add())),
            KeyCode::Char('e') => self.open_account_edit()?,
            _ => {}
        }
        Ok(())
    }

    fn open_account_edit(&mut self) -> Result<()> {
        let Some(row) = self.accounts.selected() else {
            return self.nothing_selected();
        };
        let id = row.account.id();
        let (position, of_kind) = self
            .accounts
            .position_of(id)
            .context("the selected account is not in the list it came from")?;
        let account = account::get(&self.db, id)?;
        let policy = account::interest_policy(&self.db, id)?;
        let block = self.savings_block_of(id)?;
        self.modal = Some(Modal::Account(AccountForm::edit(
            &account, policy, position, of_kind, block,
        )));
        Ok(())
    }

    /// Which `Savings` block this account is the container for, if either.
    ///
    /// Read from the keys rather than from a column: the mapping is a fact
    /// about the *workbook*, not about the account, and `savings_block::Block`
    /// is what pairs each key with the block it names. A key naming another
    /// account simply does not match -- this is a lookup, not a resolution,
    /// so a dangling one is `import::savings::containers`' error to raise.
    fn savings_block_of(&self, id: AccountId) -> Result<Option<SavingsBlock>> {
        for block in SavingsBlock::ALL {
            if setting::get(&self.db, block.key())? == Some(id) {
                return Ok(Some(block));
            }
        }
        Ok(None)
    }

    /// `a`'s one write, or the five `e` stands for.
    ///
    /// The five are ordered so the last one means what it says: `reorder`
    /// renumbers by position, so it goes after the band change rather than
    /// before one that could move the row. `a` writes none of them -- a new
    /// account takes its kind's default band, no color, no interest policy
    /// and no `Savings` block, and `e` is where it is placed.
    fn commit_account(&mut self) -> Result<()> {
        let Some(Modal::Account(form)) = &self.modal else {
            return Ok(());
        };
        let Some(id) = form.editing else {
            return self.commit_new_account();
        };
        let edit = form.commit()?;
        account::set_name(&self.db, id, &edit.name)?;
        account::set_color(&self.db, id, edit.color)?;
        account::set_group(&self.db, id, edit.group)?;
        account::set_interest_policy(&self.db, id, edit.policy)?;
        account::reorder(&self.db, id, edit.position)?;
        self.set_savings_block(id, edit.block)?;
        self.status = format!("{} saved", edit.name);
        self.close_modal();
        self.reload()
    }

    fn reload_recurring_txns(&mut self) -> Result<()> {
        self.recurring_txn.set_accounts(account::list(&self.db)?);
        self.recurring_txn.set_recurring_txns(
            recurring_txn::list(&self.db)?,
            recurring_txn::owned_counts(&self.db)?,
            recurring_txn::last_owned_dates(&self.db)?,
        );
        Ok(())
    }

    fn reload_recurring_goals(&mut self) -> Result<()> {
        self.recurring_goal.set_entries(
            recurring_goal::list(&self.db)?,
            recurring_goal::open_goal_counts(&self.db)?,
        );
        Ok(())
    }

    /// Re-run the waterfall and rebuild the screen.
    ///
    /// A database the waterfall cannot run against -- no Everyday checking account
    /// -- leaves the message on the Planning screen rather than failing the
    /// whole reload: every other screen still works there, and `App::new`
    /// must not refuse to start.
    fn reload_planning(&mut self) -> Result<()> {
        match self.planning_view() {
            Ok(view) => self.planning.set_view(view),
            Err(err) => {
                self.planning.set_unavailable(format!("{err:#}"));
                Ok(())
            }
        }
    }

    /// Freeze `Excess (Actual)` at its whole-dollar floor.
    ///
    /// The floor is the same figure `compute` uses when nothing is pinned, so
    /// pinning a plan that is already balanced changes no number on screen --
    /// only whether it goes on moving.
    ///
    /// **Always pins, and overwrites a pin already there.** It does not
    /// toggle, because the press that follows a forgotten pin is the next
    /// payday's: `p` answering it with "unpinned" makes the press that
    /// matters the second one, every time. Re-pinning is also the only thing
    /// a second press could sensibly mean here -- the drift line exists to
    /// say a pin has gone stale, and the answer to a stale pin is a fresh
    /// one. Clearing is [`App::unpin`], on its own key.
    ///
    /// Both keys move together: a date with no amount would render a line
    /// about a plan that is not pinned, so `PINNED_AT` advances to today with
    /// the figure and the drift starts again from zero.
    ///
    /// Refused while the screen has no live view: `set_unavailable` leaves
    /// `excess_actual` holding whatever the last successful view left there,
    /// and pinning against that would freeze a number belonging to a plan the
    /// screen has just said it cannot compute.
    fn pin(&mut self) -> Result<()> {
        if self.planning.message().is_some() {
            self.status = "nothing to pin".to_string();
            return Ok(());
        }
        let was_pinned = self.planning.is_pinned();
        let pinned = self.planning.excess_actual().floor_to_dollar();
        setting::set(&self.db, key::PINNED_EXCESS, pinned)?;
        setting::set(&self.db, key::PINNED_AT, self.today)?;
        // Named apart so a press that replaced a pin does not read as one
        // that made the first: the figure below the plan has just changed
        // under the owner, and "pinned" alone would not say so.
        self.status = match was_pinned {
            true => format!("re-pinned {}", crate::demo::figure(pinned)),
            false => format!("pinned {}", crate::demo::figure(pinned)),
        };
        self.reload()
    }

    /// Put the waterfall back on the live balance.
    ///
    /// The other half of the payday the pin covers, and not an undo: the plan
    /// holds still while the legs are entered, and this is what ends that.
    /// Without it a pin is permanent, and `excess_used` would run off a
    /// frozen figure that never tracks reality again.
    ///
    /// Needs no live view, unlike [`App::pin`] -- it only clears two keys, and
    /// refusing here would strand a pin behind a footer still offering to
    /// remove it.
    fn unpin(&mut self) -> Result<()> {
        if !self.planning.is_pinned() {
            self.status = "nothing pinned".to_string();
            return Ok(());
        }
        setting::clear(&self.db, key::PINNED_EXCESS)?;
        setting::clear(&self.db, key::PINNED_AT)?;
        self.status = "unpinned".to_string();
        self.reload()
    }

    fn planning_view(&self) -> Result<planning::View> {
        let plan = plan::compute_from_db(&self.db, self.adhoc)?;
        // A misconfigured destination is reported on the screen, not thrown:
        // every figure above the transfer block is still right.
        let (transfers, transfer_error) = match transfer::plan(&self.db, &plan.lines) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        // The asks are read on their own, never chained to the call above.
        // The payday `Unmet Asks` exists for is the one where every line is
        // zero, and that is exactly the payday `transfer::plan` refuses with
        // `NOTHING_TO_TRANSFER` -- a read sharing its failure would go silent
        // on the one state it was put outside the block's `match` to reach.
        //
        // A failure of *this* read is its own answer, and is why it is not
        // propagated: the strict target reader is what a taxed goal with no
        // rate on record trips, and it would take the whole screen down over
        // an annotation the screen can simply omit. Zero draws no row, which
        // is what a gap nothing can measure should look like.
        let spread_ask_total = self
            .spread_asks()
            .map(|asks| asks.iter().map(|(_, ask)| *ask).sum())
            .unwrap_or(Cents::ZERO);
        // Copied out before the struct literal below moves `plan` into it.
        let plan_lines = plan.lines;
        Ok(planning::View {
            plan,
            settings: plan::settings_from_db(&self.db)?,
            wiring: transfer::wiring(&self.db)?,
            housing: bill::list(&self.db, bill::Category::Housing)?,
            other_bills: bill::list(&self.db, bill::Category::Other)?,
            pinned: setting::get(&self.db, key::PINNED_EXCESS)?,
            pinned_at: setting::get(&self.db, key::PINNED_AT)?,
            scrubbed_adhoc: (self.scrubbed_days() != 0).then_some(self.adhoc),
            transfers,
            spread_ask_total,
            transfer_error,
            transfer_detail: transfer::diagnose(&self.db, &plan_lines)?,
        })
    }

    /// The tolerant reader, because this runs during `App::new`: a taxed goal
    /// with no rate on record would otherwise stop the application starting,
    /// and the rate is set from inside it.
    fn reload_savings(&mut self) -> Result<()> {
        self.savings
            .set_goals(goal_engine::all_with_balances_tolerant(&self.db)?)?;
        let excess = crate::savings::containers_with_excess(&self.db)?;
        let containers = excess.iter().map(|(id, _)| *id).collect();
        self.savings.set_containers(containers);
        self.savings.set_excess(excess);
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

    /// Opens on the account the ledger is filtered to, or on the first when
    /// the filter is All, and on [`App::entry_date`] -- today, until a row
    /// has been added this session.
    fn open_add(&mut self) -> Result<()> {
        let accounts = account::list_by_kind(&self.db, self.kind())?;
        let preselected = self.ledger().selected_account();
        self.modal = Some(Modal::Txn(TxnForm::add(
            accounts,
            self.entry_field(),
            preselected,
        )?));
        Ok(())
    }

    fn open_edit(&mut self) -> Result<()> {
        let Some(row) = self.ledger().selected().cloned() else {
            return self.nothing_selected();
        };
        let accounts = account::list_by_kind(&self.db, self.kind())?;
        self.modal = Some(Modal::Txn(TxnForm::edit(accounts, self.today, &row)?));
        Ok(())
    }

    /// Opens on [`App::entry_date`], the same day `a` does: a transfer or a
    /// card payment read off a statement belongs to the sitting the rows
    /// around it belong to.
    fn open_transfer(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::Transfer(TransferForm::transfer(
            accounts,
            self.entry_field(),
        )?));
        Ok(())
    }

    /// Opens on [`App::entry_date`], for the reason [`App::open_transfer`]
    /// gives.
    fn open_payment(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::Transfer(TransferForm::payment(
            accounts,
            self.entry_field(),
        )?));
        Ok(())
    }

    /// The date field every form that writes a ledger row opens with: the day
    /// the last such row was written for, and today until there is one.
    fn entry_field(&self) -> DateField {
        DateField::on(self.today, self.entry_date.unwrap_or(self.today))
    }

    fn open_delete(&mut self) {
        match self.ledger().selected().cloned() {
            None => self.status = NOTHING_SELECTED.to_string(),
            Some(row) => {
                let label = format!(
                    "{}  {}  {}",
                    row.date,
                    description::render(&row.description),
                    crate::demo::figure(row.cents)
                );
                self.modal = Some(Modal::Confirm {
                    action: Confirm::DeleteTxn(row.id),
                    label,
                });
            }
        }
    }

    fn open_allocate(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container) = (row.goal_id, row.name.clone(), row.container.id());
        // The pot `/N` divides. A container with nothing unallocated is not an
        // error -- the form still opens, and `/N` there is zero.
        let unallocated = self
            .savings
            .excess()
            .iter()
            .find(|(id, _)| *id == container)
            .map(|(_, cents)| *cents)
            .unwrap_or(Cents::ZERO);
        self.modal = Some(Modal::Allocation(AllocationForm::new(
            goal_id,
            &name,
            self.savings.account_name(container),
            unallocated,
            self.today,
        )));
        Ok(())
    }

    fn commit_allocation(&mut self) -> Result<()> {
        let Some(Modal::Allocation(form)) = &self.modal else {
            return Ok(());
        };
        let allocated = form.commit()?;
        goal::insert_allocation(
            &self.db,
            form.goal_id,
            allocated.date,
            allocated.cents,
            allocated.note.as_deref(),
            None,
        )?;
        self.status = format!(
            "allocated {} · U undoes the last batch, not this",
            crate::demo::figure(allocated.cents)
        );
        self.close_modal();
        self.reload()
    }

    /// `n`: a goal typed from scratch, in the container the screen defaults
    /// to. The container is checked here rather than only at commit, so the
    /// form never opens over a container it invented.
    fn open_new_goal(&mut self) -> Result<()> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(());
        };
        let account = account::get(&self.db, container)?;
        self.modal = Some(Modal::Goal(GoalForm::add(
            Account::named(std::slice::from_ref(&account), container),
            setting::get(&self.db, key::TAX_RATE)?,
            self.today,
        )));
        Ok(())
    }

    fn open_goal_edit(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        self.modal = Some(Modal::Goal(GoalForm::new(
            row.goal_id,
            &row.name,
            row.base,
            row.goal_date,
            row.interest_eligible,
            row.taxed,
            setting::get(&self.db, key::TAX_RATE)?,
            self.today,
        )));
        Ok(())
    }

    /// `e` and `n` share a form, so they share a commit: an id means the goal
    /// exists and is being edited, and none means it is being created in the
    /// container the screen defaults to.
    fn commit_goal(&mut self) -> Result<()> {
        let Some(Modal::Goal(form)) = &self.modal else {
            return Ok(());
        };
        let target = form.target();
        let edit = form.commit()?;
        match target {
            GoalTarget::Update(id) => {
                goal::update(&self.db, id, &edit)?;
                self.status = format!("updated {}", edit.name);
            }
            GoalTarget::Create(container) => {
                goal::insert(
                    &self.db,
                    &goal::NewGoal {
                        name: edit.name.clone(),
                        container_account_id: container,
                        base_cents: edit.base_cents,
                        goal_date: edit.goal_date,
                        // A free-form goal answers to no recurring entry.
                        recurring_goal_id: None,
                        interest_eligible: edit.interest_eligible,
                        sort: goal::next_sort(&self.db, container)?,
                        taxed: edit.taxed,
                    },
                )?;
                self.status = format!("created {}", edit.name);
            }
        }
        self.close_modal();
        self.reload()
    }

    /// Move the selected undated goal one place in its container's manual
    /// order, and put the cursor back on it.
    ///
    /// Two refusals rather than two silences. A dated goal takes its place
    /// from its date, so there is no manual order for it to move in; and a
    /// kept search hides part of the block being reordered, so a move would
    /// be one place in a list the owner cannot see. Either one says so,
    /// because a key that sometimes quietly does nothing is a key nobody
    /// trusts.
    ///
    /// The position is computed against the container's undated goals rather
    /// than against the rows on screen: `goal::reorder` renumbers that block,
    /// and the two must be counting the same list. `reload_savings` then
    /// redraws from the table, and the cursor is put back **by id** -- the
    /// rows moved under it, so an index would leave the selection on whatever
    /// took the vacated place and the next press would move that goal
    /// instead.
    fn move_goal(&mut self, direction: Move) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (id, container, name, dated) = (
            row.goal_id,
            row.container.id(),
            row.name.clone(),
            row.goal_date.is_some(),
        );
        if !self.savings.search().is_empty() {
            self.status = "Clear the search before reordering goals".to_string();
            return Ok(());
        }
        if dated {
            self.status = format!("{name} has a goal date, so its place comes from that date");
            return Ok(());
        }
        let undated: Vec<GoalId> = goal::list(&self.db, container)?
            .into_iter()
            .filter(|g| g.goal_date.is_none())
            .map(|g| g.id)
            .collect();
        let from = undated
            .iter()
            .position(|g| *g == id)
            .context("the selected goal is open and undated, so its container lists it")?;
        let Some(to) = direction.applied(from, undated.len()) else {
            return Ok(());
        };
        goal::reorder(&self.db, id, to)?;
        self.reload_savings()?;
        self.savings.select_goal(id);
        Ok(())
    }

    /// Mark or unmark the selected goal, and redraw the screen from the
    /// database.
    ///
    /// `reload_savings` rather than a write to the row in hand: the row is a
    /// copy of what the query returned, and one write that updated the copy
    /// instead of re-reading is how a screen starts disagreeing with the
    /// table under it. It is also cheap here -- every goal is already loaded
    /// for the reconciliation line -- and it keeps the cursor, which is an
    /// index into rows this does not reorder.
    fn toggle_favorite(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (id, favorite) = (row.goal_id, row.favorite);
        goal::set_favorite(&self.db, id, !favorite)?;
        self.reload_savings()
    }

    fn open_close_out(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container, current) = (
            row.goal_id,
            row.name.clone(),
            row.container.id(),
            row.current,
        );
        // Built from the container, not from the screen's filtered rows: a
        // search must not narrow what a close-out may move value into.
        let siblings = goal::list_with_balances(&self.db, container)?
            .into_iter()
            .filter(|g| g.goal.id != goal_id)
            .map(|g| (g.goal.id, g.goal.name))
            .collect();
        self.modal = Some(Modal::CloseOut(CloseForm::new(
            goal_id, &name, current, siblings, self.today,
        )));
        Ok(())
    }

    fn commit_close_out(&mut self) -> Result<()> {
        let Some(Modal::CloseOut(form)) = &self.modal else {
            return Ok(());
        };
        let ending = form.commit()?;
        goal::move_value(&self.db, form.goal_id, ending.to, ending.date)?;
        self.status = match ending.to {
            None => "closed, value returned to unallocated · U undoes the last batch, not this"
                .to_string(),
            Some(_) => "closed, value moved · U undoes the last batch, not this".to_string(),
        };
        self.close_modal();
        self.reload()
    }

    /// The worksheet is not a field form: its keys are line editing, not
    /// `Tab`-through-fields with autocomplete, so it does not go through
    /// `form_key`.
    fn worksheet_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(Modal::Worksheet(sheet)) = &mut self.modal else {
            return Ok(());
        };
        // `/` waits for the next key: a digit is the fraction operator,
        // anything else begins a name filter.
        if sheet.is_pending_slash() {
            sheet.cancel_pending_slash();
            return match key.code {
                // The worksheet is a context that takes the editing keys, so
                // `App::dispatch` lets a `Ctrl` through to reach its date --
                // and this is one of the two arms here that must not read one
                // as the letter it arrives as. The other is the operators'.
                KeyCode::Char(_) if !text::is_bare(key) => Ok(()),
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    sheet.divide(c.to_digit(10).expect("checked above") as i64)
                }
                KeyCode::Char(c) => {
                    sheet.begin_search();
                    sheet.push_search(c);
                    Ok(())
                }
                _ => Ok(()),
            };
        }
        if sheet.is_searching() {
            search::search_key(sheet, key);
            return Ok(());
        }
        if cursor::scroll_key(sheet, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if search::escape_kept_filter(sheet) {
                    return Ok(());
                }
                self.close_modal();
                self.status = "cancelled".to_string();
                return self.open_next_worksheet();
            }
            KeyCode::Enter => return self.commit_worksheet(),
            KeyCode::Tab => sheet.next_focus(),
            KeyCode::BackTab => sheet.previous_focus(),
            KeyCode::Backspace => sheet.backspace(),
            // Only the date focus has a date to move; `step_date` is where
            // that is decided, so the two digit focuses are unreachable from
            // here.
            KeyCode::Left => sheet.step_date(week_step(key, Step::PREVIOUS_WEEK, Step::PREVIOUS)),
            KeyCode::Right => sheet.step_date(week_step(key, Step::NEXT_WEEK, Step::NEXT)),
            // The operators are line-editing keys, but they are live from the
            // amount too: that field takes digits and drops everything else,
            // so gating them on `Lines` only made them dead keys on the
            // worksheet as it opens. The date is a text field -- `-` is part
            // of a date, and `s` must not spread while one is being fixed --
            // so it is the one focus that types them instead.
            //
            // A modified character never reaches them: `Ctrl` means editing
            // text everywhere in the app and `Alt` means nothing anywhere, and
            // a hand reaching for `Ctrl`+`W` on the amount would otherwise
            // spread the whole pot.
            KeyCode::Char(c) if sheet.focus() != worksheet::Focus::Date && text::is_bare(key) => {
                match c {
                    ' ' => sheet.toggle_selection(),
                    '*' => sheet.select_all_visible(),
                    '-' => sheet.clear_selection(),
                    'z' => sheet.zero_untargeted(),
                    's' => sheet.spread()?,
                    'w' => sheet.spread_by_weight()?,
                    '/' => sheet.slash(),
                    _ => sheet.type_char(c),
                }
            }
            // The date takes the character itself and the editing keys alike;
            // for the other two focuses this is where a modified one stops.
            _ => {
                sheet.edit(key);
            }
        }
        Ok(())
    }

    /// Open the next queued worksheet, if any.
    ///
    /// The worksheets open prefilled rather than posting directly, so the
    /// numbers are reviewable and cancellable before they become an undoable
    /// batch.
    fn open_next_worksheet(&mut self) -> Result<()> {
        // Taken rather than borrowed: if building the sheet below fails, the
        // `?` must not leave a non-empty queue behind with no worksheet on
        // screen to drain it -- that is what would let an unrelated `Esc` or
        // `Enter` resurrect it later. The tail is put back only once the
        // sheet is actually on screen.
        let mut queue = std::mem::take(&mut self.pending_worksheets);
        if queue.is_empty() {
            return Ok(());
        }
        let (container, pot, shares) = queue.remove(0);
        let mut prefill = Vec::new();
        for g in goal::list_with_balances(&self.db, container)? {
            prefill.push((g.goal.id, g.goal.name, Cents::ZERO));
        }
        let account = account::get(&self.db, container)?;
        let mut sheet = Worksheet::new(
            goal::BatchKind::Paycheck,
            Account::named(std::slice::from_ref(&account), container),
            self.today,
            prefill,
        );
        sheet.set_amount(pot);
        sheet.set_lines(&shares);
        self.modal = Some(Modal::Worksheet(sheet));
        self.pending_worksheets = queue;
        Ok(())
    }

    /// Opens on the container the screen is filtered to, or the first when the
    /// filter is All. Payday means running it twice -- once per container --
    /// which is what Planning instructs and what physically happens.
    fn open_payday(&mut self) -> Result<()> {
        let sheet = self.new_worksheet(goal::BatchKind::Paycheck)?;
        if let Some(sheet) = sheet {
            self.modal = Some(Modal::Worksheet(sheet));
        }
        Ok(())
    }

    /// The worksheet every entry point starts from: this container's open
    /// goals, each prefilled with what `per_paycheck` asks of it.
    fn new_worksheet(&mut self, kind: goal::BatchKind) -> Result<Option<Worksheet>> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(None);
        };
        let mut prefill = Vec::new();
        for g in goal_engine::list_with_balances(&self.db, container)? {
            let ask = crate::savings::paycheck_ask(&g, self.today, self.period_days)?;
            prefill.push((g.goal.id, g.goal.name, ask.unwrap_or(Cents::ZERO)));
        }
        let account = account::get(&self.db, container)?;
        Ok(Some(Worksheet::new(
            kind,
            Account::named(std::slice::from_ref(&account), container),
            self.today,
            prefill,
        )))
    }

    /// `i` opens the worksheet on the container's excess, with the shares its
    /// policy prefers.
    ///
    /// The lines are every open goal, not only the eligible ones: the prefill
    /// decides where the money starts, and the owner may still move it.
    fn open_interest(&mut self) -> Result<()> {
        let Some(mut sheet) = self.new_worksheet(goal::BatchKind::Interest)? else {
            return Ok(());
        };
        let container = sheet.container();
        let goals = goal::list_with_balances(&self.db, container)?;
        let eligible: Vec<(GoalId, Cents)> = goals
            .iter()
            .filter(|g| g.goal.interest_eligible)
            .map(|g| (g.goal.id, g.current))
            .collect();
        // Handed over whole: `interest_prefill` keeps the weights inside
        // `eligible`, so a goal closed or made ineligible since the last
        // posting drops out there and `pro_rata` renormalizes over the rest.
        let previous = match goal::last_batch(&self.db, goal::BatchKind::Interest, container)? {
            None => Vec::new(),
            Some(batch) => goal::batch_shares(&self.db, batch.id)?,
        };
        // Clamped at zero: `pro_rata` refuses a negative total, and an
        // over-allocated container is a state to fix by hand, not to split.
        let total = goal::container_excess(&self.db, container)?.max(Cents::ZERO);
        let policy = account::interest_policy(&self.db, container)?;
        let shares = worksheet::interest_prefill(policy, total, &eligible, &previous)?;
        sheet.set_amount(total);
        sheet.set_lines(&shares);
        self.modal = Some(Modal::Worksheet(sheet));
        Ok(())
    }

    fn commit_worksheet(&mut self) -> Result<()> {
        let Some(Modal::Worksheet(sheet)) = &self.modal else {
            return Ok(());
        };
        let committed = sheet.commit()?;
        let kind = sheet.kind();
        let total: Cents = committed.shares.iter().map(|(_, c)| *c).sum();
        goal::insert_allocations(&self.db, kind, committed.date, &committed.shares, None)?;
        self.status = format!(
            "posted {} across {} goals · U undoes it",
            crate::demo::figure(total),
            committed.shares.len()
        );
        self.close_modal();
        // Taken rather than borrowed, the same as `commit_plan_transfers`:
        // if `reload` fails, the `?` must not leave a non-empty queue behind
        // with no worksheet on screen to drain it -- that is what would let
        // some unrelated `Esc` or `Enter` resurrect a stale worksheet from
        // this payday later, prefilled with amounts already posted.
        let queue = std::mem::take(&mut self.pending_worksheets);
        self.reload()?;
        self.pending_worksheets = queue;
        self.open_next_worksheet()
    }

    /// The most recent batch, whatever it was. Never an `Import` batch:
    /// `goal::most_recent_batch` excludes it, because that one holds every
    /// opening balance in the database.
    fn open_undo(&mut self) -> Result<()> {
        let Some(batch) = goal::most_recent_batch(&self.db)? else {
            self.status = "nothing to undo".to_string();
            return Ok(());
        };
        let shares = goal::batch_shares(&self.db, batch.id)?;
        let total: Cents = shares.iter().map(|(_, c)| *c).sum();
        let label = format!(
            "{} {} · {} goals · {}",
            batch.kind.as_str(),
            batch.date,
            shares.len(),
            crate::demo::figure(total)
        );
        self.modal = Some(Modal::Confirm {
            action: Confirm::UndoBatch(batch.id),
            label,
        });
        Ok(())
    }

    /// `n`: the recurring goal, to create the next round of goals from.
    ///
    /// A goal ends and never becomes its successor, so the next round is
    /// created here rather than rolled forward on the goal that just closed.
    /// Opens on the container the screen is filtered to, or the first when the
    /// filter is All.
    /// `s` on the Recurring Goals screen: goals created *from* the entries the
    /// screen is showing.
    ///
    /// The container is the Savings screen's `Tab` filter, which is the app's
    /// one answer to "which container" -- this screen's entries carry none.
    /// The month filter preselects rather than narrows, and an entry that
    /// already has an open goal is left unticked: the annual reseed is what
    /// the preselection is for, and such an entry has already been through it.
    /// A second open goal against one entry is still legitimate, so `Space`
    /// still adds it.
    fn open_recurring_goals(&mut self) -> Result<()> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(());
        };
        let entries = recurring_goal::list(&self.db)?;
        if entries.is_empty() {
            self.status = "there are no recurring goals".to_string();
            return Ok(());
        }
        let counts = recurring_goal::open_goal_counts(&self.db)?;
        let month = self.recurring_goal.selected_month();
        let preselected: HashSet<RecurringGoalId> = entries
            .iter()
            .filter(|e| month.is_none_or(|m| e.month == m))
            .filter(|e| counts.get(&e.id).copied().unwrap_or(0) == 0)
            .map(|e| e.id)
            .collect();
        let account = account::get(&self.db, container)?;
        self.modal = Some(Modal::Picker(Picker::new(
            entries,
            counts,
            &preselected,
            Account::named(std::slice::from_ref(&account), container),
        )));
        Ok(())
    }

    fn picker_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Enter {
            return self.commit_picker();
        }
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return Ok(());
        };
        if cursor::scroll_key(picker, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.close_modal(),
            KeyCode::Char(' ') => picker.toggle(),
            _ => {}
        }
        Ok(())
    }

    /// Create one goal per selected entry, all in one transaction.
    ///
    /// Nothing here refuses an entry that already has an open goal: goal names
    /// are not unique, and a second open goal against one recurring goal entry
    /// is a legitimate thing to want. The picker's "Open?" column is the hint.
    fn commit_picker(&mut self) -> Result<()> {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return Ok(());
        };
        let container = picker.container();
        let chosen: Vec<Entry> = picker.chosen().into_iter().cloned().collect();
        ensure!(!chosen.is_empty(), NOTHING_SELECTED);
        // The picker is the second place a taxed goal is written, alongside
        // the goal form's own commit, and it is the one that hands the flag
        // across rather than computing anything -- so it has to ask for the
        // rate here, before there is a goal for the read side to call corrupt.
        if chosen.iter().any(|entry| entry.taxed) {
            ensure!(
                setting::get(&self.db, key::TAX_RATE)?.is_some(),
                goal_engine::NO_TAX_RATE
            );
        }
        // Every goal created here is dated, so each takes its place in the
        // container's dated block by deadline rather than landing at the end.
        // `sort` still runs in the order the picker showed them -- the ticked
        // group first, since that is the order the picker sorted itself into --
        // but among dated goals it decides only which of two falling on the
        // same day comes first.
        let first_sort = goal::next_sort(&self.db, container)?;
        let mut new_goals = Vec::with_capacity(chosen.len());
        for (offset, entry) in chosen.iter().enumerate() {
            let has_goal_this_year =
                goal::has_goal_dated_in_year(&self.db, entry.id, self.today.year())?;
            new_goals.push(goal::NewGoal {
                name: entry.name.clone(),
                container_account_id: container,
                // The entry's base and its flag, handed across rather than
                // spent: a goal made from a taxed entry is indistinguishable
                // from one the owner marked taxed by hand, and the lambda runs
                // once, on read.
                base_cents: entry.base_cents,
                goal_date: Some(picker::goal_date(entry, has_goal_this_year, self.today)?),
                recurring_goal_id: Some(entry.id),
                interest_eligible: true,
                sort: first_sort + offset as i64,
                taxed: entry.taxed,
            });
        }
        goal::insert_all(&self.db, &new_goals)?;
        self.status = format!("created {} goals", new_goals.len());
        self.close_modal();
        self.reload()
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

    /// One or more characters in a form's description field opens the popup;
    /// an empty field, or a form with no description field at all, closes it.
    fn refresh_suggestions(&mut self) -> Result<()> {
        let prefix = self
            .modal
            .as_mut()
            .and_then(Modal::fields_mut)
            .and_then(|f| f.suggestion_prefix())
            .unwrap_or_default()
            .to_string();
        if prefix.is_empty() {
            self.popup.clear();
            return Ok(());
        }
        let hits = txn::autocomplete(&self.db, &prefix, Autocomplete::LIMIT)?;
        self.popup.set(hits);
        Ok(())
    }

    fn commit_txn_form(&mut self) -> Result<()> {
        let Some(Modal::Txn(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                txn::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                txn::insert(&self.db, &new)?;
                // Only an add. An edit is a correction to a row already
                // written rather than a statement about the day being worked
                // on, and fixing something months back must not drag the next
                // new row there with it.
                self.entry_date = Some(new.date);
                "added"
            }
        };
        // The date is named whatever it is. The form no longer opens on the
        // date field, so a row can be written without a keystroke ever
        // visiting it, and the confirmation is the only place the day it
        // landed on appears -- unconditionally, because a line that spoke up
        // only when the date was surprising is a line the eye learns to skip
        // on the rounds it says nothing.
        self.status = format!(
            "{verb} {} {} on {}",
            description::render(&new.description),
            crate::demo::figure(new.cents),
            new.date
        );
        self.close_modal();
        self.reload()
    }

    fn commit_transfer_form(&mut self) -> Result<()> {
        let Some(Modal::Transfer(form)) = &self.modal else {
            return Ok(());
        };
        let moved = form.commit()?;
        // One description, used for both legs.
        txn::insert_transfer(
            &self.db,
            moved.from_account_id,
            moved.to_account_id,
            moved.date,
            moved.cents,
            &moved.description,
            &moved.description,
        )?;
        // Both legs are new rows, so this is a statement about the day being
        // entered for in the way an `e` is not, and it names its date for the
        // reason `commit_txn_form` does.
        self.entry_date = Some(moved.date);
        self.status = format!(
            "{} {} on {}",
            moved.description,
            crate::demo::figure(moved.cents),
            moved.date
        );
        self.close_modal();
        self.reload()
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let [tab_area, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // Abbreviated on purpose. The bar is a row of shortcuts, not a set
        // of headings: spelled out, "7 Recurring Goals" and "8 Recurring
        // Txns" spend fourteen columns restating what the screen's own title
        // and footer say the moment it is opened.
        frame.render_widget(
            Tabs::new(vec![
                "1 Overview",
                "2 Cash",
                "3 Credit",
                "4 Savings",
                "5 Planning",
                "6 Funds",
                "7 Goals",
                "8 Txns",
                "9 Accounts",
            ])
            .select(self.screen as usize)
            .divider("│"),
            tab_area,
        );

        match self.screen {
            Screen::Overview => {
                overview::render(frame, body, &self.overview, self.scrubbed_days() != 0)
            }
            // The viewport height comes back out of the draw so `PageUp` and
            // `PageDown` move by a screenful, the same way the autocomplete
            // popup's drawn-row count does below.
            Screen::Cash | Screen::Credit => {
                let height = ledger::render(frame, body, self.ledger(), self.today);
                self.ledger_mut().set_page_height(height);
            }
            Screen::Savings => {
                let height = savings::render(frame, body, &self.savings);
                self.savings.set_page_height(height);
            }
            Screen::Planning => {
                let height = planning::render(frame, body, &self.planning);
                self.planning.set_page_height(height);
            }
            Screen::Funds => {
                let height = fund_screen::render(frame, body, &self.funds);
                self.funds.set_page_height(height);
            }
            Screen::RecurringTxns => {
                let height = recurring_txn_screen::render(frame, body, &self.recurring_txn);
                self.recurring_txn.set_page_height(height);
            }
            Screen::RecurringGoals => {
                let height = recurring_goal_screen::render(frame, body, &self.recurring_goal);
                self.recurring_goal.set_page_height(height);
            }
            Screen::Accounts => {
                let height = accounts_screen::render(frame, body, &self.accounts);
                self.accounts.set_page_height(height);
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
    use crate::db;
    use crate::db::account::Group;
    use crate::db::goal;
    use crate::db::txn::{Filter, NewTxn};
    use crate::gate::Gate;
    use crate::money::Cents;
    use crate::plan_line::Line;
    use crate::rate::{BasisPoints, Percent};
    use crate::tui::MIN_WIDTH;
    use crate::tui::form::{TransferField, TxnField};
    use crate::tui::goal_form;
    use crate::tui::worksheet::Worksheet;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn today() -> NaiveDate {
        day(2026, 8, 15)
    }

    fn write(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64, what: &str) {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: Cents(cents),
                account_id,
                description: what.to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    /// Two cash accounts, two cards, and a handful of rows in the month the
    /// ledgers open on, including one dated after today. Enough for the
    /// cursor, the `Tab` filter, and autocomplete to have something to act
    /// on.
    fn app() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        let card_two = account::insert(&db, "CC2", "Card Two", Kind::Credit, 1).unwrap();
        write(&db, checking, day(2026, 8, 1), 100_000, "Paycheck");
        write(&db, checking, day(2026, 8, 10), -5_000, "Whole Foods");
        write(&db, savings, day(2026, 8, 12), 20_000, "Transfer");
        write(&db, checking, day(2026, 8, 20), -120_000, "Rent");
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        write(&db, card_two, day(2026, 8, 13), 2_599, "Batteries");
        let vacation = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Vacation 2027".to_string(),
                container_account_id: savings,
                base_cents: Cents(1_500_000),
                goal_date: Some(day(2027, 1, 1)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, vacation, day(2026, 8, 1), Cents(1_000_000), None, None)
            .unwrap();
        let couch = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents(100_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 1,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, couch, day(2026, 8, 1), Cents(25_000), None, None).unwrap();
        App::new(db, today()).unwrap()
    }

    fn savings_names(app: &App) -> Vec<String> {
        app.savings.rows().iter().map(|r| r.name.clone()).collect()
    }

    /// The footer as text. It is a `Line` because the `/` box draws a caret
    /// into it, and every assertion here is about the words.
    fn footer(app: &App) -> String {
        app.footer()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

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

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// A `Ctrl` combination -- the editing keys, and nothing else in the app.
    fn ctrl_press(app: &mut App, c: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    /// The same key with Shift held, which crossterm reports as the arrow
    /// plus a modifier rather than a code of its own.
    fn shift_press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::SHIFT));
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// One container holding three undated goals and one dated, for the
    /// manual order `K` and `J` move things around in.
    fn app_with_undated_goals() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        for (i, (name, date)) in [
            ("Couch", None),
            ("Bike", None),
            ("Camera", None),
            ("Vacation 2027", Some(day(2027, 1, 1))),
        ]
        .into_iter()
        .enumerate()
        {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: savings,
                    base_cents: Cents(100_000),
                    goal_date: date,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort: i as i64,
                    taxed: false,
                },
            )
            .unwrap();
        }
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));
        app
    }

    /// The whole point of the manual order: the owner arranges the goals no
    /// deadline arranges for them.
    #[test]
    fn k_moves_the_selected_undated_goal_up() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.savings.selected().unwrap().name, "Camera");

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Camera", "Bike", "Vacation 2027"]
        );
    }

    /// The rows move under the cursor, so the cursor has to be put back on
    /// the goal by id -- an index kept across the move would leave the
    /// selection on whichever goal took the vacated place, and a second press
    /// would move that one instead.
    #[test]
    fn the_cursor_follows_the_goal_it_moved() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.savings.selected().unwrap().name, "Camera");
        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Camera", "Couch", "Bike", "Vacation 2027"],
            "already first, so the second press had nowhere to go"
        );
        assert_eq!(app.savings.selected().unwrap().name, "Camera");
    }

    #[test]
    fn j_moves_the_selected_undated_goal_down() {
        let mut app = app_with_undated_goals();
        assert_eq!(app.savings.selected().unwrap().name, "Couch");

        press(&mut app, KeyCode::Char('J'));

        assert_eq!(
            savings_names(&app),
            vec!["Bike", "Couch", "Camera", "Vacation 2027"]
        );
        assert_eq!(app.savings.selected().unwrap().name, "Couch");
    }

    #[test]
    fn moving_the_last_undated_goal_down_changes_nothing() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('J'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Bike", "Camera", "Vacation 2027"],
            "the dated block below is not somewhere an undated goal can move into"
        );
    }

    /// A dated goal's place comes from its date, so the key says so rather
    /// than doing nothing and leaving the owner to wonder which it was.
    #[test]
    fn moving_a_dated_goal_is_refused_with_a_message() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::End);
        assert_eq!(app.savings.selected().unwrap().name, "Vacation 2027");

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Bike", "Camera", "Vacation 2027"]
        );
        assert!(
            app.status.contains("goal date"),
            "said nothing about why: {}",
            app.status
        );
    }

    /// A kept search hides part of the block being reordered, so a move
    /// would be one place in a list the owner cannot see. Refused rather
    /// than guessed at.
    #[test]
    fn moving_while_a_kept_search_narrows_the_list_is_refused() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "e");
        press(&mut app, KeyCode::Enter);
        assert_eq!(savings_names(&app), vec!["Bike", "Camera"]);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(savings_names(&app), vec!["Bike", "Camera"]);
        assert!(
            app.status.contains("search"),
            "said nothing about the search: {}",
            app.status
        );
    }

    fn savings_favorites(app: &App) -> Vec<bool> {
        app.savings.rows().iter().map(|r| r.favorite).collect()
    }

    /// `f` is a toggle over the selected row, written straight through: there
    /// is nothing to confirm and nothing to type, so a modal would be a
    /// keystroke asking whether the owner meant the keystroke.
    #[test]
    fn f_marks_the_selected_goal_and_pressing_it_again_takes_the_mark_back() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(savings_favorites(&app), vec![false, false]);

        press(&mut app, KeyCode::Char('f'));
        assert_eq!(savings_favorites(&app), vec![true, false]);

        press(&mut app, KeyCode::Char('f'));
        assert_eq!(savings_favorites(&app), vec![false, false]);
    }

    /// The write has to reach the database, not just the row: a mark that
    /// lived on the view would be gone at the next reload and the owner would
    /// not find out until the next launch.
    #[test]
    fn a_mark_survives_a_reload() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('f'));

        app.reload().unwrap();

        assert_eq!(savings_favorites(&app), vec![true, false]);
    }

    /// The mark is a highlight, so it must not move the cursor off the row it
    /// was pressed on -- pressing `f` twice has to be the same row twice.
    #[test]
    fn marking_a_goal_leaves_the_cursor_where_it_was() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.selected().unwrap().goal_id;

        press(&mut app, KeyCode::Char('f'));

        assert_eq!(app.savings.selected().unwrap().goal_id, before);
        assert_eq!(savings_favorites(&app), vec![false, true]);
    }

    /// Every other row key on this screen says so rather than doing nothing,
    /// and an empty list is the state a fresh database opens in.
    #[test]
    fn f_with_nothing_selected_says_so() {
        let db = db::open_in_memory().unwrap();
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));

        press(&mut app, KeyCode::Char('f'));

        assert!(!app.status.is_empty(), "{:?}", app.status);
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

    /// Tab from the field the form opens on to `field`.
    ///
    /// The likely failure is not a closed form but an open autocomplete
    /// popup: it answers `Tab` before the form does, so a description
    /// matching a row already written leaves the focus where it was through
    /// every press. Say which, and on which field, rather than blaming a
    /// form that is sitting right there.
    fn focus(app: &mut App, field: TxnField) {
        for _ in 0..TxnField::ORDER.len() {
            match &app.modal {
                Some(Modal::Txn(form)) if form.focus == field => return,
                _ => press(app, KeyCode::Tab),
            }
        }
        let state = match &app.modal {
            Some(Modal::Txn(form)) => format!("the focus stuck on {}", form.focus.label()),
            Some(_) => "another modal is open".to_string(),
            None => "no transaction form is open".to_string(),
        };
        panic!(
            "{} never took focus: {state}, with {} suggestions showing",
            field.label(),
            app.popup.visible()
        );
    }

    fn form(app: &App) -> &TxnForm {
        match &app.modal {
            Some(Modal::Txn(form)) => form,
            _ => panic!("no transaction form is open"),
        }
    }

    /// Add a row through the keyboard, stepping the date `days` from wherever
    /// the form opens. The description must match nothing already written, or
    /// `Tab` and `Enter` would be answering the autocomplete popup instead of
    /// the form.
    fn add_row(app: &mut App, days: usize, description: &str) {
        press(app, KeyCode::Char('a'));
        focus(app, TxnField::Date);
        for _ in 0..days {
            press(app, KeyCode::Right);
        }
        focus(app, TxnField::Description);
        type_str(app, description);
        focus(app, TxnField::Amount);
        type_str(app, "10");
        press(app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
    }

    fn form_date(app: &App) -> String {
        form(app).display(TxnField::Date).plain_text()
    }

    fn transfer_date(app: &App) -> String {
        match &app.modal {
            Some(Modal::Transfer(form)) => form.display(TransferField::Date).plain_text(),
            _ => panic!("no transfer form is open"),
        }
    }

    /// The first row of a session has nothing behind it to take a date from,
    /// so the form opens where every other date field in the app does.
    #[test]
    fn the_first_add_of_a_session_opens_on_today() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));

        assert_eq!(form_date(&app), "2026-08-15");
    }

    /// Entering a statement is a run of rows landing on the same few days, so
    /// the day the last one was written for is a better opening guess than
    /// today -- and the owner who has moved off today said so by moving.
    #[test]
    fn an_add_opens_on_the_date_the_last_row_was_added_with() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// The date is the day being entered for rather than a property of either
    /// ledger, so it survives the walk from one to the other: a statement and
    /// the card rows on it are the same sitting.
    #[test]
    fn the_date_carries_from_one_ledger_to_the_other() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// `t` and `p` write ledger rows in the same sitting `a` does -- the
    /// card payment at the bottom of the statement whose rows were just
    /// entered -- so they open where `a` does rather than back on today.
    #[test]
    fn a_transfer_and_a_payment_open_on_the_date_the_last_row_was_added_with() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(transfer_date(&app), "2026-08-20");
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('p'));
        assert_eq!(transfer_date(&app), "2026-08-20");
    }

    /// Both legs are new rows, so a transfer is a statement about the day
    /// being entered for in the way an edit is not.
    #[test]
    fn committing_a_payment_moves_the_date_the_next_add_opens_on() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        for _ in 0..5 {
            press(&mut app, KeyCode::Right);
        }
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
        }
        type_str(&mut app, "10");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// An edit is a correction to a row already written, not a statement
    /// about where the hand is working. Fixing the date on something months
    /// back must not drag the next new row there with it.
    #[test]
    fn editing_a_row_leaves_the_date_the_next_add_opens_on_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('e'));
        focus(&mut app, TxnField::Date);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the edit stayed open: {}", app.status);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    fn descriptions(ledger: &Ledger) -> Vec<&str> {
        ledger
            .rows()
            .iter()
            .map(|t| t.description.as_str())
            .collect()
    }

    /// Rows in July, August and September, so `[` and `]` have somewhere to
    /// go: the window is clamped to the months the data covers, and the
    /// single-month fixture above cannot step at all.
    fn app_spanning_three_months() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, checking, day(2026, 7, 5), 100_000, "July");
        write(&db, checking, day(2026, 8, 5), 100_000, "August");
        write(&db, checking, day(2026, 9, 5), 100_000, "September");
        write(&db, card_one, day(2026, 7, 6), 1_000, "July card");
        write(&db, card_one, day(2026, 9, 6), 1_000, "September card");
        App::new(db, today()).unwrap()
    }

    #[test]
    fn stepping_the_month_on_cash_steps_credit_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.credit.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));

        assert_ne!(app.cash.window(), august, "] must move the active ledger");
        assert_eq!(app.credit.window(), app.cash.window());
    }

    #[test]
    fn stepping_the_month_on_credit_steps_cash_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('['));

        assert_ne!(app.credit.window(), august, "[ must move the active ledger");
        assert_eq!(app.cash.window(), app.credit.window());
    }

    /// The ledgers have no All to clear to -- their window is pushed down
    /// into the SQL -- so `Esc` means "back to the window the screen opens
    /// on", and it takes the other ledger with it the way `[` and `]` do.
    #[test]
    fn esc_returns_both_ledgers_to_the_window_around_today() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));
        assert_ne!(app.cash.window(), august);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.window(), august);
        assert_eq!(app.credit.window(), august);
    }

    #[test]
    fn esc_on_credit_returns_cash_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.credit.window(), august);
        assert_eq!(app.cash.window(), august);
    }

    /// The other ledger is re-queried too: a synced window over stale rows
    /// would show August's rows under a September heading.
    #[test]
    fn the_other_ledgers_rows_follow_the_shared_month() {
        let mut app = app_spanning_three_months();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));

        assert_eq!(descriptions(&app.credit), ["September card"]);
    }

    /// `BackTab` is `Tab`'s cycle read backwards, and it re-queries the same
    /// way: a filter moved without the rows following it shows one account's
    /// rows under another's heading.
    #[test]
    fn back_tab_steps_the_ledgers_account_filter_the_other_way() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Transfer"],
            "the last cash account"
        );

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Rent"],
            "the first"
        );

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"],
            "All"
        );
    }

    #[test]
    fn t_opens_a_transfer_form_on_the_cash_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));
        assert!(matches!(app.modal, Some(Modal::Transfer(_))));
    }

    /// A transfer leaves an account you hold, so the key has nothing to start
    /// from on the Credit ledger. `p` is the move that belongs to a card.
    #[test]
    fn t_opens_nothing_on_the_credit_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('t'));
        assert!(app.modal.is_none());
    }

    #[test]
    fn p_still_opens_a_payment_form_on_the_credit_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        assert!(matches!(app.modal, Some(Modal::Transfer(_))));
    }

    /// The cursor opens on the last row dated on or before today, so `d`
    /// takes "Transfer" rather than the first row of the month.
    #[test]
    fn d_then_y_deletes_the_selected_row() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"]
        );

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(descriptions(&app.cash), ["Paycheck", "Whole Foods", "Rent"]);
        assert_eq!(app.status, "deleted");
        assert!(app.modal.is_none());
    }

    /// Opening on the first row of the month would put the cursor hundreds of
    /// rows from the ones just entered.
    #[test]
    fn the_ledger_opens_on_the_last_row_dated_on_or_before_today() {
        let app = app();
        assert_eq!(app.cash.selected().unwrap().description, "Transfer");
    }

    /// `Rent` is dated after today, so anchoring has to skip back over it
    /// rather than simply taking the last row.
    #[test]
    fn a_deleted_row_does_not_send_the_cursor_back_to_today() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::End);
        assert_eq!(app.cash.selected().unwrap().description, "Rent");

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(
            app.cash.selected().unwrap().description,
            "Transfer",
            "the cursor stays at its index, which is now the last row"
        );
    }

    /// The confirmation exists because there is no undo, so anything that is
    /// not `y` has to be a cancel rather than a fall-through.
    #[test]
    fn d_then_any_other_key_cancels_and_the_row_survives() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('n'));

        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"]
        );
        assert_eq!(app.status, "delete cancelled");
        assert!(app.modal.is_none());
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
        for tab in [
            "1 Overview",
            "2 Cash",
            "3 Credit",
            "4 Savings",
            "5 Planning",
            "6 Funds",
            "7 Goals",
            "8 Txns",
        ] {
            assert!(bar.contains(tab), "{tab:?} is cut off: {bar:?}");
        }
    }

    /// Everything the screen draws, as one string per test to search.
    fn drawn(app: &mut App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..24)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// One form, two jobs, so the border is the only thing on screen that says
    /// which one is happening.
    #[test]
    fn the_goal_modal_names_the_job_it_is_doing() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        assert!(drawn(&mut app).contains("New goal"));

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('e'));
        assert!(drawn(&mut app).contains("Edit goal"));
    }

    /// A goal's container is fixed at creation, and under the `Tab` filter's
    /// All the screen's own title says only "Savings · All" -- so without this
    /// the border is silent about which container `n` is about to create in,
    /// the way `Picker` and `Worksheet` never are about theirs.
    #[test]
    fn the_new_goal_modal_names_the_container_it_will_create_in() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert!(
            drawn(&mut app).contains("Savings · All"),
            "the fixture must open on All for this to be worth asserting"
        );

        press(&mut app, KeyCode::Char('n'));

        assert!(drawn(&mut app).contains("New goal in Rainy Day"));
    }

    #[test]
    fn q_while_searching_types_into_the_box_rather_than_quitting() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "q");

        assert!(!app.should_quit());
        assert_eq!(app.cash.search(), "q");
        assert!(app.cash.rows().is_empty(), "nothing matches \"q\"");
    }

    /// `r` is reconciliation: the balance a statement says the account holds,
    /// typed in while the rows are being entered so a typo or a missed item
    /// shows up as a delta rather than as a surprise next month.
    #[test]
    fn a_typed_target_reaches_the_ledgers_border() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "$1,200.00");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.cash.target(), Some(Cents(120_000)));
        assert!(drawn(&mut app).contains("Target $1,200.00"));
    }

    /// Under All the border quotes the whole kind's balance, which no
    /// statement names. The key says so rather than opening a form over a
    /// figure it cannot check.
    #[test]
    fn r_under_the_all_filter_reports_that_it_needs_an_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('r'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("account filter"), "{}", app.status);
    }

    /// A target is the account's, not the screen's: the other ledger is a
    /// different set of accounts and reconciles against different statements.
    #[test]
    fn a_target_on_one_ledger_leaves_the_other_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Tab);

        assert_eq!(app.credit.target(), None);
        assert!(!drawn(&mut app).contains("Target"));
    }

    /// The net under the whole application: every screen, drawn in a demo,
    /// with none of the fixture's own figures anywhere in the buffer. A new
    /// screen, or a new `format!` that formats a `Cents` itself instead of
    /// asking `tui::demo`, fails here rather than on a shared terminal.
    ///
    /// The fixture is the one with rows on every list, because an absence
    /// check over an empty table passes for free: `app()` has no funds, no
    /// recurring goals and no recurring transactions, which left three of the
    /// nine screens drawing nothing to catch. Asserting that a mask *appears*
    /// is what holds that shut from here on. Accounts is the one screen
    /// exempt -- it draws a name, a band and a position, and no figure at all.
    #[test]
    fn a_demo_leaves_no_figure_on_any_screen() {
        crate::demo::install(true);
        let mut app = app_with_two_rows_on_every_list();
        for screen in "123456789".chars() {
            press(&mut app, KeyCode::Char(screen));
            let drawn = drawn(&mut app);
            for figure in DEMO_FIXTURE_FIGURES {
                assert!(
                    !drawn.contains(figure),
                    "{figure} survived on screen {screen}:\n{drawn}"
                );
            }
            assert!(
                screen == '9' || drawn.contains("██████"),
                "screen {screen} drew no masked figure, so the check above passed for free:\n{drawn}"
            );
        }
    }

    /// Every figure `app_with_two_rows_on_every_list` puts in the database, as
    /// the screens would print it unmasked. One list rather than one per
    /// sweep, so a row added to that fixture is covered by both.
    const DEMO_FIXTURE_FIGURES: [&str; 10] = [
        "1,000", "1,200", "14.99", "25.99", "15,000", "10,000", "100.00", "128", "30,000", "90,000",
    ];

    /// A write reports what it wrote on the status line, which sits in the
    /// footer of whatever screen is open.
    #[test]
    fn a_demo_blocks_the_amount_a_written_row_reports() {
        crate::demo::install(true);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "76.54");
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Hardware");
        press(&mut app, KeyCode::Enter);

        assert!(!app.status.contains("76.54"), "{}", app.status);
        assert!(app.status.contains("██████"), "{}", app.status);
        assert!(app.status.contains("Hardware"), "{}", app.status);
    }

    /// A confirmation names the row it is about to delete, and what makes a
    /// ledger row recognisable is its amount. The modal is drawn over the
    /// screen, so a figure that survives here survives in the middle of the
    /// terminal.
    #[test]
    fn a_demo_leaves_no_figure_on_a_delete_confirmation() {
        crate::demo::install(true);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));

        let drawn = drawn(&mut app);
        assert!(drawn.contains("Delete"), "no confirmation opened:\n{drawn}");
        // Every ledger row behind the modal is blocked already, so the only
        // thing that can put this figure on screen is the label itself.
        assert!(!drawn.contains("200.00"), "the amount survived:\n{drawn}");
        assert!(drawn.contains("2026-08-12"), "the date must stay:\n{drawn}");
    }

    /// The same net one layer in: every key that opens a form or a worksheet
    /// over a row that carries a figure, with the mask on and none of the
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
    #[test]
    fn a_demo_leaves_no_figure_on_any_form_a_row_opens() {
        crate::demo::install(true);
        for (screen, key) in [
            ('2', 'a'),
            ('2', 'e'),
            ('2', 'r'),
            ('2', 't'),
            ('4', 'e'),
            ('4', 'a'),
            ('4', 'A'),
            ('4', 'n'),
            ('5', 'e'),
            ('5', 'E'),
            ('5', 'a'),
            ('6', 'e'),
            ('6', 'E'),
            ('7', 'a'),
            ('8', 'a'),
            ('9', 'e'),
        ] {
            // Screen 6 draws off the `fund` table, which `planning_app` has no
            // rows in; every other screen here has one on the fixture that
            // carries the bills screen 5 needs.
            let mut app = match screen {
                '6' => app_with_two_rows_on_every_list(),
                _ => planning_app(),
            };
            press(&mut app, KeyCode::Char(screen));
            match (screen, key) {
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
            let figures: &[&str] = match screen {
                '6' => &DEMO_FIXTURE_FIGURES,
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

    /// A status line is on screen as surely as a column is, and this one
    /// quotes the figure that was just typed back at the owner.
    #[test]
    fn a_demo_blocks_the_target_a_reconciliation_reports() {
        crate::demo::install(true);
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert!(!app.status.contains("1,200"), "{}", app.status);
        assert!(app.status.contains("██████"), "{}", app.status);

        // The buffer still holds the real figure: reopening and pressing
        // Enter must not write the mask back.
        press(&mut app, KeyCode::Char('r'));
        let Some(Modal::Reconcile(_, form)) = &app.modal else {
            panic!("r must reopen the form: {:?}", app.status);
        };
        assert_eq!(form.value(), "1,200.00");
    }

    /// Reopening on a target already set shows it, so a figure being
    /// corrected is edited rather than retyped.
    #[test]
    fn r_reopens_on_the_target_already_set() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('r'));

        let Some(Modal::Reconcile(_, form)) = &app.modal else {
            panic!("r must reopen the form: {:?}", app.status);
        };
        assert_eq!(form.value(), "1,200.00");
    }

    /// Emptying the field is how a target goes away: the account is back to
    /// having none, and the border to the balance alone.
    #[test]
    fn an_emptied_target_form_clears_the_target() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));

        press(&mut app, KeyCode::Char('r'));
        for _ in 0.."1,200.00".len() {
            press(&mut app, KeyCode::Backspace);
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), None);
        assert!(!drawn(&mut app).contains("Target"));
    }

    /// `Esc` backs out of the form, which is not the same as clearing the
    /// figure the form opened on.
    #[test]
    fn esc_leaves_a_target_already_set_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "9");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));
    }

    /// A figure that does not parse keeps the form and everything typed into
    /// it, the way every other form in the app refuses one.
    #[test]
    fn a_target_that_does_not_parse_keeps_the_form_open() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "twelve");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "{}", app.status);
        assert_eq!(app.cash.target(), None);
        assert!(drawn(&mut app).contains("twelve"));
    }

    /// The target belongs to the account it was typed on, so the `Tab` cycle
    /// carries it -- and the balance it is compared with moves with the same
    /// key.
    #[test]
    fn stepping_the_account_filter_carries_each_targets_own_delta() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Tab);
        assert!(
            !drawn(&mut app).contains("Target"),
            "Rainy Day has no target"
        );

        press(&mut app, KeyCode::BackTab);
        assert!(drawn(&mut app).contains("Target $1,200.00"));
    }

    #[test]
    fn a_on_a_ledger_with_no_accounts_reports_it_in_the_status_line() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));

        assert!(app.status.contains("no account"), "{}", app.status);
        assert!(app.modal.is_none());
        assert!(
            !app.should_quit(),
            "a failed write must not end the session"
        );
    }

    #[test]
    fn committing_a_form_reloads_the_rows_behind_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Zebra");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let rows = app.cash.rows();
        let written = rows
            .iter()
            .find(|t| t.description == "Zebra")
            .expect("the new row must be on screen without another keystroke");
        assert_eq!(written.cents, Cents(4_250));
        assert_eq!(written.date, today());
        assert_eq!(app.status, "added Zebra 42.50 on 2026-08-15");
    }

    /// The status line is the only confirmation a write gets, and a blank
    /// description would leave `added  42.50 on ...` -- a gap between two
    /// spaces that reads as a figure that failed to render rather than as a
    /// row the owner chose not to name.
    #[test]
    fn a_row_written_with_no_description_is_confirmed_by_its_em_dash() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert_eq!(app.status, "added — 42.50 on 2026-08-15");
        assert!(
            app.cash.rows().iter().any(|t| t.description.is_empty()),
            "the row itself keeps the empty description it was written with"
        );
    }

    /// Deleting is the one irreversible key on the ledger, and the label is
    /// the whole of what the question offers to identify the row by. An
    /// unnamed row still has to be identifiable as the row the cursor is on.
    #[test]
    fn the_delete_confirmation_names_an_unnamed_row_by_its_em_dash() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        press(&mut app, KeyCode::Enter);

        for _ in 0..app.cash.rows().len() {
            if app
                .cash
                .selected()
                .is_some_and(|t| t.description.is_empty())
            {
                break;
            }
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char('d'));

        let Some(Modal::Confirm { label, .. }) = &app.modal else {
            panic!("d must open the confirmation: {:?}", app.status);
        };
        assert_eq!(label, "2026-08-15  —  42.50");
    }

    /// `a` on a ledger filtered to one card must not open on a different one:
    /// it is a plausible misfiled row in exactly the workflow `Tab` exists
    /// for, and there is no undo.
    #[test]
    fn adding_on_an_account_filtered_ledger_preselects_that_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(descriptions(&app.credit), ["Batteries"]);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(
            form(&app).display(TxnField::Account).plain_text(),
            "CC2 — Card Two"
        );
    }

    /// The Savings container filter cycles both ways too, so `BackTab` backs
    /// out of a container `Tab` stepped into.
    #[test]
    fn back_tab_steps_the_savings_container_filter_the_other_way() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Tab);
        let container = app.savings.selected_container();
        assert!(container.is_some());

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.savings.selected_container(), None);

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.savings.selected_container(), container);
    }

    #[test]
    fn adding_on_an_unfiltered_ledger_opens_on_the_first_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('a'));

        assert_eq!(
            form(&app).display(TxnField::Account).plain_text(),
            "CC1 — Card One"
        );
    }

    /// Without this the only way out of the popup is `Esc`, which discarded
    /// the whole form — everything typed with it.
    #[test]
    fn esc_closes_the_popup_first_and_the_form_second() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Whole");
        assert!(app.popup.visible() > 0, "\"Whole Foods\" is a suggestion");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup.visible(), 0);
        assert!(!app.popup.is_open());
        assert_eq!(
            form(&app).description(),
            "Whole",
            "dismissing the popup must keep what was typed"
        );

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    /// A popup whose rows were all clipped away captures no keys, so `Esc`
    /// must reach the modal rather than being swallowed by an invisible list.
    #[test]
    fn esc_closes_the_form_when_the_popup_drew_no_rows() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Whole");
        app.popup.set_visible(0);

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    #[test]
    fn four_opens_the_savings_screen_with_every_goal_on_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        assert_eq!(app.screen, Screen::Savings);
        let names: Vec<&str> = app.savings.rows().iter().map(|r| r.name.as_str()).collect();
        // Undated first: Couch has no date, Vacation 2027 does.
        assert_eq!(names, ["Couch", "Vacation 2027"]);
    }

    /// The reconciliation line is the Savings screen's alone.
    #[test]
    fn the_savings_screen_carries_each_containers_unallocated_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        let excess = app.savings.excess();
        assert_eq!(excess.len(), 1, "only Rainy Day holds goals");
        // Rainy Day holds one 200.00 transfer against 10,250.00 allocated.
        assert_eq!(excess[0].1, Cents(20_000) - Cents(1_025_000));
    }

    #[test]
    fn q_while_searching_the_savings_screen_types_into_the_box() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "q");

        assert!(!app.should_quit());
        assert_eq!(app.savings.search(), "q");
        assert!(app.savings.rows().is_empty());
    }

    /// The written allocation must be on screen without another keystroke,
    /// and the container's unallocated remainder must move with it.
    #[test]
    fn a_on_the_savings_screen_writes_an_allocation_against_the_selected_goal() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Down once: the undated Couch heads the list, and this is the goal
        // whose balance the figures below are about.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.savings.selected().unwrap().name, "Vacation 2027");
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "454");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, Cents(1_045_400));
        assert_eq!(app.savings.excess()[0].1, before - Cents(45_400));
    }

    /// `/N` on the amount is a fraction of the container's unallocated
    /// remainder -- what the Savings footer reports, and the same arithmetic
    /// the worksheet's `/N` does.
    #[test]
    fn a_share_typed_on_the_savings_form_books_a_fraction_of_the_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        // The fixture's goals hold more than Rainy Day does, so top it up to a
        // remainder with cents in it: the division has to floor them away.
        write(&app.db, savings, day(2026, 8, 15), 1_255_001, "Interest");
        app.reload().unwrap();
        assert_eq!(app.savings.excess()[0].1, Cents(250_001));
        let before = app.savings.rows()[0].current;

        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "/2");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[0].current, before + Cents(125_000));
        assert_eq!(app.savings.excess()[0].1, Cents(125_001));
    }

    /// A divisor is not a figure, so the form resolves it on screen. Without
    /// this the owner commits to find out what they typed.
    #[test]
    fn the_allocation_form_shows_what_a_share_comes_to_before_it_is_committed() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        write(&app.db, savings, day(2026, 8, 15), 1_255_000, "Interest");
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "/12");

        let screen = drawn(&mut app);
        assert!(screen.contains("= 208"), "{screen}");
    }

    /// The pot is behind the modal and the key has no room in the help table
    /// `Topic::Form` shares with the forms that do not offer it, so the form
    /// says both itself.
    #[test]
    fn the_allocation_form_names_the_remainder_a_share_would_divide() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        write(&app.db, savings, day(2026, 8, 15), 1_255_000, "Interest");
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('a'));

        let screen = drawn(&mut app);
        assert!(
            screen.contains("Rainy Day unallocated 2,500.00 · /N takes 1/N"),
            "{screen}"
        );
    }

    /// A bad divisor reports itself where every other unparseable field does,
    /// and the form stays open on what was typed.
    #[test]
    fn a_share_divided_by_zero_reports_itself_and_keeps_the_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "/0");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the form must stay open");
        assert!(app.status.contains("divide by 0"), "{}", app.status);
    }

    #[test]
    fn a_on_the_savings_screen_with_nothing_selected_says_so() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "zzz");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('a'));

        assert!(app.modal.is_none());
        assert_eq!(app.status, "nothing selected");
    }

    /// `%` and `$/Pay` are derived from the target, so an edit has to move
    /// them without another keystroke.
    #[test]
    fn e_on_the_savings_screen_rewrites_the_goal_and_its_derived_columns() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Down once: Vacation 2027 is the funded goal, so a rewritten target
        // moves a percentage rather than leaving it where it was.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab);
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "20000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let row = &app.savings.rows()[1];
        assert_eq!(row.goal, Cents(2_000_000));
        assert_eq!(row.percent, Some(Percent(50)));
    }

    /// Abandoning returns the value to unallocated, so the goal leaves the
    /// list and the container's remainder rises by exactly its balance.
    #[test]
    fn c_then_enter_abandons_the_selected_goal() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let names: Vec<&str> = app.savings.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Couch"]);
        assert_eq!(app.savings.excess()[0].1, before + Cents(1_000_000));
    }

    /// A close-out into another goal moves value inside one container, so the
    /// reconciliation must not move.
    #[test]
    fn c_into_another_goal_moves_the_balance_and_leaves_the_remainder_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        let rows = app.savings.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Couch");
        assert_eq!(rows[0].current, Cents(1_025_000));
        assert_eq!(app.savings.excess()[0].1, before);
    }

    fn worksheet(app: &App) -> &Worksheet {
        match &app.modal {
            Some(Modal::Worksheet(sheet)) => sheet,
            _ => panic!("no worksheet is open"),
        }
    }

    /// The payday worksheet opens on the Tab container with `per_paycheck`
    /// down every line, so it starts at zero remaining.
    #[test]
    fn capital_a_opens_a_payday_worksheet_prefilled_from_per_paycheck() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));

        let sheet = worksheet(&app);
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        // The worksheet lists the container in screen order, so the undated
        // Couch leads it.
        assert_eq!(names, ["Couch", "Vacation 2027"]);
        // Couch is undated and asks nothing; Vacation 2027 needs 4,986.00
        // over 10 paychecks.
        assert_eq!(sheet.lines()[0].amount, Cents::ZERO);
        assert_eq!(sheet.lines()[1].amount, Cents(50_000));
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// One batch per commit, so a fumbled payday is one `U` rather than dozens
    /// of deletions -- and the goals move without another keystroke.
    #[test]
    fn committing_a_worksheet_writes_one_batch_and_reloads_the_screen() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Row 1 is Vacation 2027, the only goal the prefill asks anything of.
        let before = app.savings.rows()[1].current;
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, before + Cents(50_000));
        let batch = goal::most_recent_batch(&app.db).unwrap().unwrap();
        assert_eq!(batch.kind, goal::BatchKind::Paycheck);
        assert_eq!(goal::batch_shares(&app.db, batch.id).unwrap().len(), 1);
    }

    /// `/` then a digit is the fraction operator; `/` then anything else is
    /// the name filter. Both live under one key, so this is the test that
    /// keeps them apart.
    #[test]
    fn a_slash_then_a_digit_divides_and_a_slash_then_a_letter_filters() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        // Tab twice: Amount -> Date -> Lines.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('*'));
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(worksheet(&app).lines()[0].amount, Cents(25_000));

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('C'));
        assert!(worksheet(&app).is_searching());
        assert_eq!(worksheet(&app).search(), "C");
        // "Vacation 2027" also matches "C" (Vacation), so disambiguate before
        // asserting the filtered list -- the filter is a substring match, and
        // this test exists to prove `/`+digit divides while `/`+letter
        // filters, not to pin the filter's matching rule.
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('u'));
        let names: Vec<&str> = worksheet(&app)
            .lines()
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(names, ["Couch"]);
    }

    /// The operator keys are line-editing keys. With the date focused they
    /// have to type instead, or `s` would spread while you are fixing a date.
    #[test]
    fn the_operator_keys_type_into_the_date_field_rather_than_acting_on_lines() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);
        let before: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();

        press(&mut app, KeyCode::Char('s'));

        let after: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();
        assert_eq!(before, after, "s must not spread while the date is focused");
        assert!(worksheet(&app).date_text().ends_with('s'));
    }

    /// The worksheet's date is a text field with a key handler of its own,
    /// and the editing keys have to reach it there too.
    #[test]
    fn the_worksheets_date_answers_the_editing_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "2026-09-01");

        assert_eq!(worksheet(&app).date_text(), "2026-09-01");
    }

    /// `Ctrl` means editing text everywhere in the app, so it must not reach
    /// an operator: a hand reaching for "delete the last word" would
    /// otherwise spread the whole pot.
    #[test]
    fn a_ctrl_key_does_not_reach_the_worksheets_operators() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        let before: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();

        ctrl_press(&mut app, 'z');

        let after: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();
        assert_eq!(before, after, "Ctrl+Z zeroed the untargeted lines");
    }

    /// Over-allocating would hand out money the container does not hold, and
    /// the failure has to reach the status line rather than the panic hook.
    #[test]
    fn committing_an_over_allocated_worksheet_reports_it_and_keeps_the_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the worksheet must stay open");
        assert!(app.status.contains("over-allocated"), "{}", app.status);
    }

    /// The amount prefills from the container's unallocated remainder: the
    /// interest row is entered on the Cash screen first, so at that point the
    /// excess *is* the interest.
    #[test]
    fn i_opens_a_worksheet_on_the_containers_unallocated_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Give Rainy Day an unallocated balance to post: the fixture's goals hold
        // more than the account does, so top it up first.
        write(
            &app.db,
            app.savings.excess()[0].0,
            day(2026, 8, 15),
            1_100_000,
            "Interest",
        );
        app.reload().unwrap();
        let excess = app.savings.excess()[0].1;
        assert!(excess > Cents::ZERO);

        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        assert_eq!(sheet.kind(), goal::BatchKind::Interest);
        assert_eq!(sheet.amount(), excess);
        // Rainy Day is `manual` with no previous posting, so it falls back to
        // pro-rata across the eligible goals' balances.
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// A Brokerage-shaped container: `pro_rata`, three buckets, and the
    /// down-payment one excluded from the split the way `Planning!J7`
    /// excludes it. Its goals hold 1,945.00 less than the account does --
    /// the interest row, already entered on the Cash screen.
    ///
    /// Returned in screen order: down payment, emergency, mom and dad.
    fn pro_rata_container() -> (App, GoalId, GoalId, GoalId) {
        let db = db::open_in_memory().unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 0).unwrap();
        account::set_interest_policy(&db, brokerage, account::InterestPolicy::ProRata).unwrap();
        let add_goal = |name: &str, target: i64, eligible: bool, balance: i64| {
            let id = goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: brokerage,
                    base_cents: Cents(target),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: eligible,
                    sort: 0,
                    taxed: false,
                },
            )
            .unwrap();
            goal::insert_allocation(&db, id, day(2026, 1, 1), Cents(balance), None, None).unwrap();
            id
        };
        let down_payment = add_goal("Home Down Payment", 50_000_000, false, 50_000_000);
        let emergency = add_goal("Emergency Savings", 10_000_000, true, 10_600_195);
        let mom_and_dad = add_goal("Mom & Dad", 2_500_000, true, 2_500_000);
        write(&db, brokerage, day(2026, 7, 31), 63_300_195, "Balance");
        (
            App::new(db, today()).unwrap(),
            down_payment,
            emergency,
            mom_and_dad,
        )
    }

    /// `Planning!J7` forces the down-payment bucket's interest weight to zero,
    /// which the importer records as `interest_eligible = 0`. A prefill that
    /// ignored the flag would misallocate every future posting.
    #[test]
    fn a_pro_rata_prefill_skips_goals_that_are_not_interest_eligible() {
        let (mut app, down_payment, emergency, mom_and_dad) = pro_rata_container();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        let share = |id| {
            sheet
                .lines()
                .iter()
                .find(|l| l.goal_id == id)
                .expect("every open goal is a line")
                .amount
        };
        assert_eq!(share(down_payment), Cents::ZERO, "excluded from the split");
        assert_eq!(share(emergency), Cents::from_dollars(1_618));
        assert_eq!(share(mom_and_dad), Cents::from_dollars(382));
    }

    /// Eligibility is the owner's call once the import has had its say: a
    /// bucket the sheet excluded can be brought into the split without
    /// touching the importer or the database by hand.
    #[test]
    fn making_a_goal_interest_eligible_brings_it_into_the_split() {
        let (mut app, down_payment, ..) = pro_rata_container();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Goal(form)) = &app.modal else {
            panic!("e opens the goal form");
        };
        assert_eq!(
            form.display(goal_form::GoalField::Name).plain_text(),
            "Home Down Payment"
        );
        // Walked to rather than counted to: the form grows fields, and a
        // count would send the `Right` below into whichever one it grew.
        while !matches!(&app.modal, Some(Modal::Goal(form)) if form.focus == goal_form::GoalField::Interest)
        {
            press(&mut app, KeyCode::Tab);
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        let share = sheet
            .lines()
            .iter()
            .find(|l| l.goal_id == down_payment)
            .expect("every open goal is a line")
            .amount;
        assert!(share > Cents::ZERO, "now weighted, got {share}");
    }

    /// Every worksheet opens on the amount, which takes digits and drops
    /// everything else -- so the line operators have to be live there, or a
    /// tick does nothing until the reader has found `Tab` twice.
    #[test]
    fn space_ticks_a_line_on_a_worksheet_that_has_just_opened() {
        let (mut app, ..) = pro_rata_container();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(worksheet(&app).focus(), worksheet::Focus::Amount);

        press(&mut app, KeyCode::Char(' '));

        assert_eq!(worksheet(&app).selected_count(), 1);
    }

    /// The flow the two keys exist for: tick who the posting funds, `z` to
    /// free the pot, `w` to divide it in the prefill's proportions. One
    /// ticked goal takes the lot.
    #[test]
    fn ticking_one_goal_then_z_and_w_posts_the_whole_excess_to_it() {
        let (mut app, down_payment, emergency, mom_and_dad) = pro_rata_container();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));
        let pot = worksheet(&app).amount();

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('w'));

        let share = |id| {
            worksheet(&app)
                .lines()
                .iter()
                .find(|l| l.goal_id == id)
                .expect("every open goal is a line")
                .amount
        };
        assert_eq!(share(emergency), pot);
        assert_eq!(share(down_payment), Cents::ZERO);
        assert_eq!(share(mom_and_dad), Cents::ZERO);
        assert_eq!(worksheet(&app).remaining(), Cents::ZERO);
    }

    /// Committing an interest posting returns the container to reconciled,
    /// which is the whole shape of the operation.
    #[test]
    fn committing_an_interest_posting_returns_the_container_to_zero_excess() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        write(
            &app.db,
            app.savings.excess()[0].0,
            day(2026, 8, 15),
            1_100_000,
            "Interest",
        );
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.excess()[0].1, Cents::ZERO);
    }

    /// `s` creates every selected entry at once, dating each one and recording
    /// the entry it came from -- so the picker's "Open?" column stays true.
    #[test]
    fn s_creates_a_goal_for_every_selected_catalog_entry() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let created = app
            .savings
            .rows()
            .into_iter()
            .find(|r| r.name == "Dropbox")
            .expect("the new goal must be on screen without another keystroke");
        assert_eq!(created.goal, Cents::from_dollars(128));
        assert_eq!(
            created.goal_date,
            Some(day(2027, 9, 1)),
            "a reseed is for the year ahead"
        );
        assert_eq!(created.current, Cents::ZERO);
    }

    /// The wiring rather than the rule, which `picker`'s own tests pin:
    /// `commit_picker` has to ask about *this calendar year*, so an entry that
    /// has already had its round skips the year between rather than repeating
    /// it.
    #[test]
    fn s_dates_a_biennial_entry_that_has_had_this_years_round_two_years_out() {
        let mut app = app();
        let id = recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: recurring_goal::Cadence::Biennial,
            },
        )
        .unwrap();
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Backblaze".to_string(),
                container_account_id: app.savings.default_container().unwrap(),
                base_cents: Cents::from_dollars(99),
                goal_date: Some(day(2026, 11, 1)),
                recurring_goal_id: Some(id),
                interest_eligible: true,
                sort: 9,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        // It has an open goal, so it opens unticked -- Space is the deliberate
        // second round the picker never refuses.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);

        let dates: Vec<Option<NaiveDate>> = app
            .savings
            .rows()
            .into_iter()
            .filter(|r| r.name == "Backblaze")
            .map(|r| r.goal_date)
            .collect();
        assert_eq!(dates, [Some(day(2026, 11, 1)), Some(day(2028, 11, 1))]);
    }

    /// `Tax()` applies to the entries flagged for it and to no others -- the
    /// workbook's own `Savings!Q` column is a mix.
    ///
    /// The picker hands the flag across rather than spending it, so the screen
    /// shows the derived target while the table holds the base.
    #[test]
    fn only_a_taxed_catalog_entry_goes_through_the_tax_lambda() {
        let mut app = app();
        setting::set(&app.db, key::TAX_RATE, crate::rate::BasisPoints(625)).unwrap();
        for (name, taxed) in [("Rolex", true), ("Dropbox", false)] {
            recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month: 9,
                    base_cents: Cents::from_dollars(9_000),
                    taxed,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
        }
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        let target = |name: &str| {
            app.savings
                .rows()
                .into_iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no goal named {name}"))
                .goal
        };
        // 9,000 × 1.0625 = 9,562.50, rounded up to the next $5.
        assert_eq!(target("Rolex"), Cents::from_dollars(9_565));
        assert_eq!(target("Dropbox"), Cents::from_dollars(9_000));

        // ...and what the *table* holds is the base and the flag, handed
        // across rather than spent: the lambda runs on read, so a goal made
        // from a taxed entry is indistinguishable from one the owner marked
        // taxed by hand, and nothing can tax the taxed figure a second time.
        let stored = |name: &str| {
            let g = goal::all_with_balances(&app.db)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no goal named {name}"));
            (g.goal.base_cents, g.goal.taxed)
        };
        assert_eq!(stored("Rolex"), (Cents::from_dollars(9_000), true));
        assert_eq!(stored("Dropbox"), (Cents::from_dollars(9_000), false));
    }

    /// The guards above are what keep a taxed goal with no rate out of the
    /// database, but a database that already holds one -- hand-edited, or
    /// migrated from a build that had no flag -- still has to open. A strict
    /// read in `reload_savings` runs inside `App::new`, so the refusal would
    /// not blank a screen, it would stop the application starting, and the
    /// rate is set from inside the application.
    #[test]
    fn the_app_starts_on_a_taxed_goal_with_no_rate_and_draws_it_against_its_base() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        goal::insert(
            &db,
            &goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents(100_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();

        let app = App::new(db, today()).unwrap();
        let row = app
            .savings
            .rows()
            .into_iter()
            .find(|r| r.name == "Couch")
            .expect("the goal is drawn rather than dropped");
        assert_eq!(row.goal, Cents(100_000), "the base, for want of a rate");
    }

    /// The picker is the second place a taxed goal is written, alongside the
    /// goal form's own commit, and it hands the flag across rather than
    /// computing anything -- so on a database with no rate on record it has
    /// to refuse before `insert_all` ever runs. Without the guard this writes
    /// `taxed = 1` with no rate anywhere, which is the row the read side
    /// calls corrupt -- every screen then draws it against its base, and
    /// every path that would spend the figure refuses.
    #[test]
    fn s_refuses_a_taxed_entry_on_a_database_with_no_tax_rate_and_writes_no_goal() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Rolex".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(9_000),
                taxed: true,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the picker stays open on a refusal");
        assert_eq!(app.status, goal_engine::NO_TAX_RATE);
        assert!(
            goal::all_with_balances(&app.db)
                .unwrap()
                .iter()
                .all(|g| g.goal.name != "Rolex"),
            "no goal is written"
        );
    }

    /// "Open?" is a hint, never a block: goal names are not unique, and a
    /// second open goal against one entry is a legitimate thing to want. It is
    /// the one the preselection will not tick, so the second round takes an
    /// explicit `Space` where the first took none.
    #[test]
    fn a_second_goal_against_an_entry_that_already_has_one_is_allowed() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Lego".to_string(),
                month: 12,
                base_cents: Cents::from_dollars(340),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);

        let legos = app
            .savings
            .rows()
            .into_iter()
            .filter(|r| r.name == "Lego")
            .count();
        assert_eq!(legos, 2);
    }

    /// One batch per worksheet commit, so a fumbled payday is one `U` rather
    /// than dozens of deletions.
    #[test]
    fn capital_u_then_y_undoes_the_last_batch() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Row 1 is Vacation 2027, the only goal the prefill moves.
        let before = app.savings.rows()[1].current;
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        assert_ne!(app.savings.rows()[1].current, before);

        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, before);
        assert!(goal::most_recent_batch(&app.db).unwrap().is_none());
    }

    /// The confirmation exists because a batch is many rows: anything that is
    /// not `y` has to be a cancel rather than a fall-through.
    #[test]
    fn capital_u_then_any_other_key_cancels_and_the_batch_survives() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        let after_payday = app.savings.rows()[0].current;

        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('n'));

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[0].current, after_payday);
        assert!(goal::most_recent_batch(&app.db).unwrap().is_some());
    }

    /// The import batch holds every opening balance in the database. Undoing
    /// it would empty every goal in one keystroke, so `U` must not see it.
    #[test]
    fn capital_u_never_offers_the_import_batch() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Vacation 2027".to_string(),
                container_account_id: savings,
                base_cents: Cents(1_500_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocations(
            &db,
            goal::BatchKind::Import,
            day(2026, 8, 12),
            &[(id, Cents(1_000_000))],
            Some("imported balance"),
        )
        .unwrap();
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing to undo"), "{}", app.status);
        assert_eq!(app.savings.rows()[0].current, Cents(1_000_000));
    }

    /// Two housing bills and one other, so both waterfall lines have
    /// something in them and a transposition is visible.
    fn with_bills(db: &Db) {
        use crate::db::bill::{self, Category, NewBill};
        let bill = |label: &str, dollars: i64, category, sort| NewBill {
            label: label.to_string(),
            cents: Cents::from_dollars(dollars),
            category,
            sort,
        };
        bill::insert(db, &bill("Mortgage", 1_200, Category::Housing, 0)).unwrap();
        bill::insert(db, &bill("HOA", 300, Category::Housing, 1)).unwrap();
        bill::insert(db, &bill("Coworking", 1_000, Category::Other, 0)).unwrap();
    }

    /// The Rainy Day and Brokerage containers, the six lines with a goal-backed
    /// destination, and the settings that point each at its goal -- the same
    /// shape `transfer::tests::configured` builds, so the destination block
    /// resolves rather than falling back to withdrawals.
    ///
    /// The two spare unclaimed goals that divide the Goals plug live in
    /// Brokerage rather than Rainy Day, unlike `transfer::tests::configured`:
    /// `nulling_a_lines_destination_key_moves_it_out_of_its_group_on_screen`
    /// clears Future Housing's key, which un-claims a Brokerage goal. If the
    /// plug's goals sat in Rainy Day, that would leave two containers each holding
    /// an unclaimed goal and `spread_container` would refuse to pick one.
    fn planning_app() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();
        // A few cents of drift off the round paycheck so `Excess (Actual)`
        // has something for `floor_to_dollar` to floor.
        write(&db, checking, day(2026, 8, 1), 5_000_007, "Paycheck");
        with_bills(&db);

        let add_goal = |container: AccountId, name: &str| -> GoalId {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: container,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort: 0,
                    taxed: false,
                },
            )
            .unwrap()
        };
        let bill_payments = add_goal(savings, "Bill Payments");
        let housing = add_goal(savings, "Housing");
        let roth = add_goal(savings, "Roth IRA");
        let down_payment = add_goal(brokerage, "Home Down Payment");
        let mom_and_dad = add_goal(brokerage, "Mom & Dad");
        let emergency = add_goal(brokerage, "Emergency Savings");
        add_goal(brokerage, "Lego");
        add_goal(brokerage, "Dropbox");
        // Fund the two gate-backed goals fully so neither gate fires: this
        // fixture exists to test where money lands, not to exercise the
        // gates.
        for id in [roth, emergency] {
            goal::insert_allocation(
                &db,
                id,
                day(2026, 8, 1),
                Cents::from_dollars(1_000),
                None,
                None,
            )
            .unwrap();
        }

        let key = |line: Line| match line.destination() {
            Destination::Goal(key) => key,
            other => panic!("{line:?} resolves to {other:?}"),
        };
        setting::set(&db, key(Line::Bills), bill_payments).unwrap();
        setting::set(&db, key(Line::CurrentHousing), housing).unwrap();
        setting::set(&db, Gate::Roth.key(), roth).unwrap();
        setting::set(&db, key(Line::FutureHousing), down_payment).unwrap();
        setting::set(&db, key(Line::MomAndDad), mom_and_dad).unwrap();
        setting::set(&db, Gate::EmergencyFund.key(), emergency).unwrap();

        App::new(db, today()).unwrap()
    }

    fn planning_row<'a>(app: &'a App, label: &str) -> &'a crate::tui::planning::Row {
        app.planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == label)
            .unwrap_or_else(|| panic!("no Planning row labelled {label:?}"))
    }

    /// `planning_app` plus one Everyday row three days after today, so the three
    /// days between the derived Paycheck-Eve date and the scrubbed one hold a
    /// balance change big enough to move the whole waterfall.
    fn planning_app_with_a_row_after_today() -> App {
        let mut app = planning_app();
        let checking = account::by_code(&app.db, "CHK", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        write(&app.db, checking, day(2026, 8, 18), -1_000_000, "Rent");
        app.reload().unwrap();
        app
    }

    /// Three `Right`s off today (2026-08-15) puts Paycheck-Eve on the 18th,
    /// the day the fixture's Rent lands.
    fn scrub_past_the_rent(app: &mut App) {
        for _ in 0..3 {
            press(app, KeyCode::Right);
        }
        assert_eq!(app.adhoc, day(2026, 8, 18));
    }

    /// The scrub asks "what is the balance if the paycheck lands on a
    /// different day", and `Excess (Actual)` is that balance less Target and
    /// Buffer. Quoting it at the derived date while the Overview quotes the
    /// scrubbed one is two screens disagreeing about which day they mean.
    #[test]
    fn scrubbing_moves_the_date_planning_quotes_the_checking_balance_at() {
        let mut app = planning_app_with_a_row_after_today();
        let before = app.planning.excess_actual();

        scrub_past_the_rent(&mut app);

        assert_eq!(app.planning.excess_actual(), before - Cents(1_000_000));
    }

    /// `t` recomputes the plan rather than reading the one behind the screen,
    /// so it is its own chance to quote the wrong date -- and the one that
    /// would move real money to a figure the owner never saw.
    #[test]
    fn t_builds_its_transfers_from_the_scrubbed_plan() {
        let mut app = planning_app_with_a_row_after_today();
        scrub_past_the_rent(&mut app);
        let expected = plan::compute_from_db(&app.db, app.adhoc)
            .unwrap()
            .lines
            .future_housing;

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        assert_eq!(modal_transfer_amount(&app, Line::FutureHousing), expected);
    }

    /// `t`'s confirmation is the one date field that opens neither on today
    /// nor blank: its rows are dated for when the transfers *land*, two
    /// business days out. The fixture's today is a Saturday, so this pins the
    /// weekend skip as well as the offset -- and pins the one opening date
    /// nothing else in the suite held down, which is how the invariant listing
    /// them came to be short by one.
    #[test]
    fn the_transfer_confirmation_opens_two_business_days_out() {
        let mut app = planning_app();
        assert_eq!(app.today.weekday(), chrono::Weekday::Sat);

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        assert_eq!(confirm.date_value(), "2026-08-18");
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 18));
    }

    /// The confirmation's date is a text field like any other, and it has a
    /// key handler of its own -- the one place the editing keys could be
    /// missing while every form had them.
    #[test]
    fn the_transfer_confirmation_answers_the_editing_keys() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "2026-09-01");

        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        assert_eq!(confirm.commit().unwrap(), day(2026, 9, 1));
    }

    /// What `t`'s confirmation modal says it will move for one line.
    fn modal_transfer_amount(app: &App, wanted: Line) -> Cents {
        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        confirm
            .rows()
            .iter()
            .find_map(|row| match row {
                transfer::Row::Transfer { lines, .. } => lines
                    .iter()
                    .find(|(line, _)| *line == wanted)
                    .map(|(_, cents)| *cents),
                transfer::Row::Withdrawal { line, cents } if *line == wanted => Some(*cents),
                transfer::Row::Withdrawal { .. } => None,
            })
            .unwrap_or_else(|| panic!("no transfer row carries {wanted:?}"))
    }

    /// Pinning freezes the figure the screen shows, which is now the scrubbed
    /// one: a pin that quoted the derived date would disagree with the row it
    /// was pressed on.
    #[test]
    fn p_pins_the_scrubbed_excess_the_screen_is_showing() {
        let mut app = planning_app_with_a_row_after_today();
        scrub_past_the_rent(&mut app);
        // Computed here rather than read off the screen: reading the screen
        // would agree with itself whichever date it had used.
        let expected = plan::compute_from_db(&app.db, app.adhoc)
            .unwrap()
            .excess_actual
            .floor_to_dollar();

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));

        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn five_opens_the_planning_screen_with_the_waterfall_on_it() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        assert_eq!(app.screen, Screen::Planning);
        assert_eq!(
            planning_row(&app, "Mortgage + HOA").value,
            Cents::from_dollars(1_500).to_whole_dollars()
        );
        assert!(app.planning.selected().unwrap().editable.is_some());
    }

    /// The write has to land in the database *and* be back on screen without
    /// another keystroke, which is what the reload after a commit is for.
    #[test]
    fn e_on_the_planning_screen_rewrites_the_constant_and_the_waterfall_moves() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.planning.selected_target(), Some(Target::Target));
        let before = planning_row(&app, "Excess (Actual)").value.clone();

        press(&mut app, KeyCode::Char('e'));
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "1000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, key::PLANNING_TARGET).unwrap(),
            Some(Cents::from_dollars(1_000))
        );
        assert_eq!(planning_row(&app, "Target").value, "1,000");
        assert_ne!(
            planning_row(&app, "Excess (Actual)").value,
            before,
            "a lower target frees more excess"
        );
        assert_eq!(
            app.planning.selected_target(),
            Some(Target::Target),
            "the cursor stays where the edit was"
        );
    }

    /// The waterfall's own hand-typed figure: `e` on `Excess (Used)` pins
    /// what was typed, exactly as `p` pins what was computed, and every line
    /// below runs off it.
    #[test]
    fn e_on_excess_used_pins_the_figure_that_was_typed() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        for _ in 0..40 {
            if app.planning.selected_target() == Some(Target::PinnedExcess) {
                break;
            }
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.planning.selected_target(), Some(Target::PinnedExcess));
        assert!(!app.planning.is_pinned());

        press(&mut app, KeyCode::Char('e'));
        for _ in 0..16 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(Cents::from_dollars(1_200))
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(app.today)
        );
        assert!(app.planning.is_pinned());
        assert_eq!(planning_row(&app, "Excess (Used)").value, "1,200");
    }

    /// A failed parse must keep the form open with what was typed, not
    /// discard the edit and leave the user guessing.
    #[test]
    fn a_rejected_edit_reports_it_and_keeps_the_form_open() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('e'));
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "lots");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the form must survive a bad parse");
        assert!(app.status.contains("lots"), "{}", app.status);
        assert_eq!(setting::get(&app.db, key::PLANNING_TARGET).unwrap(), None);
    }

    #[test]
    fn a_adds_a_bill_and_the_biweekly_column_picks_it_up() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Newspaper");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "30");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let added = planning_row(&app, "Newspaper");
        assert_eq!(added.value, "30");
        // 30 * 12 / 26 = 13.85, rounded up to a whole dollar.
        assert_eq!(added.extra, Cents::from_dollars(14).to_whole_dollars());
        assert_eq!(
            crate::db::bill::amounts(&app.db, crate::db::bill::Category::Other)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn d_then_y_deletes_the_selected_bill() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        while !matches!(app.planning.selected_target(), Some(Target::Bill(_))) {
            press(&mut app, KeyCode::Down);
        }

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none());
        assert_eq!(
            crate::db::bill::amounts(&app.db, crate::db::bill::Category::Housing)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(planning_row(&app, "Mortgage + HOA").value, "300");
    }

    /// Moves the Planning cursor onto the first bill row and answers which one
    /// it landed on.
    fn select_first_bill(app: &mut App) -> crate::db::bill::Bill {
        while !matches!(app.planning.selected_target(), Some(Target::Bill(_))) {
            press(app, KeyCode::Down);
        }
        let Some(Target::Bill(id)) = app.planning.selected_target() else {
            unreachable!()
        };
        crate::db::bill::get(&app.db, id).unwrap()
    }

    #[test]
    fn capital_e_opens_the_selected_bill_prefilled() {
        use crate::tui::planning::BillField;
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);

        press(&mut app, KeyCode::Char('E'));

        match &app.modal {
            Some(Modal::Bill(form)) => {
                assert_eq!(form.editing, Some(bill.id));
                assert_eq!(form.display(BillField::Label).plain_text(), bill.label);
                assert_eq!(
                    form.display(BillField::Amount).plain_text(),
                    bill.cents.to_string()
                );
                assert_eq!(form.category(), bill.category);
            }
            _ => panic!("no bill form is open"),
        }
    }

    /// `E` prefills the bill's own figure, so the form is where that figure
    /// would otherwise be published to whoever is watching -- the same rule
    /// every other amount field follows.
    #[test]
    fn a_demo_blocks_the_amount_capital_e_opens_a_bill_on() {
        use crate::tui::planning::BillField;
        crate::demo::install(true);
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);

        press(&mut app, KeyCode::Char('E'));

        let Some(Modal::Bill(form)) = &app.modal else {
            panic!("no bill form is open");
        };
        assert_eq!(form.display(BillField::Amount).plain_text(), "██████");
        assert_eq!(form.display(BillField::Label).plain_text(), bill.label);
        // The buffer is untouched, so Enter still rewrites the real figure.
        assert_eq!(form.commit().unwrap().cents, bill.cents);
    }

    /// `E` is the whole row, so committing it must rewrite the bill it opened
    /// on -- an insert would leave the old row behind and double the category.
    #[test]
    fn committing_capital_e_rewrites_the_bill_rather_than_adding_one() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);
        let before = crate::db::bill::list(&app.db, bill.category).unwrap().len();

        press(&mut app, KeyCode::Char('E'));
        type_str(&mut app, " + PMI");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let after = crate::db::bill::list(&app.db, bill.category).unwrap();
        assert_eq!(after.len(), before);
        assert_eq!(
            after.iter().find(|b| b.id == bill.id).unwrap().label,
            format!("{} + PMI", bill.label)
        );
    }

    /// Every other row is a constant, which `e` already edits in place.
    #[test]
    fn capital_e_on_a_row_that_is_not_a_bill_is_refused() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.planning.selected_target(), Some(Target::Target));

        press(&mut app, KeyCode::Char('E'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("bill"), "{}", app.status);
    }

    /// `d` on a constant would otherwise have to mean "delete a setting",
    /// which is what `p` does for the one setting that can be absent.
    #[test]
    fn d_on_a_row_that_is_not_a_bill_is_refused() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('d'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("bill"), "{}", app.status);
    }

    /// `App::new` must survive a database the waterfall cannot run against --
    /// every other screen still works there.
    #[test]
    fn the_planning_screen_says_so_when_there_is_no_checking_account() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('5'));
        assert!(app.planning.rows().is_empty());
        assert!(
            app.planning.message().unwrap().contains("Checking band"),
            "{:?}",
            app.planning.message()
        );
    }

    /// Pinning freezes the excess the whole waterfall is divided from, so it
    /// takes the whole-dollar floor -- the same figure `compute` uses when
    /// nothing is pinned.
    #[test]
    fn p_pins_the_live_excess_at_its_whole_dollar_floor() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let live = app.planning.excess_actual();
        assert_ne!(live, live.floor_to_dollar(), "the fixture must have cents");

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(live.floor_to_dollar())
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(today())
        );
        assert!(app.planning.is_pinned());
        assert!(app.planning.pin_line().unwrap().contains("pinned"));
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

    /// Unpinning has to remove both keys -- a pin with a date and no amount
    /// would render a line about a plan that is not pinned.
    #[test]
    fn capital_p_unpins_and_clears_the_date_with_it() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('P'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&app.db, key::PINNED_AT).unwrap(), None);
        assert!(!app.planning.is_pinned());
        assert_eq!(app.planning.pin_line(), None);
    }

    /// The whole of the change: a second `p` re-pins rather than clearing.
    /// The press that follows a forgotten pin is the next payday's, and a `p`
    /// that answered it with "unpinned" would make the press that matters the
    /// second one every time.
    #[test]
    fn p_on_an_already_pinned_plan_re_pins_rather_than_clearing() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let first = setting::get(&app.db, key::PINNED_EXCESS).unwrap();
        assert!(first.is_some());

        press(&mut app, KeyCode::Char('p'));

        assert!(app.planning.is_pinned(), "the second p cleared the pin");
        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), first);
        assert_eq!(app.status, format!("re-pinned {}", first.unwrap()));
    }

    /// A re-pin takes the figure the screen is showing *now*, and moves the
    /// date with it -- so the drift falls back to the cents the whole-dollar
    /// floor drops, rather than going on reporting a gap against a figure
    /// that has just been replaced.
    #[test]
    fn a_re_pin_takes_the_current_excess_and_resets_the_drift() {
        let mut app = planning_app_with_a_row_after_today();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let first = setting::get(&app.db, key::PINNED_EXCESS)
            .unwrap()
            .expect("p did not pin");

        // Scrubbing past the Rent moves `Excess (Actual)` off the pin, which
        // is what the drift line reports. The scrub is an Overview key, and
        // it reaches Planning through the `App::adhoc` both screens read.
        press(&mut app, KeyCode::Char('1'));
        scrub_past_the_rent(&mut app);
        press(&mut app, KeyCode::Char('5'));
        assert!(app.planning.pin_line().unwrap().contains("moved"));

        press(&mut app, KeyCode::Char('p'));

        let second = setting::get(&app.db, key::PINNED_EXCESS)
            .unwrap()
            .expect("the re-pin cleared it");
        assert_ne!(second, first, "the re-pin kept the stale figure");
        assert_eq!(second, app.planning.excess_actual().floor_to_dollar());
        // Under a dollar, which is the floor's own remainder -- a fresh pin
        // never reads as zero drift, and it was a whole Rent out a moment ago.
        assert!(
            app.planning.excess_actual() - second < Cents::from_dollars(1),
            "{:?}",
            app.planning.pin_line()
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(app.today),
            "the re-pin left the old date behind"
        );
    }

    /// `P` on a plan nobody pinned says so rather than writing anything.
    #[test]
    fn capital_p_with_nothing_pinned_says_so() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('P'));

        assert_eq!(app.status, "nothing pinned");
        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
    }

    /// The pin is what `Excess (Used)` divides from, so pinning must be
    /// visible in the waterfall itself and not only in the footer.
    #[test]
    fn a_pinned_plan_uses_the_pinned_figure_rather_than_the_live_one() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let pinned = planning_row(&app, "Excess (Used)").value.clone();

        // Move the live excess by lowering the target, and confirm the used
        // figure does not follow it.
        setting::set(&app.db, key::PLANNING_TARGET, Cents::from_dollars(1)).unwrap();
        app.reload().unwrap();

        assert_eq!(planning_row(&app, "Excess (Used)").value, pinned);
        assert!(
            app.planning
                .pin_line()
                .unwrap()
                .contains("excess has since moved"),
            "{:?}",
            app.planning.pin_line()
        );
    }

    /// Unpinning clears two keys and reads nothing off the view, and the
    /// footer offers `P unpin` on the strength of `is_pinned`, which survives
    /// `set_unavailable`. Refusing here would advertise a key that then does
    /// nothing, with the pin stuck until the plan computes again.
    #[test]
    fn a_pinned_plan_can_be_unpinned_after_it_stops_computing() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        assert!(app.planning.is_pinned());

        // A gate pointing at a goal that does not exist is a corrupt
        // database, which is exactly the error the screen reports rather than
        // computing a plan.
        setting::set(&app.db, crate::gate::Gate::Roth.key(), GoalId(999)).unwrap();
        app.reload().unwrap();
        assert!(app.planning.message().is_some());

        press(&mut app, KeyCode::Char('P'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&app.db, key::PINNED_AT).unwrap(), None);
        assert_eq!(app.status, "unpinned");
    }

    /// `set_unavailable` leaves `excess_actual` holding whatever the last
    /// successful view left there, so `p` on a screen showing "no Everyday cash
    /// account" must not pin a number belonging to a plan the screen has just
    /// said it cannot compute.
    #[test]
    fn p_on_a_screen_with_no_live_view_is_refused() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert!(!app.planning.is_pinned());
        assert_eq!(app.status, "nothing to pin");
    }

    /// The destination block is the resolution, not three hardcoded names:
    /// nulling a line's key must move its money on the screen with no code
    /// change. This is the test the whole indirection exists for.
    ///
    /// "Future Housing" also labels the Split section's percentage row, and
    /// -- while the key is still set -- the destination block's own row for
    /// a line that resolved into a container. Both render on every load, so
    /// merely finding the label anywhere on screen, or even counting its
    /// occurrences, proves nothing: the count is identical whether the line
    /// landed in Brokerage or fell out as a withdrawal. What changes is
    /// which row comes immediately before it: a withdrawal is always the
    /// row directly under its own `Withdrawal` heading -- one line's
    /// `(Line, Cents)` pair per pair of rows, from `transfer::Row::Withdrawal`.
    #[test]
    fn nulling_a_lines_destination_key_moves_it_out_of_its_group_on_screen() {
        let mut app = planning_app();
        let before = planning_row(&app, "Brokerage").value.clone();
        let future_housing_is_a_withdrawal = |app: &App| {
            app.planning
                .rows()
                .windows(2)
                .any(|w| w[0].label.trim() == "Withdrawal" && w[1].label.trim() == "Future Housing")
        };
        assert!(
            !future_housing_is_a_withdrawal(&app),
            "Future Housing should still be inside Brokerage, not a withdrawal, before the key is cleared"
        );

        let Destination::Goal(key) = Line::FutureHousing.destination() else {
            panic!("Future Housing is goal-backed");
        };
        setting::clear(&app.db, key).unwrap();
        app.reload().unwrap();

        assert_ne!(planning_row(&app, "Brokerage").value, before);
        assert!(
            future_housing_is_a_withdrawal(&app),
            "the line did not appear as a withdrawal"
        );
    }

    /// `t` writes every row in one go and leaves the ledger holding both
    /// legs of each transfer.
    #[test]
    fn t_on_planning_writes_the_transfers() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        // `planning_app` already seeds one paycheck row, so the assertion
        // below is against the count `t` itself adds, not against zero.
        let before = crate::db::txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        assert!(crate::db::txn::count(&app.db).unwrap() > before);
    }

    /// `planning_app` leaves Lego and Dropbox unclaimed in Brokerage. One
    /// unclaimed goal in Rainy Day as well is all it takes for the plug to have
    /// nowhere single to land -- the owner's own database, in miniature.
    fn ambiguous_app() -> App {
        let mut app = planning_app();
        let savings = account::by_code(&app.db, "SAV", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Sabbatical".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 9,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();
        app
    }

    /// The complaint this answers: the screen says the plan is unresolved in
    /// a cell too narrow to name the goal that caused it.
    #[test]
    fn enter_on_an_unresolved_plan_opens_the_details() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Enter);

        let Some(Modal::Details(title, lines)) = &app.modal else {
            panic!("no details panel opened: {}", app.status);
        };
        assert_eq!(*title, "Transfers unresolved");
        let text = lines.join("\n");
        assert!(text.contains("Rainy Day"), "{text}");
        assert!(text.contains("Brokerage"), "{text}");
        assert!(text.contains("Sabbatical"), "{text}");
    }

    /// The row and the panel are two lengths of one failure, and the panel is
    /// the one with room for the part that says what to do about it.
    #[test]
    fn the_panel_names_the_goal_the_unresolved_row_has_no_room_for() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));
        let row = app
            .planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "unresolved")
            .expect("no unresolved row");
        assert!(!row.value.contains("Sabbatical"), "{}", row.value);

        press(&mut app, KeyCode::Enter);
        let Some(Modal::Details(_, lines)) = &app.modal else {
            panic!("no details panel opened");
        };
        assert!(lines.join("\n").contains("Sabbatical"));
    }

    #[test]
    fn esc_closes_the_details() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        assert!(app.modal.is_none());
    }

    /// A key that opens an empty panel is worse than one that says why it
    /// did nothing.
    #[test]
    fn enter_on_a_resolved_plan_opens_nothing_and_says_so() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "a panel opened over a resolved plan");
        assert!(app.status.contains("nothing to explain"), "{}", app.status);
    }

    /// End to end, through the keyboard: closing the goal a line names must
    /// not turn the next `e`-`Enter` into a silent clearing.
    #[test]
    fn e_then_enter_on_a_line_naming_a_closed_goal_changes_nothing() {
        let mut app = planning_app();
        let key = destination_key(Line::Bills);
        let closed = setting::get(&app.db, key).unwrap().unwrap();
        goal::close(&app.db, closed).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, key).unwrap(),
            Some(closed),
            "a bare Enter cleared a destination nobody questioned"
        );
    }

    /// The six scroll keys are documented nowhere because they work on every
    /// list in the app. A modal whose height never reaches its cursor keeps
    /// `page_height: 1`, so `PageDown` degenerates to `Down` -- an
    /// undocumented key quietly not working, which is the one failure mode
    /// that promise cannot survive.
    #[test]
    fn page_down_moves_a_screenful_in_the_destination_list() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));
        // The viewport height is a render-time measurement, so the list has
        // to be drawn once before a page means anything.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 40)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        press(&mut app, KeyCode::PageDown);

        let Some(Modal::Destination(chooser)) = &app.modal else {
            panic!("the list closed");
        };
        assert!(
            chooser.selected_index() > 1,
            "PageDown moved {} row(s) -- the viewport height never reached the cursor",
            chooser.selected_index()
        );
    }

    /// Walk the Planning cursor down to one line's destination row.
    fn cursor_to(app: &mut App, line: Line) {
        for _ in 0..app.planning.rows().len() {
            if app.planning.selected_editable() == Some(planning::Editable::Destination(line)) {
                return;
            }
            press(app, KeyCode::Down);
        }
        panic!("no destination row for {line:?}");
    }

    fn goal_named(app: &App, name: &str) -> GoalId {
        goal::all_with_balances(&app.db)
            .unwrap()
            .into_iter()
            .find(|g| g.goal.name == name)
            .unwrap_or_else(|| panic!("no goal named {name:?}"))
            .goal
            .id
    }

    fn destination_key(line: Line) -> crate::db::setting::Key<GoalId> {
        match line.destination() {
            Destination::Goal(key) => key,
            other => panic!("{line:?} resolves to {other:?}"),
        }
    }

    /// The whole feature, end to end: a line whose key an older import never
    /// wrote, pointed at the goal it names without leaving the app.
    #[test]
    fn e_then_enter_points_an_unset_line_at_the_goal_its_name_suggests() {
        let mut app = planning_app();
        setting::clear(&app.db, destination_key(Line::MomAndDad)).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::MomAndDad);

        press(&mut app, KeyCode::Char('e'));
        assert!(
            matches!(app.modal, Some(Modal::Destination(_))),
            "no destination list opened: {}",
            app.status
        );

        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, destination_key(Line::MomAndDad)).unwrap(),
            Some(goal_named(&app, "Mom & Dad"))
        );
        assert!(app.modal.is_none(), "the list stayed open");
        assert!(app.status.contains("Mom & Dad"), "{}", app.status);
    }

    /// The reverse, and the reason the withdrawal is a row rather than an
    /// `Esc`: unset is a destination the owner may want to choose.
    #[test]
    fn choosing_the_withdrawal_clears_the_lines_key() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        // The list opens on the goal this line already names, lifted to the
        // top, so the withdrawal is the row directly under it -- one Down,
        // whatever that goal's own place in the list would have been.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, destination_key(Line::Bills)).unwrap(),
            None
        );
    }

    /// The screen behind the list is rebuilt from the setting that was just
    /// written -- otherwise the block still shows the state it was opened to
    /// change.
    #[test]
    fn the_block_shows_the_new_destination_as_soon_as_the_list_closes() {
        let mut app = planning_app();
        setting::clear(&app.db, destination_key(Line::MomAndDad)).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::MomAndDad);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        let row = app
            .planning
            .rows()
            .iter()
            .find(|r| r.editable == Some(planning::Editable::Destination(Line::MomAndDad)))
            .expect("no Mom & Dad destination row");
        assert_eq!(row.value, "Mom & Dad");
        assert_eq!(row.extra, "Brokerage");
    }

    #[test]
    fn esc_leaves_the_destination_alone() {
        let mut app = planning_app();
        let before = setting::get(&app.db, destination_key(Line::Bills)).unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, destination_key(Line::Bills)).unwrap(),
            before
        );
    }

    /// `/` opens a box that takes typed characters, so `?` types into it
    /// rather than opening the panel -- the rule every other search box
    /// follows.
    #[test]
    fn a_question_mark_types_into_the_destination_search_rather_than_opening_the_panel() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('?'));

        assert!(app.help.is_none(), "the panel opened over the search box");
        let Some(Modal::Destination(chooser)) = &app.modal else {
            panic!("the list closed");
        };
        let title: String = chooser
            .title()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(title.contains("/?"), "{title}");

        // F1 is the way in while a question mark is a character.
        press(&mut app, KeyCode::F(1));
        assert!(open_on(&app, Topic::DestinationSearch));
    }

    /// `TransferConfirm` has one field and it is a date, which holds no literal
    /// `?` -- the same reasoning that excludes the worksheet's date focus -- so
    /// a plain `?` reaches the panel rather than typing into the field.
    #[test]
    fn a_plain_question_mark_opens_the_panel_over_the_transfer_confirmation() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));
        assert!(
            matches!(app.modal, Some(Modal::PlanTransfers(_))),
            "no transfer confirmation opened"
        );

        press(&mut app, KeyCode::Char('?'));
        assert!(open_on(&app, Topic::PlanTransfers));
        assert!(
            matches!(app.modal, Some(Modal::PlanTransfers(_))),
            "the confirmation is still queued behind the panel"
        );
    }

    /// A second run on the same date is a real case -- a corrected date --
    /// so it warns rather than blocks.
    #[test]
    fn a_second_t_on_the_same_date_warns_without_blocking() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        // `planning_app` already seeds one paycheck row, so the payday's own
        // count is `after_first - before`, not `after_first` outright.
        let before = crate::db::txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        let after_first = crate::db::txn::count(&app.db).unwrap();
        let payday_rows = after_first - before;
        // The commit opened the receiving containers' worksheets in turn;
        // dismiss both before the second `t`, which is a Planning-screen key.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none(), "a worksheet is still queued");

        press(&mut app, KeyCode::Char('t'));
        assert!(app.status.contains("already"), "{}", app.status);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);

        assert_eq!(
            crate::db::txn::count(&app.db).unwrap(),
            after_first + payday_rows
        );
    }

    /// A key pointing at a goal that no longer exists is a corrupt database,
    /// not an empty payday. `t` must refuse it before the modal opens, not
    /// let the owner confirm a plan `transfer::plan` could not resolve.
    #[test]
    fn t_on_an_unresolved_plan_refuses_and_opens_no_modal() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let Destination::Goal(key) = Line::MomAndDad.destination() else {
            panic!("Mom & Dad is goal-backed");
        };
        setting::set(&app.db, key, GoalId(9_999)).unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert!(
            app.modal.is_none(),
            "a modal opened over an unresolved plan"
        );
        assert!(
            app.status.contains("planning.goal.mom_and_dad_id"),
            "{}",
            app.status
        );
    }

    /// Committing the transfers opens the receiving containers' worksheets in
    /// turn, each prefilled with the lines that landed there and the plug
    /// spread over the goals no line claims.
    #[test]
    fn committing_the_transfers_opens_each_containers_worksheet_in_turn() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let first = match &app.modal {
            Some(Modal::Worksheet(sheet)) => sheet.container(),
            _ => panic!("no worksheet opened"),
        };
        // Esc closes one worksheet and the next opens behind it.
        press(&mut app, KeyCode::Esc);
        let second = match &app.modal {
            Some(Modal::Worksheet(sheet)) => sheet.container(),
            _ => panic!("the second worksheet did not open"),
        };
        assert_ne!(first, second);

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none(), "a third worksheet opened");
    }

    /// The Rainy Day worksheet's pot is the transfer, and its lines are the
    /// claimed goals at their own amounts plus the plug spread over the rest
    /// -- so the sheet opens reconciled and the owner reviews rather than
    /// retypes.
    #[test]
    fn the_first_worksheet_opens_with_the_transfer_as_its_pot() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        // The plan behind this payday, read fresh: the same figures `t`
        // itself just used to build the confirm modal and the prefill.
        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();
        let Destination::Goal(bills_key) = Line::Bills.destination() else {
            panic!("Bills is goal-backed");
        };
        let bill_payments = setting::get(&app.db, bills_key).unwrap().unwrap();
        let Destination::Goal(roth_key) = Line::Roth.destination() else {
            panic!("Roth is goal-backed");
        };
        let roth = setting::get(&app.db, roth_key).unwrap().unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet opened");
        };
        assert_eq!(
            sheet.remaining(),
            Cents::ZERO,
            "the sheet did not open reconciled"
        );
        assert!(sheet.amount() > Cents::ZERO);

        // The central claim: each claimed goal carries its own line's
        // amount, not a share of the pot dumped somewhere convenient.
        let line = |id: GoalId| {
            sheet
                .lines()
                .into_iter()
                .find(|l| l.goal_id == id)
                .unwrap_or_else(|| panic!("no worksheet line for goal {id}"))
        };
        assert_eq!(line(bill_payments).amount, plan.lines.bills);
        assert_eq!(
            line(roth).amount,
            Cents::ZERO,
            "the Roth gate is already met, so plan emits no row for it"
        );
    }

    /// Nothing stops two lines naming one goal -- `open_destination` offers
    /// every open goal, claimed or not -- and `transfer::plan` merges them
    /// into one transfer. The prefill has to merge them the same way, or the
    /// sheet opens short by the second line and the difference is
    /// indistinguishable from the remainder `calc::fit` leaves on purpose.
    ///
    /// Housing is funded before it is unpointed: a goal no line claims and
    /// which is still short joins the plug, and a plug spanning Rainy Day and
    /// Brokerage is a refusal rather than the case under test.
    #[test]
    fn two_lines_naming_one_goal_prefill_it_with_their_sum() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let savings = account::by_code(&app.db, "SAV", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        let goal_id = |name: &str| {
            goal::list_with_balances(&app.db, savings)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no {name:?} goal in Rainy Day"))
                .goal
                .id
        };
        let bill_payments = goal_id("Bill Payments");
        goal::insert_allocation(
            &app.db,
            goal_id("Housing"),
            day(2026, 8, 1),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
        let Destination::Goal(housing_key) = Line::CurrentHousing.destination() else {
            panic!("Current Housing is goal-backed");
        };
        setting::set(&app.db, housing_key, bill_payments).unwrap();
        app.reload().unwrap();
        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet opened");
        };
        let line = sheet
            .lines()
            .into_iter()
            .find(|l| l.goal_id == bill_payments)
            .expect("no worksheet line for Bill Payments");
        assert!(plan.lines.current_housing > Cents::ZERO, "nothing to lose");
        assert_eq!(line.amount, plan.lines.bills + plan.lines.current_housing);
        assert_eq!(
            sheet.remaining(),
            Cents::ZERO,
            "the sheet did not open reconciled"
        );
    }

    /// The rows a payday of this fixture writes, put on the ledger at
    /// `date` -- a first run, so a later `t` has something to clash with.
    fn payday_landed_on(app: &App, date: NaiveDate) {
        let from = transfer::source(&app.db).unwrap();
        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();
        let rows = transfer::plan(&app.db, &plan.lines).unwrap();
        transfer::execute(&app.db, from, date, &rows).unwrap();
    }

    /// The form's date is editable, so the day a first run landed on is not
    /// the day the second run opens on -- which is the whole case the warning
    /// exists for. It scans business days either side and names what it
    /// found, because those are days the owner cannot see on the form.
    ///
    /// The fixture's today is a Saturday, so the default is Tuesday the 18th
    /// and Monday the 17th is one business day behind it.
    #[test]
    fn t_warns_when_a_neighbouring_business_day_already_carries_the_payday() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 17));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert_eq!(app.status, "2026-08-17 already carries matching rows");
    }

    /// Two clashes are both named: the owner is picking a date, and being
    /// told about one of the two days to avoid is worse than being told the
    /// count.
    #[test]
    fn t_names_every_neighbouring_day_that_clashes() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 17));
        payday_landed_on(&app, day(2026, 8, 19));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert_eq!(
            app.status,
            "2026-08-17 and 2026-08-19 already carry matching rows"
        );
    }

    /// The window has an edge, and past it the warning stays quiet: Friday
    /// the 21st is three business days past the 18th.
    #[test]
    fn t_is_quiet_about_a_payday_beyond_the_window() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 21));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert!(app.status.is_empty(), "{}", app.status);
    }

    /// `planning_app`'s spare goals live in Brokerage, so the Rainy Day sheet
    /// above never touches `worksheet_prefills`' `Destination::Spread`
    /// branch at all -- the second sheet, Brokerage, is the one worksheet
    /// that does.
    ///
    /// Dated here rather than in the fixture: an undated goal asks for
    /// nothing, and a sheet where every spare goal asks nothing exercises the
    /// division not at all. The sibling test below is the undated case.
    #[test]
    fn the_second_worksheet_prefills_the_plugs_goals_with_what_they_ask() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        let goal_id = |name: &str| {
            goal::list_with_balances(&app.db, brokerage)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no {name:?} goal in Brokerage"))
                .goal
                .id
        };
        let down_payment = goal_id("Home Down Payment");
        let lego = goal_id("Lego");
        let dropbox = goal_id("Dropbox");
        // One paycheck away, so each asks for the whole of what it lacks:
        // $1,000 apiece against a plug of several thousand, which fits.
        for id in [lego, dropbox] {
            goal::update(
                &app.db,
                id,
                &goal::GoalEdit {
                    name: goal::get(&app.db, id).unwrap().unwrap().name,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: Some(app.today + chrono::Duration::days(7)),
                    interest_eligible: true,
                    taxed: false,
                },
            )
            .unwrap();
        }

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("the second worksheet did not open");
        };
        assert_eq!(
            sheet.amount(),
            plan.lines.future_housing + plan.lines.mom_and_dad + plan.lines.goals
        );

        let line = |id: GoalId| {
            sheet
                .lines()
                .into_iter()
                .find(|l| l.goal_id == id)
                .unwrap_or_else(|| panic!("no worksheet line for goal {id}"))
        };
        assert_eq!(line(down_payment).amount, plan.lines.future_housing);

        // Each asked for the $1,000 it lacks and got it, whole.
        assert_eq!(line(lego).amount, Cents::from_dollars(1_000));
        assert_eq!(line(dropbox).amount, Cents::from_dollars(1_000));

        // And the rest of the plug is left where the owner can place it,
        // rather than shared out over goals that did not ask for it.
        assert_eq!(
            sheet.remaining(),
            plan.lines.goals - Cents::from_dollars(2_000),
        );
    }

    /// The plug's gap is the one figure on the Planning screen that comes
    /// from outside the waterfall: `transfer::spread_asks`, over the same
    /// goals `t`'s own prefill divides that line between. Asserted through
    /// `App`, because nothing in `tui::planning` reads a database -- a `View`
    /// left carrying zero would draw a silent footer on every real payday and
    /// every test in that module would still pass.
    #[test]
    fn the_planning_screens_plug_is_measured_against_its_own_goals_asks() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        // In the container the plug already spreads over, dated one pay
        // period out -- so it asks for the whole of what it lacks, which is
        // far past anything this payday moves.
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Roof".to_string(),
                container_account_id: brokerage,
                base_cents: Cents::from_dollars(500_000),
                goal_date: Some(app.today + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();

        let start = app
            .planning
            .rows()
            .iter()
            .position(|r| r.label == "Transfers")
            .expect("no Transfers heading");
        let footer = app.planning.rows()[start..]
            .iter()
            .take_while(|r| !(r.label.is_empty() && r.value.is_empty()))
            .find(|r| r.label.trim() == "Unmet Asks")
            .expect("no Unmet Asks footer among the transfers");

        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();
        let gap = plan.lines.goals - Cents::from_dollars(500_000);
        assert_eq!(footer.extra, format!("\u{394} {}", gap.to_whole_dollars()));
    }

    /// The payday `Unmet Asks` exists for is the one where every line is
    /// zero, and that is the payday `transfer::plan` refuses outright -- so
    /// the asks the footer measures the plug against cannot be read through
    /// that call. Asserted through `App` for the reason the test above it is:
    /// a `View` built by hand carries whatever total it is given, and every
    /// test in `tui::planning` would still pass over a screen that had gone
    /// silent on the one payday the footer exists for.
    #[test]
    fn a_payday_with_nothing_to_transfer_still_reports_what_its_goals_asked() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Roof".to_string(),
                container_account_id: brokerage,
                base_cents: Cents::from_dollars(500_000),
                goal_date: Some(app.today + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        // An excess of nothing puts every line at zero -- what a payday whose
        // fixed bills took the whole of it produces, and what `plan` refuses.
        setting::set(&app.db, key::PINNED_EXCESS, Cents::ZERO).unwrap();
        app.reload().unwrap();

        let rows = app.planning.rows().to_vec();
        let start = rows
            .iter()
            .position(|r| r.label == "Transfers")
            .expect("no Transfers heading");
        let block: Vec<_> = rows[start..]
            .iter()
            .take_while(|r| !(r.label.is_empty() && r.value.is_empty()))
            .collect();

        assert!(
            block
                .iter()
                .any(|r| r.label.trim() == transfer::NOTHING_TO_TRANSFER),
            "the block is not the one this payday produces: {block:?}"
        );
        let footer = block
            .iter()
            .find(|r| r.label.trim() == "Unmet Asks")
            .expect("no Unmet Asks footer on a payday with nothing to transfer");
        let gap = Cents::ZERO - Cents::from_dollars(500_000);
        assert_eq!(footer.extra, format!("\u{394} {}", gap.to_whole_dollars()));
    }

    /// The plug is priced against the **target**, so a taxed goal funded to
    /// its base is still short by the tax and still asks for it. Asserted
    /// through `App` rather than on `transfer::spread_asks` directly, because
    /// what this pins is that the two dates the ask divides by are the ones
    /// the application is running on.
    #[test]
    fn a_taxed_goals_plug_ask_is_measured_against_its_taxed_target() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                // One pay period (14 days) past `today`, so the ask is the
                // whole of what is lacking rather than a fraction of it.
                goal_date: Some(today() + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, id, today(), Cents::from_dollars(1_000), None, None).unwrap();

        let app = App::new(db, today()).unwrap();
        let asks = app.spread_asks().unwrap();
        let (_, ask) = asks
            .into_iter()
            .find(|(g, _)| g.id == id)
            .unwrap_or_else(|| panic!("no plug entry for the taxed goal"));

        // 1,000 taxed at 6.25% is 1,062.50, carried up to 1,065 by the
        // lambda's $5 increment. Funded to the base, the goal still lacks
        // the $65 of tax.
        assert_eq!(ask, Cents::from_dollars(65));
    }

    /// An undated goal has no runway to divide, so it asks for nothing and
    /// the whole plug is left unallocated. That is the intended answer, not a
    /// gap: money nobody has dated a use for is money to place by hand.
    #[test]
    fn a_plug_whose_goals_ask_for_nothing_is_left_unallocated() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let plan = plan::compute_from_db(&app.db, app.today).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("the second worksheet did not open");
        };
        assert_eq!(sheet.remaining(), plan.lines.goals);
    }

    /// `worksheet_prefills` calls `spread_container` unconditionally, even
    /// when this payday's own Goals plug is zero and `transfer::plan` itself
    /// never needed a spread destination. A database whose unclaimed goals
    /// span two containers must therefore refuse before anything is
    /// written, not after: `already_written` is never consulted on a
    /// re-confirm, so a refusal stranded after `transfer::execute` would let
    /// a second `Enter` duplicate the whole payday.
    #[test]
    fn a_zero_plug_with_unclaimed_goals_in_two_containers_refuses_before_writing() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();
        // A round $10,000.00, so Future Housing/Retirement/Investment's
        // 40/30/30 split lands on whole dollars with nothing left for Goals
        // to absorb.
        write(&db, checking, day(2026, 8, 1), 1_000_000, "Paycheck");

        let new_goal = |container: AccountId, name: &str, sort: i64| -> GoalId {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: container,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort,
                    taxed: false,
                },
            )
            .unwrap()
        };
        let down_payment = new_goal(brokerage, "Home Down Payment", 0);
        // Two unclaimed goals, one per container, so `unclaimed_goals` spans
        // both -- the exact state `spread_container` refuses to divide the
        // plug over.
        new_goal(savings, "Stray Rainy Day", 0);
        new_goal(brokerage, "Stray Brokerage", 1);

        let Destination::Goal(fh_key) = Line::FutureHousing.destination() else {
            panic!("Future Housing is goal-backed");
        };
        setting::set(&db, fh_key, down_payment).unwrap();
        // No bills, no Mom & Dad, and Future Housing + Retirement +
        // Investment sum to exactly 100% of a whole-dollar remainder: every
        // other line is zero and the three that aren't leave no residue for
        // Goals to absorb, so the plug lands on exactly `Cents::ZERO`
        // rather than a rounding leftover.
        setting::set(&db, key::PLANNING_TARGET, Cents::ZERO).unwrap();
        setting::set(&db, key::PLANNING_BUFFER, Cents::ZERO).unwrap();
        setting::set(&db, key::BILL_PAYMENT_CAP, Cents::ZERO).unwrap();
        setting::set(&db, key::MOM_AND_DAD_ANNUAL, Cents::ZERO).unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(40)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(30)).unwrap();

        let mut app = App::new(db, today()).unwrap();
        app.screen = Screen::Planning;
        let plan = plan::compute_from_db(&app.db, app.adhoc).unwrap();
        assert_eq!(
            plan.lines.goals,
            Cents::ZERO,
            "the fixture must zero the plug for this test to mean anything"
        );

        let before = crate::db::txn::count(&app.db).unwrap();
        press(&mut app, KeyCode::Char('t'));
        assert!(
            app.modal.is_some(),
            "the confirm modal did not open: {}",
            app.status
        );

        press(&mut app, KeyCode::Enter);

        assert_eq!(
            crate::db::txn::count(&app.db).unwrap(),
            before,
            "the payday wrote rows before the container-spanning plug was refused"
        );
        assert!(
            app.status.contains("Rainy Day") && app.status.contains("Brokerage"),
            "{}",
            app.status
        );
    }

    fn recurring_txns_app() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        // The workbook's pre-entered future rows, unclaimed. A "Salary
        // Income" row lets the paycheck form's Esc dismiss the suggestion
        // popup instead of the whole form, the same way "Mortgage" does.
        write(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 10, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 8, 28), 500_000, "Salary");
        App::new(db, today()).unwrap()
    }

    fn add_mortgage_rule(app: &mut App) {
        press(app, KeyCode::Char('8'));
        press(app, KeyCode::Char('a'));
        type_str(app, "Mortgage");
        press(app, KeyCode::Esc); // dismiss the suggestion popup, keep the form
        press(app, KeyCode::Tab);
        type_str(app, "-1200.00");
        press(app, KeyCode::Tab);
        press(app, KeyCode::Tab);
        press(app, KeyCode::Right); // monthly
        press(app, KeyCode::Tab);
        for _ in 0..10 {
            press(app, KeyCode::Backspace);
        }
        type_str(app, "2026-09-01");
        press(app, KeyCode::Enter);
    }

    #[test]
    fn seven_opens_the_rules_screen_and_a_writes_a_rule() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);

        assert_eq!(app.screen, Screen::RecurringTxns);
        assert!(app.modal.is_none(), "{}", app.status);
        let recurring_txn = crate::db::recurring_txn::list(&app.db).unwrap();
        assert_eq!(recurring_txn.len(), 1);
        assert_eq!(recurring_txn[0].description, "Mortgage");
        assert_eq!(
            recurring_txn[0].cadence,
            crate::db::recurring_txn::Cadence::Monthly
        );
        assert_eq!(
            app.recurring_txn.rows()[0].owned,
            0,
            "nothing generated yet"
        );
    }

    /// Adoption is invisible otherwise: "adopted 2" is how the owner sees that
    /// the first `g` claimed the imported rows instead of duplicating them.
    #[test]
    fn g_regenerates_the_selected_rule_and_reports_its_counts() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        let before = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert!(app.status.contains("adopted 2"), "{}", app.status);
        assert!(app.status.contains("inserted 1"), "{}", app.status);
        assert_eq!(
            txn::count(&app.db).unwrap(),
            before + 1,
            "the two imported rows were adopted, not duplicated"
        );
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    /// From 2026-08-15 the three-month window ends 2026-11-15, so the
    /// mortgage's rows stop at 2026-11-01. One `x` buys another window: the
    /// rows run to 2027-02-01, and the screen's date column says so.
    #[test]
    fn x_extends_the_selected_rule_and_the_last_date_follows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2026, 11, 1))
        );

        press(&mut app, KeyCode::Char('x'));

        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2027, 2, 1))
        );
        assert_eq!(app.recurring_txn.rows()[0].owned, 6);
        assert!(app.status.contains("2027-02-15"), "{}", app.status);
    }

    /// The extension is durable, or the next `g` would sweep the extra rows
    /// straight back out again.
    #[test]
    fn a_g_after_an_x_keeps_the_extended_rows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('x'));
        let extended = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert_eq!(txn::count(&app.db).unwrap(), extended);
        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2027, 2, 1))
        );
    }

    /// A recurring transaction that ends cannot be extended past its end:
    /// moving that date is the form's decision, and the status line says
    /// where to make it.
    #[test]
    fn x_on_a_rule_that_has_ended_names_the_date_it_ends() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        let id = app.recurring_txn.rows()[0].recurring_txn_id;
        let mut ends = recurring_txn::get(&app.db, id).unwrap();
        ends.horizon = Some(day(2026, 10, 1));
        recurring_txn::update(
            &app.db,
            id,
            &recurring_txn::NewRecurringTxn {
                description: ends.description,
                cents: ends.cents,
                account_id: ends.account_id,
                cadence: ends.cadence,
                anchor_date: ends.anchor_date,
                horizon: ends.horizon,
            },
        )
        .unwrap();

        press(&mut app, KeyCode::Char('x'));

        assert!(app.status.contains("2026-10-01"), "{}", app.status);
        assert_eq!(
            recurring_txn::get(&app.db, id).unwrap().generate_through,
            None,
            "a refused extension writes nothing"
        );
    }

    /// Regenerating twice in a row must produce identical rows.
    #[test]
    fn a_second_g_changes_nothing() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        let after_first = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert_eq!(txn::count(&app.db).unwrap(), after_first);
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    #[test]
    fn capital_g_regenerates_every_rule() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('G'));
        assert!(app.status.contains("adopted 2"), "{}", app.status);
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    /// The whole point of the flag: Paycheck-Eve stops being today.
    #[test]
    fn capital_p_names_the_paycheck_rule_and_the_overview_column_follows_it() {
        let mut app = recurring_txns_app();
        assert_eq!(app.dates.adhoc, today(), "no paycheck transaction yet");

        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Salary");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "5000.00");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-08-28");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('P'));

        assert!(app.recurring_txn.rows()[0].is_paycheck);
        assert_eq!(app.dates.adhoc, day(2026, 8, 27));
        assert_eq!(
            app.adhoc,
            day(2026, 8, 27),
            "the scrub follows the baseline"
        );
        assert_eq!(app.scrubbed_days(), 0);
    }

    /// At offset 0 `self.adhoc = self.dates.adhoc` would pass just as well --
    /// this scrubs first, so only carrying the *offset* across the moved
    /// baseline can make it pass.
    #[test]
    fn capital_p_moves_the_baseline_and_carries_a_nonzero_scrub_across_it() {
        let mut app = recurring_txns_app();
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.scrubbed_days(), 2, "scrubbed two days off today");

        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Salary");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "5000.00");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-08-28");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('P'));

        assert_eq!(app.dates.adhoc, day(2026, 8, 27), "the baseline moved");
        assert_eq!(
            app.scrubbed_days(),
            2,
            "the scrub is unchanged, not reset by the moved baseline"
        );
        assert_eq!(
            app.adhoc,
            day(2026, 8, 29),
            "still two days past the new baseline"
        );
    }

    /// Deleting a recurring transaction must not silently move a balance.
    #[test]
    fn d_then_y_deletes_a_rule_and_releases_its_rows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        let rows = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(crate::db::recurring_txn::list(&app.db).unwrap().is_empty());
        assert_eq!(txn::count(&app.db).unwrap(), rows, "no row was deleted");
        assert!(app.status.contains("released"), "{}", app.status);
    }

    #[test]
    fn d_on_an_empty_rules_screen_says_nothing_is_selected() {
        let mut app = recurring_txns_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('d'));
        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing selected"), "{}", app.status);
    }

    #[test]
    fn e_opens_the_selected_rule_prefilled() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);

        press(&mut app, KeyCode::Char('e'));

        match &app.modal {
            Some(Modal::RecurringTxn(form)) => {
                assert_eq!(
                    form.display(crate::tui::recurring_txn::RecurringTxnField::Description)
                        .plain_text(),
                    "Mortgage"
                );
                assert_eq!(
                    form.editing,
                    Some(crate::db::recurring_txn::list(&app.db).unwrap()[0].id)
                );
            }
            _ => panic!("no recurring transaction form is open"),
        }
    }

    fn all_cash_rows(db: &Db) -> Vec<crate::db::txn::Txn> {
        txn::list(
            db,
            &Filter {
                kind: Kind::Cash,
                account_id: None,
                from: day(2000, 1, 1),
                to: day(2100, 1, 1),
            },
        )
        .unwrap()
    }

    /// `e`'s only write is `recurring_txn::update`; it does not regenerate.
    /// When the anchor moves and `g` runs next, the hand-corrected row from
    /// the old schedule is exempt from `release_generated` (it is `edited`)
    /// and matches no occurrence the new schedule produces, so step 1b hands
    /// it back to the ledger: the correction survives untouched, but as an
    /// ordinary row the owner can keep or delete rather than one the recurring
    /// transaction still claims.
    ///
    /// The row itself stays: nothing decides that a correction moved off its
    /// occurrence *consumes* that occurrence, so four rows stand where the
    /// schedule says three. Ownership is coherent; the count is the owner's
    /// call.
    #[test]
    fn editing_a_rules_anchor_releases_the_hand_corrected_row_at_the_old_occurrence() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.recurring_txn.rows()[0].owned, 3, "09-01, 10-01, 11-01");

        // Hand-correct the first generated row -- the same effect a ledger
        // edit has: any write through `txn::update` on a recurring
        // transaction-owned row flags it `edited`.
        let original = all_cash_rows(&app.db)
            .into_iter()
            .find(|t| t.date == day(2026, 9, 1))
            .unwrap();
        txn::update(
            &app.db,
            original.id,
            &NewTxn {
                date: original.date,
                cents: Cents(-300_000),
                account_id: original.account_id,
                description: original.description.clone(),
                recurring_txn_id: original.recurring_txn_id,
            },
        )
        .unwrap();

        // Move the recurring transaction's whole schedule off the 1st and onto
        // the 15th.
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab); // Amount
        press(&mut app, KeyCode::Tab); // Account
        press(&mut app, KeyCode::Tab); // Cadence
        press(&mut app, KeyCode::Tab); // Anchor
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-09-15");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "{}", app.status);

        // `e` alone does not regenerate: nothing moves until the next `g`.
        press(&mut app, KeyCode::Char('g'));

        assert!(app.status.contains("released 1"), "{}", app.status);
        let rows = all_cash_rows(&app.db);
        let given_back = rows
            .iter()
            .find(|t| t.date == day(2026, 9, 1))
            .expect("the hand-corrected row was released, not deleted");
        assert_eq!(given_back.cents, Cents(-300_000), "the correction survives");
        assert_eq!(
            given_back.recurring_txn_id, None,
            "no longer claimed by a recurring transaction whose schedule does not produce this date"
        );
        assert!(!given_back.edited, "a released row is nobody's correction");

        // The released row stays in, not filtered out: a spurious second
        // insert at 09-01 must fail this the same way a missing 09-15 would.
        let mortgage_dates: Vec<NaiveDate> = rows
            .iter()
            .filter(|t| t.description == "Mortgage")
            .map(|t| t.date)
            .collect();
        assert_eq!(
            mortgage_dates,
            vec![
                day(2026, 9, 1),
                day(2026, 9, 15),
                day(2026, 10, 15),
                day(2026, 11, 15),
            ],
            "the recurring transaction regenerated on the new schedule, leaving the released row behind rather than folding it in"
        );
        assert_eq!(
            app.recurring_txn.rows()[0].owned,
            3,
            "the recurring transaction owns its three occurrences and nothing else"
        );
    }

    #[test]
    fn seven_opens_the_catalog_and_a_adds_an_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "128");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let entries = recurring_goal::list(&app.db).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Dropbox");
        assert_eq!(app.recurring_goal.rows()[0].name, "Dropbox");
    }

    fn add_dropbox_entry(app: &mut App) {
        press(app, KeyCode::Char('7'));
        press(app, KeyCode::Char('a'));
        type_str(app, "Dropbox");
        press(app, KeyCode::Tab);
        for _ in 1..9 {
            press(app, KeyCode::Right); // January to September
        }
        press(app, KeyCode::Tab);
        type_str(app, "128");
        press(app, KeyCode::Enter);
    }

    /// The keys reach the screen. `app()`'s today is in August, so the first
    /// `]` out of All lands there — where nothing recurs — and the
    /// second on September, where the Dropbox entry is.
    #[test]
    fn the_bracket_keys_and_esc_filter_the_catalog_by_month() {
        let mut app = app();
        add_dropbox_entry(&mut app);
        assert_eq!(app.recurring_goal.selected_month(), None);

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.selected_month(), Some(8));
        assert!(
            app.recurring_goal.rows().is_empty(),
            "August holds no entry"
        );

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.rows()[0].name, "Dropbox");

        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.recurring_goal.selected_month(), Some(8));

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.recurring_goal.selected_month(), None);
        assert_eq!(app.recurring_goal.rows().len(), 1);
    }

    /// `Tab` is the one key that moves the ledger total: All is the kind's
    /// balance, and narrowing to an account is that account's. Both are
    /// balances *at today*, so the Rent row dated the 20th is in neither.
    #[test]
    fn tab_moves_the_ledger_total_from_the_kind_to_the_selected_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        // Everyday 1,000.00 - 50.00, and Rainy Day 200.00.
        assert_eq!(app.cash.total(), Cents(115_000));
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.cash.selected_account(),
            Some(app.cash.rows()[0].account_id)
        );
        assert_eq!(app.cash.total(), Cents(95_000), "Everyday alone");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.cash.total(), Cents(20_000), "Rainy Day alone");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.cash.total(), Cents(115_000), "back to All");
    }

    /// The Credit ledger renders amounts as stored — debt positive — and its
    /// total agrees with the column above it rather than with the Overview,
    /// which is the one screen that negates.
    #[test]
    fn the_credit_total_reads_as_stored_debt_where_the_overview_negates_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.credit.total(), Cents(4_098));
        assert_eq!(app.overview.credit.total.to_date, Cents(-4_098));
    }

    /// The total is a balance at a date, so the rows it is drawn over do not
    /// bound it: stepping the window changes the rows and leaves it alone.
    /// The future-dated row is outside it under either window.
    #[test]
    fn stepping_the_month_leaves_the_ledger_total_alone() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        write(&db, checking, day(2026, 7, 4), 50_000, "July");
        write(&db, checking, day(2026, 8, 10), 30_000, "August");
        write(&db, checking, day(2026, 8, 20), 999_999, "Not yet");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.cash.rows().len(), 2, "August");
        assert_eq!(app.cash.total(), Cents(80_000));

        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.cash.rows().len(), 1, "July");
        assert_eq!(app.cash.total(), Cents(80_000));
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

    /// The keys reach the Savings screen. `app()` holds one dated goal
    /// (January 2027) and one undated one, so the month filter is also what
    /// hides the undated goal.
    #[test]
    fn the_bracket_keys_and_esc_filter_savings_by_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(
            savings_names(&app),
            ["Vacation 2027"],
            "the undated Couch belongs to no month"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// The screen narrows two ways, and `Esc` is the way out of both: an
    /// owner who has tabbed to a container and stepped to a month should not
    /// have to remember which of the two is hiding the goal they are looking
    /// for.
    #[test]
    fn esc_on_savings_clears_the_container_filter_as_well_as_the_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(']'));
        assert!(app.savings.selected_container().is_some());
        assert!(app.savings.selected_month().is_some());

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_container(), None);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// `Esc` inside the search box still clears the search, which is the
    /// handler the month filter must not have taken over.
    #[test]
    fn esc_in_the_savings_search_box_clears_the_search_not_the_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "vac");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.savings.search(), "");
        assert!(app.savings.selected_month().is_some());
    }

    /// `Enter` leaves the box and keeps the filter, so `Esc` outside the box
    /// is what clears it -- and the needle goes before the container and the
    /// month, which `clear_filters` then takes together.
    #[test]
    fn esc_outside_the_savings_box_clears_a_kept_search_before_the_other_filters() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "vac");
        press(&mut app, KeyCode::Enter);
        assert!(!app.savings.is_searching(), "Enter left the box");
        assert_eq!(app.savings.search(), "vac");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.search(), "");
        assert!(
            app.savings.selected_container().is_some(),
            "the container is the next thing out, not this one"
        );
        assert!(
            app.savings.selected_month().is_some(),
            "and so is the month"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_container(), None);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// The same order on a ledger, where the outer filter is the window.
    #[test]
    fn esc_outside_a_ledger_box_clears_a_kept_search_before_the_window() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));
        let september = app.cash.window();
        assert_ne!(september, august);

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "sept");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.cash.rows().len(), 1);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.search(), "");
        assert_eq!(
            app.cash.window(),
            september,
            "the window is the next thing out"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.window(), august);
    }

    /// The needle is one ledger's; the window is both. Clearing a kept filter
    /// on Cash must not reach across to Credit the way `Esc` on the window
    /// deliberately does.
    #[test]
    fn clearing_a_kept_filter_on_one_ledger_leaves_the_other_alone() {
        let mut app = app_spanning_three_months();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "card");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "aug");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.cash.search(), "");
        assert_eq!(app.credit.search(), "card");
    }

    /// On a modal the outer thing is the modal itself, so a kept filter has to
    /// be cleared before `Esc` may throw the worksheet away.
    #[test]
    fn esc_with_a_kept_filter_clears_it_rather_than_cancelling_the_worksheet() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "couch");
        press(&mut app, KeyCode::Enter);
        assert_eq!(worksheet(&app).lines().len(), 1);

        press(&mut app, KeyCode::Esc);
        assert_eq!(worksheet(&app).search(), "");
        assert_eq!(worksheet(&app).lines().len(), 2, "the sheet is still open");

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    #[test]
    fn esc_with_a_kept_filter_clears_it_rather_than_closing_the_destination_list() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "zzz");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Esc);
        let Some(Modal::Destination(chooser)) = &app.modal else {
            panic!("the list closed");
        };
        assert_eq!(chooser.search(), "");

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    #[test]
    fn e_opens_the_selected_catalog_entry_prefilled() {
        let mut app = app();
        add_dropbox_entry(&mut app);

        press(&mut app, KeyCode::Char('e'));

        match &app.modal {
            Some(Modal::RecurringGoalEntry(form)) => {
                assert_eq!(
                    form.display(crate::tui::recurring_goal::RecurringGoalField::Name)
                        .plain_text(),
                    "Dropbox"
                );
                assert_eq!(
                    form.editing,
                    Some(recurring_goal::list(&app.db).unwrap()[0].id)
                );
            }
            _ => panic!("no recurring goal form is open"),
        }
    }

    /// The wiring `e` depends on: `commit_recurring_goal` must reach
    /// `recurring_goal::update` with the id the form is editing. An `insert`
    /// here, or the wrong id, leaves the old entry in place beside a new one.
    #[test]
    fn committing_an_edited_catalog_entry_updates_it_rather_than_adding_a_second() {
        let mut app = app();
        add_dropbox_entry(&mut app);
        let before = recurring_goal::list(&app.db).unwrap()[0].id;

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab); // Month
        press(&mut app, KeyCode::Tab); // Base
        for _ in 0..6 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "144");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let entries = recurring_goal::list(&app.db).unwrap();
        assert_eq!(entries.len(), 1, "the entry was updated, not duplicated");
        assert_eq!(entries[0].id, before);
        assert_eq!(entries[0].base_cents, Cents::from_dollars(144));
        assert_eq!(
            app.recurring_goal.rows()[0].base_cents,
            Cents::from_dollars(144)
        );
    }

    #[test]
    fn e_on_an_empty_catalog_says_nothing_is_selected() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('e'));
        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing selected"), "{}", app.status);
    }

    #[test]
    fn d_on_a_catalog_entry_with_goals_against_it_is_refused_and_says_why() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "128");
        press(&mut app, KeyCode::Enter);
        // Create a goal from it, through the picker `s` opens.
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(recurring_goal::list(&app.db).unwrap().len(), 1);
        assert!(
            app.status.contains("reference this recurring goal"),
            "{}",
            app.status
        );
    }

    /// The screen's "Open" column and the confirmation box read from two
    /// different counts on purpose: `recurring_goal::goal_count`, which is
    /// what `recurring_goal::delete` actually gates on, counts a closed goal
    /// same as an open one. An entry whose only goal has been closed must
    /// still show and confirm a nonzero count, not the zero the "Open" column
    /// shows.
    #[test]
    fn d_on_a_catalog_entry_whose_only_goal_is_closed_still_confirms_a_nonzero_count() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        // $500.00 rather than $128.00: no digit `1` anywhere in the rendered
        // amount, so a stray `1` in the price cannot rescue an assertion
        // that is meant to be reading the goal count.
        type_str(&mut app, "500");
        press(&mut app, KeyCode::Enter);
        // Create a goal from it, through the picker `s` opens, then close it
        // out.
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);
        let goal_id = goal::all_with_balances(&app.db)
            .unwrap()
            .into_iter()
            .find(|g| g.goal.name == "Dropbox")
            .unwrap()
            .goal
            .id;
        goal::close(&app.db, goal_id).unwrap();
        // The close went straight through `db::goal`, bypassing `App`'s own
        // write path, so the cached screen needs an explicit reload to catch
        // up -- the same as any other out-of-band change would.
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('7'));
        assert_eq!(
            app.recurring_goal.rows()[0].open_goals,
            0,
            "the goal is closed, not open"
        );

        press(&mut app, KeyCode::Char('d'));
        match &app.modal {
            Some(Modal::Confirm {
                action: Confirm::DeleteRecurringGoal(_),
                label,
            }) => {
                // The exact count, not a bare digit: a label built from
                // `row.open_goals` would read "0 open goals" here, which
                // also happens to contain a `1` if the amount were $128.00
                // -- this is the assertion the review asked for.
                assert!(label.contains("1 goal(s)"), "{label}");
            }
            _ => panic!("no delete confirmation is open"),
        }

        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            recurring_goal::list(&app.db).unwrap().len(),
            1,
            "the closed goal still blocks the delete"
        );
        assert!(app.status.contains("1 goal(s)"), "{}", app.status);
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

    /// The panel is open, and on the topic named.
    fn open_on(app: &App, topic: Topic) -> bool {
        app.help.as_ref().is_some_and(|h| h.topic() == topic)
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
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
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
        for (name, day_of_month) in [("Mortgage", 1), ("Gym", 15)] {
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

    /// The birth date is a setting with no screen of its own, so the screen
    /// that needs it asks -- every time it is entered, until it is answered.
    #[test]
    fn entering_funds_with_an_age_row_and_no_birth_date_asks_for_one() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Bonds".to_string(),
                ord: 0,
                target: fund::Target::AgeOver30,
                actual: Cents::from_dollars(30_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        assert!(matches!(app.modal, Some(Modal::BirthDate(_))));

        // Esc dismisses it, and the screen still draws.
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert_eq!(app.screen, Screen::Funds);
        assert!(
            footer(&app).contains("birth date unset"),
            "{}",
            footer(&app)
        );

        // And it comes back on the next visit.
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('6'));
        assert!(matches!(app.modal, Some(Modal::BirthDate(_))));
    }

    /// Answering it is the last time it is asked.
    #[test]
    fn saving_a_birth_date_settles_the_question_and_gives_the_age_row_a_target() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Bonds".to_string(),
                ord: 0,
                target: fund::Target::AgeOver30,
                actual: Cents::from_dollars(30_000),
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('6'));

        let birth = app.today.with_year(app.today.year() - 44).unwrap();
        for c in birth.format("%Y-%m-%d").to_string().chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.funds.rows()[0].target, Some(BasisPoints(1_400)));

        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('6'));
        assert!(app.modal.is_none(), "the question is settled");
    }

    /// A fund table with no age row is never asked for a birth date.
    #[test]
    fn entering_funds_with_only_share_rows_asks_nothing() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(10_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));

        assert!(app.modal.is_none());
        assert_eq!(app.screen, Screen::Funds);
    }

    /// `commit_fund`'s insert branch, exercised through the keys rather than
    /// called directly -- this is what would catch `commit_fund` and
    /// `commit_fund_value` swapped on their `Modal` arms, which every test up
    /// to here still passes with.
    #[test]
    fn a_on_the_funds_screen_writes_a_fund() {
        let mut app = app();
        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "International");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "40");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "60,000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1);
        assert_eq!(funds[0].name, "International");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(4_000))
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(60_000));
    }

    /// `e` prefills the stored cents, not the whole dollars the row prints, so
    /// pressing Enter straight away must not quietly round it -- the claim
    /// `open_fund_value_edit`'s own comment makes.
    #[test]
    fn e_on_the_funds_screen_round_trips_the_value_unchanged() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1, "e must not add a second row");
        assert_eq!(funds[0].name, "Domestic");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(6_000))
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(90_000));
    }

    /// `commit_fund`'s update branch, and `open_fund_edit`'s prefill: `E`
    /// rewrites the selected row rather than adding a second, the same
    /// property `committing_capital_e_rewrites_the_bill_rather_than_adding_one`
    /// pins for Planning's bills.
    #[test]
    fn capital_e_on_the_funds_screen_rewrites_the_selected_fund() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('E'));
        for _ in 0.."Domestic".len() {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "Total Market");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1, "E updates the row rather than adding one");
        assert_eq!(funds[0].name, "Total Market");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(6_000)),
            "fields left untouched by the edit survive it"
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(90_000));
    }

    #[test]
    fn d_then_y_deletes_the_selected_fund() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(fund::list(&app.db).unwrap().is_empty());
        assert!(app.status.contains("fund deleted"), "{}", app.status);
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
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
        assert_eq!(sheet.focus(), worksheet::Focus::Date);
        assert_eq!(sheet.date_text(), "2026-08-16");

        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
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
        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        let stepped = confirm.commit().unwrap();

        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
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
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
        assert_eq!(sheet.date_text(), "2026-08-22");

        shift_press(&mut app, KeyCode::Left);
        shift_press(&mut app, KeyCode::Left);
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
        assert_eq!(sheet.date_text(), "2026-08-08");
    }

    /// `t`'s confirmation answers its own keys too, and its date is the one
    /// every payday transfer is stamped with.
    #[test]
    fn shift_and_an_arrow_step_the_transfer_confirmations_date_a_week() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        let opened = confirm.commit().unwrap();

        shift_press(&mut app, KeyCode::Right);
        let Some(Modal::PlanTransfers(confirm)) = &app.modal else {
            panic!("no transfer confirmation is open");
        };
        assert_eq!(
            confirm.commit().unwrap(),
            opened + chrono::TimeDelta::days(7)
        );
    }

    /// The worksheet is a modal, so it is reached through the Savings screen rather
    /// than a screen key -- but it implements the same `Scroll` and must answer the
    /// same keys.
    #[test]
    fn the_scroll_keys_work_inside_the_worksheet() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        // The line list is the third focus; the amount and the date come first.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);

        press(&mut app, KeyCode::End);
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
        let last = sheet.selected_index();

        press(&mut app, KeyCode::Home);
        let Some(Modal::Worksheet(sheet)) = &app.modal else {
            panic!("no worksheet is open");
        };
        assert_eq!(sheet.selected_index(), 0);
        assert!(last > 0, "End does not reach the last line");
    }

    /// The eighth `Scroll` implementor, and the one the shared fixtures cannot
    /// reach on their own: the picker only opens over recurring goal entries, so
    /// two are inserted here to give `End` somewhere to go.
    #[test]
    fn the_scroll_keys_work_inside_the_recurring_goal_picker() {
        let mut app = app();
        for name in ["Dropbox", "Rolex"] {
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
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        press(&mut app, KeyCode::End);
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker is open");
        };
        assert_eq!(picker.selected_index(), 1, "End must reach the last entry");

        press(&mut app, KeyCode::Home);
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker is open");
        };
        assert_eq!(picker.selected_index(), 0);
    }

    /// `n` is a free-form goal: a name, a target and a date, in the container
    /// the `Tab` filter names. Creating goals *from* recurring goal entries is
    /// `s` on screen 7, over on the table those entries live in.
    #[test]
    fn n_on_savings_opens_a_blank_goal_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));

        let Some(Modal::Goal(form)) = &app.modal else {
            panic!("no goal form is open");
        };
        assert_eq!(
            form.target(),
            GoalTarget::Create(app.savings.default_container().unwrap()),
            "a new goal has no id to edit, and lands in the default container"
        );
        assert_eq!(form.display(goal_form::GoalField::Name).plain_text(), "");
    }

    #[test]
    fn committing_a_blank_goal_form_creates_the_goal_in_the_tab_container() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        type_str(&mut app, "Bike");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Tab);
        // The date opens prefilled, so a typed one replaces it the way it
        // replaces the prefilled date on every other form.
        for _ in 0.."2026-09-01".len() {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2027-05-01");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open");
        let rows = app.savings.rows();
        let bike = rows
            .iter()
            .find(|r| r.name == "Bike")
            .expect("the new goal is not on the screen");
        assert_eq!(bike.goal, Cents::from_dollars(1_200));
        assert_eq!(bike.goal_date, Some(day(2027, 5, 1)));
        assert_eq!(
            bike.container.id(),
            app.savings.default_container().unwrap(),
            "the new goal landed outside the container the screen defaults to"
        );
    }

    /// The same guard `A` and `i` have: with no container there is nowhere for
    /// a goal to go, and the form must not open over a container it invented.
    #[test]
    fn n_on_savings_with_no_container_says_so() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));

        assert!(app.modal.is_none());
        assert_eq!(app.status, "no container holds goals yet");
    }

    /// Three entries: two in September and one in October, with one of
    /// September's already carrying an open goal.
    fn app_with_recurring_goals() -> App {
        let mut app = app();
        for (name, month) in [("Dropbox", 9), ("Lego", 9), ("Rolex", 10)] {
            let id = recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month,
                    base_cents: Cents::from_dollars(128),
                    taxed: false,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
            if name == "Dropbox" {
                goal::insert(
                    &app.db,
                    &goal::NewGoal {
                        name: name.to_string(),
                        container_account_id: app.savings.default_container().unwrap(),
                        base_cents: Cents::from_dollars(128),
                        goal_date: Some(day(2026, 9, 1)),
                        recurring_goal_id: Some(id),
                        interest_eligible: true,
                        sort: 9,
                        taxed: false,
                    },
                )
                .unwrap();
            }
        }
        app.reload().unwrap();
        app
    }

    /// `[`/`]` from All enter at today's month, so a second step reaches
    /// September.
    fn open_september_picker(app: &mut App) {
        press(app, KeyCode::Char('7'));
        press(app, KeyCode::Char(']'));
        press(app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.selected_month(), Some(9));
        press(app, KeyCode::Char('s'));
    }

    fn entries(app: &App) -> Vec<String> {
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker is open");
        };
        picker.entries().iter().map(|e| e.name.clone()).collect()
    }

    fn chosen(app: &App) -> Vec<String> {
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker is open");
        };
        picker.chosen().iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn s_preselects_the_entries_the_month_filter_is_showing() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert_eq!(chosen(&app), ["Lego"], "October's entry came along");
    }

    /// The annual reseed is what the preselection is for, and an entry that
    /// already has an open goal is one it has already been through. Selecting
    /// it again is still legal -- `Space` -- just not the default.
    #[test]
    fn s_does_not_preselect_an_entry_that_already_has_an_open_goal() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert!(!chosen(&app).contains(&"Dropbox".to_string()));
    }

    /// The filter preselects; it does not narrow. September's filter is a
    /// starting point the list can still be scrolled out of.
    #[test]
    fn s_lists_every_entry_even_under_a_month_filter() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("no picker is open");
        };
        let names: Vec<&str> = picker.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
    }

    /// Ticks alone are easy to miss in a list dozens long. The ticked entries are
    /// also the ones the list opens on, in the order the table holds them.
    #[test]
    fn s_sorts_the_entries_it_ticked_to_the_top() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert_eq!(entries(&app), ["Lego", "Dropbox", "Rolex"]);
    }

    /// Under All the split is open-goal alone, so the entries a reseed would
    /// create float above the ones it would skip.
    #[test]
    fn s_sorts_the_unopened_entries_above_the_rest_under_the_all_filter() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        assert_eq!(entries(&app), ["Lego", "Rolex", "Dropbox"]);
    }

    #[test]
    fn s_under_the_all_filter_preselects_every_unopened_entry() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        assert_eq!(chosen(&app), ["Lego", "Rolex"]);
    }

    #[test]
    fn enter_on_the_preselected_picker_creates_the_goals_it_shows_as_chosen() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        press(&mut app, KeyCode::Char('4'));
        assert!(savings_names(&app).contains(&"Lego".to_string()));
    }
    /// Everything on screen 9 is the owner's rather than the workbook's, and
    /// `e` is the one key that writes any of it.
    ///
    /// The account renamed is the goals' container, so the Savings rows -- which
    /// cache their container's name rather than resolving it as they draw --
    /// are in the set of things that have to move.
    #[test]
    fn editing_an_account_renames_it_everywhere_that_shows_a_name() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[1].id;
        assert_eq!(account::get(&app.db, id).unwrap().name, "Rainy Day");

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.modal, Some(Modal::Account(_))));
        for _ in 0..40 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(account::get(&app.db, id).unwrap().name, "Nest Egg");
        // The three screens holding their own account list see it without a
        // restart, which is what `reload_accounts` is for.
        assert_eq!(app.savings.account_name(id), "Nest Egg");
        assert_eq!(app.accounts.rows()[1].account.text(), "Nest Egg");
        assert!(
            app.overview
                .cash
                .bands
                .iter()
                .flat_map(|b| &b.lines)
                .any(|l| l.account.as_ref().is_some_and(|a| a.text() == "Nest Egg"))
        );
        // The cached column, which only moves if `reload` refreshes the account
        // list before it re-sets the goals.
        let cached: Vec<&str> = app
            .savings
            .rows()
            .iter()
            .filter(|r| r.container.id() == id)
            .map(|r| r.container.text())
            .collect();
        assert!(
            !cached.is_empty(),
            "the fixture's goals sit in this account"
        );
        assert!(cached.iter().all(|n| *n == "Nest Egg"), "{cached:?}");
    }

    /// The order is a place among the accounts of one kind, and the write
    /// renumbers all of them -- so moving the last cash account to the front
    /// reverses nothing else.
    #[test]
    fn reordering_an_account_moves_it_among_its_own_kind() {
        let mut app = app();
        let before: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert!(before.len() > 1, "the fixture needs two cash accounts");
        let cards: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Credit)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();

        press(&mut app, KeyCode::Char('9'));
        for _ in 1..before.len() {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(
            app.accounts.selected().unwrap().account.id(),
            *before.last().unwrap()
        );
        press(&mut app, KeyCode::Char('e'));
        // Tab to Order, then step it to the front.
        while !matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Order)
        {
            press(&mut app, KeyCode::Tab);
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        let after: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        let mut expected = before.clone();
        let moved = expected.pop().unwrap();
        expected.insert(0, moved);
        assert_eq!(after, expected);
        assert_eq!(
            account::list_by_kind(&app.db, Kind::Credit)
                .unwrap()
                .into_iter()
                .map(|a| a.id)
                .collect::<Vec<_>>(),
            cards,
            "reordering one kind moved the other"
        );
    }

    /// `a` writes the one row the workbook cannot: an account the sheet does
    /// not name. It lands last among its own kind, in that kind's default
    /// band and with no color, because everything else about an account is a
    /// placement and `e` is where an account is placed.
    #[test]
    fn a_creates_an_account_at_the_end_of_its_own_kind() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Cash).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "NST");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        assert_eq!(cash.len(), before + 1);
        let added = cash.last().unwrap();
        assert_eq!(added.code, "NST");
        assert_eq!(added.name, "Nest Egg");
        assert_eq!(added.group, Group::Savings);
        assert_eq!(added.color, None);
    }

    /// The kind selector is what puts a new account on the Credit ledger
    /// rather than the Cash one, and it is asked here because `e` cannot
    /// change it afterwards.
    #[test]
    fn a_creates_a_card_when_the_kind_selector_says_credit() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Credit).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "CC3");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Card Three");
        press(&mut app, KeyCode::Enter);

        let cards = account::list_by_kind(&app.db, Kind::Credit).unwrap();
        assert_eq!(cards.len(), before + 1);
        let added = cards.last().unwrap();
        assert_eq!(added.name, "Card Three");
        assert_eq!(added.group, Group::Credit);
    }

    /// A code the kind already holds is what the next import would match two
    /// rows against. The modal stays open with the message on it, rather than
    /// a `UNIQUE constraint failed` naming an index the owner never typed.
    #[test]
    fn a_refuses_a_code_the_kind_already_holds() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Cash).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "SAV");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Second Rainy Day");
        press(&mut app, KeyCode::Enter);

        assert!(app.status.contains("SAV"), "{}", app.status);
        assert!(app.modal.is_some(), "the form closed over a refused write");
        assert_eq!(
            account::list_by_kind(&app.db, Kind::Cash).unwrap().len(),
            before
        );
    }

    /// The refusal is per kind, because `UNIQUE (code, kind)` is: one code
    /// naming both a cash account and the card drawn on it is the shape the
    /// constraint exists for.
    #[test]
    fn a_accepts_a_code_that_only_the_other_kind_holds() {
        let mut app = app();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "SAV");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Rainy Day Card");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        assert_eq!(
            account::by_code(&app.db, "SAV", Kind::Credit)
                .unwrap()
                .unwrap()
                .name,
            "Rainy Day Card"
        );
    }

    /// A new account has to reach the screens that hold their own account
    /// list, the way a rename does: a card nobody can filter the Credit
    /// ledger to is a card that exists on one screen only.
    #[test]
    fn an_added_account_reaches_the_screens_that_cache_the_account_list() {
        let mut app = app();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "NST");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(
            app.accounts.rows().iter().any(|r| r.code == "NST"),
            "the Accounts screen does not show it"
        );
        assert!(
            app.cash.accounts().iter().any(|a| a.name == "Nest Egg"),
            "the cash ledger cannot be filtered to it"
        );
    }

    /// The policy decides how an interest posting is divided, so it is a
    /// property of the account and belongs with the rest of its row.
    #[test]
    fn the_interest_policy_is_written_from_the_account_form() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        assert_eq!(
            account::interest_policy(&app.db, id).unwrap(),
            account::InterestPolicy::Manual
        );

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('e'));
        while !matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Interest)
        {
            press(&mut app, KeyCode::Tab);
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        assert_ne!(
            account::interest_policy(&app.db, id).unwrap(),
            account::InterestPolicy::Manual
        );
        assert_eq!(
            app.accounts.rows()[0].policy,
            account::interest_policy(&app.db, id).unwrap()
        );
    }

    /// A card has no band to move between and no goals to divide interest
    /// among, so its form is two fields and neither selector is offered.
    /// The Color field end to end: the selector is cycled on the form, `Enter`
    /// writes it, and the row the Accounts screen redraws carries it. Nothing
    /// else on the row moves -- the same press must not rename or reband the
    /// account it recolors.
    #[test]
    fn e_on_the_accounts_screen_writes_the_color_it_was_left_on() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let before = app.accounts.selected().unwrap().clone();
        assert_eq!(
            account::get(&app.db, before.account.id()).unwrap().color,
            None,
            "an account starts with no color"
        );

        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Account(form)) = &mut app.modal else {
            panic!("e did not open the account form");
        };
        // The form opens on the shade the row is already drawn in, so one
        // step off it is the *next* color rather than the head of the list.
        let opening = account::AccountColor::derived(before.account.id());
        form.next_choice_on(accounts_screen::AccountField::Color);
        let picked = form
            .color_choice()
            .expect("one step off a color is a color");
        assert_ne!(picked, opening, "the step did not move");
        press(&mut app, KeyCode::Enter);

        let after = app
            .accounts
            .rows()
            .iter()
            .find(|r| r.account.id() == before.account.id())
            .expect("the account is gone");
        assert_eq!(after.account.text(), before.account.text());
        assert_eq!(after.group, before.group);
        assert_eq!(
            account::get(&app.db, before.account.id()).unwrap().color,
            Some(picked),
            "the color did not reach the database"
        );
    }

    /// The one cost of opening on the derived shade: Enter on an untouched
    /// form writes it down. Nothing on screen changes, because it is the
    /// shade the row was already drawn in -- what it gives up is the stored
    /// difference between "not chosen" and "chosen to be what it already
    /// was", which no screen shows.
    #[test]
    fn enter_on_an_untouched_account_form_pins_the_derived_color() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let id = app.accounts.selected().unwrap().account.id();
        assert_eq!(account::get(&app.db, id).unwrap().color, None);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            account::get(&app.db, id).unwrap().color,
            Some(account::AccountColor::derived(id))
        );
        // And the row is drawn in exactly the shade it was before.
        assert_eq!(
            crate::tui::style::account_color(id, Some(account::AccountColor::derived(id))),
            crate::tui::style::account_color(id, None)
        );
    }

    /// `—` is a choice the owner can take back, so cycling the whole way
    /// round and saving has to leave the account exactly as it was found.
    #[test]
    fn a_color_can_be_cleared_from_the_accounts_screen() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let id = app.accounts.selected().unwrap().account.id();
        account::set_color(&app.db, id, Some(account::AccountColor::Rose)).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Account(form)) = &mut app.modal else {
            panic!("e did not open the account form");
        };
        // Step round to `—` rather than counting to it: how long the cycle
        // is, is the form's business and not this test's.
        for _ in 0..=account::AccountColor::ALL.len() {
            if form
                .display(accounts_screen::AccountField::Color)
                .plain_text()
                == "—"
            {
                break;
            }
            form.next_choice_on(accounts_screen::AccountField::Color);
        }
        assert_eq!(
            form.display(accounts_screen::AccountField::Color)
                .plain_text(),
            "—"
        );
        press(&mut app, KeyCode::Enter);

        assert_eq!(account::get(&app.db, id).unwrap().color, None);
    }

    #[test]
    fn a_card_gets_a_shorter_account_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        while app.accounts.selected().unwrap().kind != Kind::Credit {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char('e'));

        let Some(Modal::Account(form)) = &app.modal else {
            panic!("e did not open the account form");
        };
        assert_eq!(
            form.fields(),
            vec![
                accounts_screen::AccountField::Name,
                // Color survives the trim: a card is named on the Credit
                // ledger and on Recurring Transactions, so it is tinted
                // there, and the choice belongs to every account.
                accounts_screen::AccountField::Color,
                accounts_screen::AccountField::Order
            ]
        );
    }

    /// The block mapping is what `mm import` waits on, and screen 9 is the
    /// only place it can be set. One selector per account, so an account
    /// cannot claim both blocks -- and moving it off a block clears that
    /// block's key rather than leaving it naming an account that no longer
    /// answers for it.
    #[test]
    fn the_account_form_points_a_savings_block_at_an_account_and_off_again() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        assert!(
            setting::get(&app.db, SavingsBlock::Goals.key())
                .unwrap()
                .is_none()
        );

        let pick_block = |app: &mut App, steps: usize| {
            press(app, KeyCode::Char('9'));
            press(app, KeyCode::Char('e'));
            while !matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Savings)
            {
                press(app, KeyCode::Tab);
            }
            for _ in 0..steps {
                press(app, KeyCode::Right);
            }
            press(app, KeyCode::Enter);
        };

        // One step off "neither" is the first block.
        pick_block(&mut app, 1);
        assert_eq!(
            setting::get(&app.db, SavingsBlock::Goals.key()).unwrap(),
            Some(id)
        );
        assert_eq!(
            app.accounts.rows()[0].block,
            Some(SavingsBlock::Goals),
            "the screen does not show what was written"
        );

        // A full cycle back to "neither" clears it again.
        pick_block(&mut app, SavingsBlock::ALL.len());
        assert!(
            setting::get(&app.db, SavingsBlock::Goals.key())
                .unwrap()
                .is_none(),
            "moving off a block left its key naming the account"
        );
        assert_eq!(app.accounts.rows()[0].block, None);
    }

    /// Editing one account must not disturb the other block's mapping: the
    /// key is only ever cleared when this account is the one it names.
    #[test]
    fn editing_one_account_leaves_the_other_blocks_container_alone() {
        let mut app = app();
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        assert!(cash.len() > 1, "the fixture needs two cash accounts");
        setting::set(&app.db, SavingsBlock::Buckets.key(), cash[1].id).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, SavingsBlock::Buckets.key()).unwrap(),
            Some(cash[1].id)
        );
    }
}
