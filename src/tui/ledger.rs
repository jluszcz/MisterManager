use super::cursor::{Cursor, Scroll};
use super::search::{Search, SearchBox};
use crate::db::AccountId;
use crate::db::account::{Account, Kind};
use crate::db::txn::{Filter, Txn};
use crate::money::Cents;
use chrono::{Datelike, Months, NaiveDate};
use std::collections::HashMap;

/// How many days at each end of a month widen the window to two months.
const FLANK: u32 = 7;

/// The months one ledger screen shows: a first-of-month date and a count.
///
/// The end date is derived rather than stored. Storing it would put
/// end-of-month arithmetic in the path of every `[` press, which is the
/// January 31 → February 31 bug waiting to happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    start: NaiveDate,
    months: u32,
}

impl Window {
    /// The window the ledger opens on, chosen by where `today` falls in its
    /// own month: the first seven days show the prior month too, the last
    /// seven show the next month too, and the rest of the month shows only
    /// itself.
    ///
    /// The seven days are counted from the month's own length, so February
    /// starts widening on the 22nd where August waits until the 25th.
    pub fn containing(today: NaiveDate) -> Window {
        let start = first_of_month(today);
        let month_length = end_of_month(start).day();
        if today.day() <= FLANK {
            Window {
                start: shift_months(start, -1),
                months: 2,
            }
        } else if today.day() > month_length - FLANK {
            Window { start, months: 2 }
        } else {
            Window { start, months: 1 }
        }
    }

    pub fn start(&self) -> NaiveDate {
        self.start
    }

    pub fn end(&self) -> NaiveDate {
        end_of_month(shift_months(self.start, self.months as i32 - 1))
    }

    /// How the window reads in the title bar: `Aug 2026`, `Jul–Aug 2026`, or
    /// `Dec 2026 – Jan 2027` when it straddles New Year, since `Dec–Jan 2027`
    /// would misdate the December half.
    pub fn label(&self) -> String {
        let (start, end) = (self.start, self.end());
        let month = |d: NaiveDate| d.format("%b").to_string();
        if start.year() != end.year() {
            format!(
                "{} {} – {} {}",
                month(start),
                start.year(),
                month(end),
                end.year()
            )
        } else if start.month() != end.month() {
            format!("{}–{} {}", month(start), month(end), end.year())
        } else {
            format!("{} {}", month(start), start.year())
        }
    }

    /// Slide by whole months, keeping the span. `[` and `]` are this.
    fn shifted(&self, months: i32) -> Window {
        Window {
            start: shift_months(self.start, months),
            months: self.months,
        }
    }
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("every month has a first day")
}

/// The last day of the month `first_of_month` opens.
fn end_of_month(first_of_month: NaiveDate) -> NaiveDate {
    shift_months(first_of_month, 1)
        .pred_opt()
        .expect("the day before a first-of-month is representable")
}

fn shift_months(date: NaiveDate, months: i32) -> NaiveDate {
    let shifted = if months >= 0 {
        date.checked_add_months(Months::new(months as u32))
    } else {
        date.checked_sub_months(Months::new(months.unsigned_abs()))
    };
    shifted.expect("ledger dates are nowhere near the ends of the calendar")
}

/// The Cash or Credit screen's view state: which slice of the ledger is
/// showing, and where the cursor sits in it.
///
/// One type for both screens, parameterized by `kind`. They differ in that
/// kind and in the sign convention of the amount column, and in nothing else.
///
/// Holds no `Db`: `App` asks for [`Ledger::filter`], runs the query, and
/// hands the rows back through [`Ledger::set_rows`]. That is what makes the
/// month stepping and the selection clamping testable without a terminal or a
/// database.
pub struct Ledger {
    kind: Kind,
    accounts: Vec<Account>,
    /// `None` is the All filter; `Some(i)` indexes `accounts`.
    account: Option<usize>,
    /// The oldest and newest dates the data covers, or `None` for an empty
    /// database. Bounds `[` and `]`.
    range: Option<(NaiveDate, NaiveDate)>,
    window: Window,
    search: SearchBox,
    rows: Vec<Txn>,
    /// The balance of what the account filter names, as of today — the same
    /// figure the Overview's To-Date column carries. Not derived from `rows`:
    /// see [`Ledger::set_total`].
    total: Cents,
    /// What each account's balance is being reconciled against, keyed by id
    /// rather than by the filter's position so a target survives the `Tab`
    /// cycle, the month steps and a search.
    ///
    /// Session state: nothing here reaches the database. The figure is a
    /// statement read off a bank's screen while rows are being entered, and
    /// it stops meaning anything once the entering is done.
    targets: HashMap<AccountId, Cents>,
    cursor: Cursor,
}

impl Ledger {
    pub fn new(
        kind: Kind,
        accounts: Vec<Account>,
        range: Option<(NaiveDate, NaiveDate)>,
        today: NaiveDate,
    ) -> Ledger {
        Ledger {
            kind,
            accounts,
            account: None,
            range,
            window: Window::containing(today),
            search: SearchBox::new(),
            rows: Vec::new(),
            total: Cents::ZERO,
            targets: HashMap::new(),
            cursor: Cursor::new(),
        }
    }

    /// Take a refreshed account list, keeping the filter on the same account.
    ///
    /// The filter is a *position* in this list, and the Accounts screen can
    /// rename, re-band and reorder -- so carrying the index across would
    /// silently point it at a different account. What the filter means is an
    /// id, so that is what is carried; an account that has gone falls back to
    /// All, which is the filter that needs no account to exist.
    pub fn set_accounts(&mut self, accounts: Vec<Account>) {
        let selected = self.selected_account();
        self.accounts = accounts;
        self.account = selected.and_then(|id| self.accounts.iter().position(|a| a.id == id));
    }

    /// Take the data's date range, refreshed after every write.
    ///
    /// The window is left where it is: a write can only widen the range, and
    /// re-clamping would move the screen out from under a cursor that is
    /// already where the user put it.
    pub fn set_date_range(&mut self, range: Option<(NaiveDate, NaiveDate)>) {
        self.range = range;
    }

    /// A window past either end of the data shows an empty screen, which
    /// reads as lost data rather than as a filter out of range.
    ///
    /// Only the start is clamped, so the opening window may reach past the
    /// newest row into a month that has nothing in it yet — that is the month
    /// about to be pre-entered. An empty database has no range to clamp to.
    fn clamped(&self, window: Window) -> Window {
        let Some((oldest, newest)) = self.range else {
            return window;
        };
        Window {
            start: window
                .start
                .clamp(first_of_month(oldest), first_of_month(newest)),
            months: window.months,
        }
    }

    pub fn window(&self) -> Window {
        self.window
    }

    /// Move the window bodily, clamped the same way `[` and `]` are.
    ///
    /// `App` uses this to hold the Cash and Credit ledgers on one shared
    /// month. Stepping is still [`Ledger::next_month`] and
    /// [`Ledger::previous_month`]; this only copies the result across.
    pub fn set_window(&mut self, window: Window) {
        self.window = self.clamped(window);
    }

    pub fn next_month(&mut self) {
        self.window = self.clamped(self.window.shifted(1));
    }

    pub fn previous_month(&mut self) {
        self.window = self.clamped(self.window.shifted(-1));
    }

    /// `Tab`: All → each account of this kind → All. The per-card Credit view
    /// the parent design asks for is this filter, not a screen per card.
    pub fn next_account(&mut self) {
        self.account = match self.account {
            None if self.accounts.is_empty() => None,
            None => Some(0),
            Some(i) if i + 1 < self.accounts.len() => Some(i + 1),
            Some(_) => None,
        };
    }

    /// `BackTab`: the same cycle the other way — All → the last account of
    /// this kind → its predecessor → All.
    pub fn previous_account(&mut self) {
        self.account = match self.account {
            None => self.accounts.len().checked_sub(1),
            Some(0) => None,
            Some(i) => Some(i - 1),
        };
    }

    /// The account the `Tab` filter is narrowed to, or `None` for All.
    ///
    /// `a` opens its form on this account: adding a row while looking at one
    /// card and having the form default to a different one is a misfiled row
    /// in a ledger with no undo.
    pub fn selected_account(&self) -> Option<AccountId> {
        self.account.map(|i| self.accounts[i].id)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn filter(&self) -> Filter {
        Filter {
            kind: self.kind,
            account_id: self.selected_account(),
            from: self.window.start(),
            to: self.window.end(),
            search: if self.search().is_empty() {
                None
            } else {
                Some(self.search().to_string())
            },
        }
    }

    /// Take the rows the filter matched, keeping the cursor inside them.
    pub fn set_rows(&mut self, rows: Vec<Txn>) {
        self.rows = rows;
        self.cursor.clamp(self.rows.len());
    }

    pub fn rows(&self) -> &[Txn] {
        &self.rows
    }

    /// Take the balance of whatever the `Tab` filter names, as of today.
    ///
    /// Queried by `App` beside the rows, for the reason [`Ledger::set_rows`]
    /// is: this type holds no `Db`. Deliberately *not* a sum over `rows` —
    /// the window and the search bound the rows, and this is a balance at a
    /// date, so a two-month window and a search still quote today's figure.
    pub fn set_total(&mut self, total: Cents) {
        self.total = total;
    }

    pub fn total(&self) -> Cents {
        self.total
    }

    /// What the filtered account's balance is being reconciled against, or
    /// `None` when it has no target and under the All filter -- where the
    /// balance is the whole kind's and no statement quotes it.
    pub fn target(&self) -> Option<Cents> {
        let id = self.selected_account()?;
        self.targets.get(&id).copied()
    }

    /// Set or clear the filtered account's target. Under All there is no
    /// account to hang one on, so this does nothing.
    pub fn set_target(&mut self, target: Option<Cents>) {
        let Some(id) = self.selected_account() else {
            return;
        };
        match target {
            Some(cents) => {
                self.targets.insert(id, cents);
            }
            None => {
                self.targets.remove(&id);
            }
        }
    }

    /// `Today − Target`: what the ledger holds, less what it should hold.
    ///
    /// Signed the same way on both screens. The Credit ledger stores debt as
    /// positive and renders as stored, so a delta that negated there would be
    /// the one figure on the screen disagreeing with the rest of it.
    pub fn delta(&self) -> Option<Cents> {
        Some(self.total - self.target()?)
    }

    pub fn selected(&self) -> Option<&Txn> {
        self.rows.get(self.cursor.index())
    }

    /// Put the cursor on the last row dated on or before `today` — the rows
    /// just entered are the ones worth landing on, and the dim future rows
    /// end up directly below the cursor.
    ///
    /// Deliberately not part of [`Ledger::set_rows`]: rows are re-queried
    /// after every write, and re-anchoring there would send the cursor home
    /// on every edit and delete. `App` calls this only when the window or the
    /// account filter changed the row set out from under it.
    pub fn select_at_or_before(&mut self, today: NaiveDate) {
        self.cursor
            .select(self.rows.iter().rposition(|t| t.date <= today).unwrap_or(0));
    }

    pub fn account_name(&self, id: AccountId) -> &str {
        super::account_name(&self.accounts, id)
    }

    pub fn title(&self) -> String {
        let kind = match self.kind {
            Kind::Cash => "Cash",
            Kind::Credit => "Credit",
        };
        let account = match self.account {
            None => "All",
            Some(i) => self.accounts[i].code.as_str(),
        };
        let mut title = format!("{kind} · {} · {account}", self.window.label());
        if !self.search().is_empty() {
            title.push_str(&format!(" · /{}", self.search()));
        }
        title
    }
}

/// The rows come from SQL — [`Ledger::filter`] carries the needle into the
/// query — so there is nothing in memory to re-filter. `App` re-queries after
/// the key is consumed.
impl Search for Ledger {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }
}

impl Scroll for Ledger {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    fn row_count(&self) -> usize {
        self.rows.len()
    }
}

use super::{account_cell, amount, money_span, money_text, right_header, table_state};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Cell, Row, Table};

/// The filter chain, and then the balance that chain names.
///
/// A `Line` of spans rather than the one string [`Ledger::title`] returns, so
/// the figure carries [`super::style::amount_color`] the way every money cell
/// does. The figure goes last and the filter terms stay contiguous, so it sits
/// in the same place whether or not a search is running.
///
/// `Today` is not decoration: `Aug 2026` is two terms to its left, and without
/// the word the figure reads as a total of the month on show rather than a
/// balance at a date.
fn title_line(ledger: &Ledger) -> TextLine<'static> {
    let mut spans = vec![
        Span::raw(format!("{} · Today ", ledger.title())),
        money_span(ledger.total()),
    ];
    if let (Some(target), Some(delta)) = (ledger.target(), ledger.delta()) {
        spans.push(Span::raw(" · Target "));
        spans.push(money_span(target));
        spans.push(Span::raw(" · Δ "));
        spans.push(delta_span(delta));
    }
    TextLine::from(spans)
}

/// The delta, green above the target and red below it, and a bare `-` when
/// there is none.
///
/// Not [`super::money_span`]: that colors a figure by its sign, which leaves
/// a surplus in the same no-color as every other positive number on screen.
/// Here the sign is the answer, so both directions are worth a color.
///
/// The dash is a state to see rather than a figure to read -- `$0.00` says
/// "reconciled" only to whoever stops to compare it with the two figures to
/// its left.
fn delta_span(delta: Cents) -> Span<'static> {
    match super::style::delta_color(delta) {
        Some(color) => Span::styled(money_text(delta), Style::default().fg(color)),
        None => Span::raw("-"),
    }
}

/// Date, Account, Description, Amount.
///
/// Amounts render **as stored**: cash signed naturally, credit signed as
/// debt, so each screen matches its sheet. Only Overview negates.
///
/// Rows dated after today render dim — the projection model made visible.
///
/// Returns the viewport height in rows, which `App` records on the `Ledger`
/// for `PageUp` and `PageDown` to move by.
pub fn render(frame: &mut Frame, area: Rect, ledger: &Ledger, today: NaiveDate) -> usize {
    let future = Style::default().add_modifier(Modifier::DIM);
    let rows: Vec<Row> = ledger
        .rows()
        .iter()
        .map(|t| {
            Row::new(vec![
                Cell::from(t.date.to_string()),
                account_cell(ledger.account_name(t.account_id).to_string(), t.account_id),
                Cell::from(t.description.clone()),
                amount(t.cents),
            ])
            .style(if t.date > today {
                future
            } else {
                Style::default()
            })
        })
        .collect();

    // `Amount` is right-aligned to sit over its own figures; the other three
    // columns are left-aligned like their cells.
    let header = Row::new(vec![
        Cell::from("Date"),
        Cell::from("Account"),
        Cell::from("Description"),
        right_header("Amount"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    // `Account` holds the longest account name (`Brokerage`) and a gap. It
    // is paid for out of `Description`, which absorbs whatever slack is left.
    let widths = [
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Min(20),
        Constraint::Length(14),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ")
        .block(Block::bordered().title(title_line(ledger)));

    // Two borders and the header row are not available to data rows.
    let height = usize::from(area.height).saturating_sub(3);

    // The selection lives in `Ledger`, which holds no ratatui state; the
    // widget's scroll offset is derived from it each frame.
    let mut state = table_state(ledger.selected_index(), ledger.rows().len(), height);
    frame.render_stateful_widget(table, area, &mut state);
    height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::Group;
    use crate::db::{RecurringTxnId, TxnId};
    use crate::money::Cents;
    use crate::tui::MIN_WIDTH;
    use chrono::NaiveDate;

    fn accounts() -> Vec<Account> {
        vec![
            Account {
                id: AccountId(1),
                code: "CHK".to_string(),
                name: "Everyday".to_string(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
            },
            Account {
                id: AccountId(2),
                code: "BKR".to_string(),
                name: "Brokerage".to_string(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
            },
        ]
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test dates are valid")
    }

    fn row(n: i64) -> Txn {
        dated_row(n, day(2026, 8, 1))
    }

    fn dated_row(n: i64, date: NaiveDate) -> Txn {
        Txn {
            id: TxnId(n),
            date,
            cents: Cents(100),
            account_id: AccountId(1),
            description: format!("row {n}"),
            recurring_txn_id: None::<RecurringTxnId>,
            edited: false,
        }
    }

    /// A ledger over an empty database: no date range, so `[` and `]` are
    /// unclamped unless a test sets one.
    fn ledger(today: NaiveDate) -> Ledger {
        Ledger::new(Kind::Cash, accounts(), None, today)
    }

    /// The **column** a substring starts at in a rendered row.
    ///
    /// `str::find` answers in bytes, and a drawn title is full of multi-byte
    /// symbols — the `┌` border is three, each `·` separator is two — so a byte
    /// offset used as a column silently drifts ahead of the cell it names. It
    /// drifts *forward*, so it lands past the intended cell and often still
    /// inside the span being checked, which is what makes the mistake pass.
    fn column_of(row: &str, needle: &str) -> u16 {
        let byte = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is not in {row:?}"));
        u16::try_from(row[..byte].chars().count()).expect("a row is at most MIN_WIDTH cells")
    }

    /// The window is stated as its inclusive bounds, which is what the filter
    /// carries and what the reader of a failing test wants to see.
    fn bounds(window: Window) -> (NaiveDate, NaiveDate) {
        (window.start(), window.end())
    }

    #[test]
    fn the_first_seven_days_of_a_month_show_the_prior_month_too() {
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 1))),
            (day(2026, 7, 1), day(2026, 8, 31))
        );
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 7))),
            (day(2026, 7, 1), day(2026, 8, 31))
        );
    }

    #[test]
    fn the_eighth_is_the_first_day_showing_only_the_current_month() {
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 8))),
            (day(2026, 8, 1), day(2026, 8, 31))
        );
    }

    /// August has 31 days, so its last seven are the 25th through the 31st.
    #[test]
    fn the_last_seven_days_of_a_month_show_the_next_month_too() {
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 24))),
            (day(2026, 8, 1), day(2026, 8, 31)),
            "the 24th is the eighth-from-last day, so still current-only"
        );
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 25))),
            (day(2026, 8, 1), day(2026, 9, 30))
        );
        assert_eq!(
            bounds(Window::containing(day(2026, 8, 31))),
            (day(2026, 8, 1), day(2026, 9, 30))
        );
    }

    /// The seven days are counted from the month's own length, so a short
    /// month's widening starts earlier in the numbering.
    #[test]
    fn a_short_month_counts_its_last_seven_days_from_its_own_length() {
        assert_eq!(
            bounds(Window::containing(day(2026, 2, 21))),
            (day(2026, 2, 1), day(2026, 2, 28))
        );
        assert_eq!(
            bounds(Window::containing(day(2026, 2, 22))),
            (day(2026, 2, 1), day(2026, 3, 31))
        );
    }

    #[test]
    fn a_leap_february_counts_its_last_seven_days_from_twenty_nine() {
        assert_eq!(
            bounds(Window::containing(day(2028, 2, 22))),
            (day(2028, 2, 1), day(2028, 2, 29))
        );
        assert_eq!(
            bounds(Window::containing(day(2028, 2, 23))),
            (day(2028, 2, 1), day(2028, 3, 31))
        );
    }

    #[test]
    fn a_window_reaching_back_from_january_reaches_into_the_prior_year() {
        assert_eq!(
            bounds(Window::containing(day(2026, 1, 3))),
            (day(2025, 12, 1), day(2026, 1, 31))
        );
    }

    #[test]
    fn a_window_reaching_forward_from_december_reaches_into_the_next_year() {
        assert_eq!(
            bounds(Window::containing(day(2026, 12, 28))),
            (day(2026, 12, 1), day(2027, 1, 31))
        );
    }

    #[test]
    fn sliding_a_two_month_window_keeps_it_two_months_wide() {
        let mut ledger = ledger(day(2026, 8, 3));
        assert_eq!(bounds(ledger.window()), (day(2026, 7, 1), day(2026, 8, 31)));

        ledger.next_month();
        assert_eq!(bounds(ledger.window()), (day(2026, 8, 1), day(2026, 9, 30)));
    }

    #[test]
    fn sliding_lands_on_the_last_day_of_a_shorter_month() {
        let mut ledger = ledger(day(2026, 1, 15));
        ledger.next_month();
        assert_eq!(
            bounds(ledger.window()),
            (day(2026, 2, 1), day(2026, 2, 28)),
            "January 31 must not slide to a February 31"
        );
    }

    /// Sliding past the data shows an empty screen, which reads as lost data
    /// rather than as a filter out of range.
    #[test]
    fn sliding_past_either_end_of_the_data_stays_put() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_date_range(Some((day(2026, 7, 4), day(2026, 9, 2))));

        ledger.next_month();
        ledger.next_month();
        assert_eq!(ledger.window().start(), day(2026, 9, 1));

        ledger.previous_month();
        ledger.previous_month();
        ledger.previous_month();
        assert_eq!(ledger.window().start(), day(2026, 7, 1));
    }

    /// Only the start is clamped: the opening window may legitimately reach
    /// into a month that has nothing in it yet, which is the month about to
    /// be pre-entered.
    #[test]
    fn the_opening_window_may_reach_past_the_newest_row() {
        let mut ledger = ledger(day(2026, 8, 28));
        ledger.set_date_range(Some((day(2026, 1, 1), day(2026, 8, 31))));

        assert_eq!(bounds(ledger.window()), (day(2026, 8, 1), day(2026, 9, 30)));
    }

    /// An empty database has no range to clamp against. Sliding still works
    /// rather than pinning the window where it opened.
    #[test]
    fn sliding_an_empty_ledger_is_unclamped() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_month();
        assert_eq!(bounds(ledger.window()), (day(2026, 9, 1), day(2026, 9, 30)));

        ledger.previous_month();
        ledger.previous_month();
        assert_eq!(bounds(ledger.window()), (day(2026, 7, 1), day(2026, 7, 31)));
    }

    #[test]
    fn the_account_filter_cycles_through_each_account_and_back_to_all() {
        let mut ledger = ledger(day(2026, 8, 15));
        assert_eq!(ledger.filter().account_id, None);
        ledger.next_account();
        assert_eq!(ledger.filter().account_id, Some(AccountId(1)));
        ledger.next_account();
        assert_eq!(ledger.filter().account_id, Some(AccountId(2)));
        ledger.next_account();
        assert_eq!(ledger.filter().account_id, None, "Tab must return to All");
    }

    #[test]
    fn back_tab_cycles_the_account_filter_the_other_way() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.previous_account();
        assert_eq!(ledger.filter().account_id, Some(AccountId(2)));
        ledger.previous_account();
        assert_eq!(ledger.filter().account_id, Some(AccountId(1)));
        ledger.previous_account();
        assert_eq!(
            ledger.filter().account_id,
            None,
            "BackTab must return to All"
        );
    }

    /// The two directions are one cycle, not two: whatever `Tab` reached,
    /// `BackTab` undoes.
    #[test]
    fn back_tab_undoes_a_tab_from_anywhere_in_the_account_cycle() {
        let mut ledger = ledger(day(2026, 8, 15));
        for _ in 0..3 {
            let before = ledger.filter().account_id;
            ledger.next_account();
            ledger.previous_account();
            assert_eq!(ledger.filter().account_id, before);
            ledger.next_account();
        }
    }

    #[test]
    fn the_filter_carries_the_kind_window_and_search() {
        let mut ledger = Ledger::new(Kind::Credit, Vec::new(), None, day(2026, 8, 15));
        ledger.begin_search();
        for c in "Foods".chars() {
            ledger.push_search(c);
        }
        ledger.backspace_search();

        let filter = ledger.filter();
        assert_eq!(filter.kind, Kind::Credit);
        assert_eq!(filter.from, day(2026, 8, 1));
        assert_eq!(filter.to, day(2026, 8, 31));
        assert_eq!(filter.search.as_deref(), Some("Food"));
    }

    #[test]
    fn an_empty_search_box_filters_nothing() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.begin_search();
        assert_eq!(ledger.filter().search, None);
        ledger.push_search('x');
        ledger.backspace_search();
        assert_eq!(ledger.filter().search, None);
    }

    /// Typing into the search box shrinks the list under the cursor. A
    /// selection left past the end would make `e` and `d` operate on nothing
    /// — or, worse, on whatever later lands at that index.
    #[test]
    fn a_shrinking_filter_moves_the_selection_into_bounds() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=5).map(row).collect());
        for _ in 0..4 {
            ledger.select_next();
        }
        assert_eq!(ledger.selected_index(), 4);

        ledger.set_rows((1..=2).map(row).collect());
        assert_eq!(ledger.selected_index(), 1);
        assert_eq!(ledger.selected().unwrap().id, TxnId(2));
    }

    #[test]
    fn an_empty_result_has_nothing_selected() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=3).map(row).collect());
        ledger.select_next();
        ledger.set_rows(Vec::new());

        assert_eq!(ledger.selected_index(), 0);
        assert!(ledger.selected().is_none());
    }

    #[test]
    fn the_selection_stops_at_both_ends_of_the_list() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=2).map(row).collect());
        ledger.select_previous();
        assert_eq!(ledger.selected_index(), 0);
        ledger.select_next();
        ledger.select_next();
        ledger.select_next();
        assert_eq!(ledger.selected_index(), 1);
    }

    /// A window slid entirely into the past has no row after today, so its
    /// last row is the one to open on.
    #[test]
    fn the_cursor_opens_on_the_last_row_dated_on_or_before_today() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows(vec![
            dated_row(1, day(2026, 8, 1)),
            dated_row(2, day(2026, 8, 15)),
            dated_row(3, day(2026, 8, 15)),
            dated_row(4, day(2026, 8, 20)),
        ]);

        ledger.select_at_or_before(day(2026, 8, 15));
        assert_eq!(ledger.selected().unwrap().id, TxnId(3));
    }

    #[test]
    fn a_window_entirely_in_the_future_opens_on_its_first_row() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows(vec![
            dated_row(1, day(2026, 9, 4)),
            dated_row(2, day(2026, 9, 8)),
        ]);

        ledger.select_at_or_before(day(2026, 8, 15));
        assert_eq!(ledger.selected_index(), 0);
    }

    #[test]
    fn a_window_entirely_in_the_past_opens_on_its_last_row() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows(vec![
            dated_row(1, day(2026, 6, 4)),
            dated_row(2, day(2026, 6, 8)),
        ]);

        ledger.select_at_or_before(day(2026, 8, 15));
        assert_eq!(ledger.selected_index(), 1);
    }

    /// Re-querying after an edit or a delete must not send the cursor home,
    /// which is why anchoring is its own call rather than part of `set_rows`.
    #[test]
    fn reloading_rows_leaves_the_cursor_where_it_was() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=5).map(row).collect());
        ledger.select_next();
        ledger.select_next();

        ledger.set_rows((1..=5).map(row).collect());
        assert_eq!(ledger.selected_index(), 2);
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_row() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=50).map(row).collect());

        ledger.select_last();
        assert_eq!(ledger.selected_index(), 49);
        ledger.select_first();
        assert_eq!(ledger.selected_index(), 0);
    }

    #[test]
    fn paging_moves_the_cursor_by_one_viewport() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=50).map(row).collect());
        ledger.set_page_height(10);

        ledger.page_down();
        assert_eq!(ledger.selected_index(), 10);
        ledger.page_down();
        assert_eq!(ledger.selected_index(), 20);
        ledger.page_up();
        assert_eq!(ledger.selected_index(), 10);
    }

    #[test]
    fn paging_past_either_end_stops_on_the_last_row() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_rows((1..=15).map(row).collect());
        ledger.set_page_height(10);

        ledger.page_down();
        ledger.page_down();
        assert_eq!(ledger.selected_index(), 14);
        ledger.page_up();
        ledger.page_up();
        assert_eq!(ledger.selected_index(), 0);
    }

    #[test]
    fn paging_an_empty_list_selects_nothing() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.set_page_height(10);

        ledger.page_down();
        ledger.select_last();
        assert_eq!(ledger.selected_index(), 0);
        assert!(ledger.selected().is_none());
    }

    /// The column shows the account's name, not its code, and is wide enough
    /// for the longest of them at `MIN_WIDTH`. A truncated `Brokerage` would
    /// read as an account that does not exist.
    #[test]
    fn the_account_column_names_the_account_in_full_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_rows(vec![Txn {
            account_id: AccountId(2),
            ..dated_row(1, today)
        }]);

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Brokerage"), "{text}");
        assert!(
            text.contains("Account"),
            "the header names the column: {text}"
        );
    }

    /// A right-aligned column wants a right-aligned header over it; left over
    /// right put `Amount` at the far side of the column from its own figures.
    #[test]
    fn the_amount_header_sits_over_its_own_column() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_rows(vec![dated_row(1, today)]);

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        // Row 1 is the header. Both the word and the figure below it must end
        // one cell inside the right border.
        let buffer = terminal.backend().buffer();
        let last = MIN_WIDTH - 2;
        assert_eq!(buffer[(last, 1)].symbol(), "t", "end of `Amount`");
        assert_eq!(buffer[(last + 1 - "Amount".len() as u16, 1)].symbol(), "A");
        assert_eq!(buffer[(last, 2)].symbol(), "0", "end of the figure");
    }

    /// The amount cell sets a foreground and nothing else, so the DIM the row
    /// carries for a future-dated transaction survives underneath it. Setting
    /// a whole `Style` on the cell instead would silently un-dim exactly the
    /// rows the projection model exists to mark.
    #[test]
    fn a_negative_amount_reads_red_without_losing_a_future_rows_dim() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_rows(vec![Txn {
            cents: Cents(-4_200),
            date: day(2026, 8, 20),
            ..dated_row(1, day(2026, 8, 20))
        }]);

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        // Right-aligned in the last column, so the figure ends one cell inside
        // the right border. The first data row sits below the border and the
        // header.
        let buffer = terminal.backend().buffer();
        let cell = &buffer[(MIN_WIDTH - 2, 2)];
        assert_eq!(cell.symbol(), "0", "expected the end of -42.00: {cell:?}");
        assert_eq!(cell.fg, super::super::style::NEGATIVE);
        assert!(cell.modifier.contains(Modifier::DIM), "{cell:?}");
    }

    #[test]
    fn the_title_names_the_kind_the_window_and_the_account_filter() {
        let mut ledger = ledger(day(2026, 8, 15));
        assert_eq!(ledger.title(), "Cash · Aug 2026 · All");
        ledger.next_account();
        assert_eq!(ledger.title(), "Cash · Aug 2026 · CHK");
        ledger.begin_search();
        ledger.push_search('F');
        assert_eq!(ledger.title(), "Cash · Aug 2026 · CHK · /F");
    }

    /// The target is the figure the owner is reconciling against, so it
    /// belongs to the account and not to the filter position: stepping away
    /// to another account and back must find it still there.
    #[test]
    fn a_target_belongs_to_its_account_across_the_filter_cycle() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_account();
        ledger.set_target(Some(Cents(120_000)));
        ledger.next_account();
        assert_eq!(
            ledger.target(),
            None,
            "the next account has no target of its own"
        );
        ledger.next_account();
        ledger.previous_account();
        ledger.previous_account();
        assert_eq!(ledger.target(), Some(Cents(120_000)));
    }

    /// Under All the balance in the border is the whole kind's, which is not
    /// a figure any statement quotes. So there is nothing to reconcile it
    /// against, and a target set on an account does not leak into it.
    #[test]
    fn the_all_filter_has_no_target_of_its_own() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_account();
        ledger.set_target(Some(Cents(120_000)));
        ledger.previous_account();
        assert_eq!(ledger.target(), None);
        // And nothing is written when there is no account to write it to.
        ledger.set_target(Some(Cents(999)));
        assert_eq!(ledger.target(), None);
        ledger.next_account();
        assert_eq!(ledger.target(), Some(Cents(120_000)));
    }

    /// `Today − Target`: what the ledger holds, less what it should hold.
    /// Above the target is positive on both ledgers -- the Credit screen
    /// renders as stored the way every one of its other figures does.
    #[test]
    fn the_delta_is_the_balance_less_the_target() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_account();
        ledger.set_total(Cents(124_000));
        ledger.set_target(Some(Cents(120_000)));
        assert_eq!(ledger.delta(), Some(Cents(4_000)));
        ledger.set_total(Cents(116_000));
        assert_eq!(ledger.delta(), Some(Cents(-4_000)));
    }

    /// A reconciled account is the point of the exercise, and zero is a
    /// perfectly good answer -- not the absence of one.
    #[test]
    fn a_balance_matching_its_target_has_a_zero_delta_not_a_missing_one() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_account();
        ledger.set_total(Cents(120_000));
        ledger.set_target(Some(Cents(120_000)));
        assert_eq!(ledger.delta(), Some(Cents::ZERO));
    }

    /// No target, no delta: the border goes back to the balance alone.
    #[test]
    fn clearing_a_target_leaves_the_account_with_none() {
        let mut ledger = ledger(day(2026, 8, 15));
        ledger.next_account();
        ledger.set_target(Some(Cents(120_000)));
        ledger.set_target(None);
        assert_eq!(ledger.target(), None);
        assert_eq!(ledger.delta(), None);
    }

    /// The total is a balance *at today*, not a sum of the rows in the
    /// window: the future-dated row in view is outside it, and so is
    /// everything the month filter left off screen. `Today` in the title is
    /// what says so, with `Aug 2026` two terms to its left.
    #[test]
    fn the_title_carries_the_balance_to_date_after_the_filter_chain() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_rows(vec![dated_row(1, today)]);
        ledger.set_total(Cents(4_200_000));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("Cash · Aug 2026 · All · Today $42,000.00"),
            "{text}"
        );
    }

    /// The figure carries its own color, which is why the title is composed of
    /// spans rather than being the one string `title()` returns.
    #[test]
    fn a_negative_total_reads_red_in_the_title() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_total(Cents(-4_200));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        // The title runs along the top border, so its row is 0.
        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        let figure = column_of(&row, "-$42.00");
        assert_eq!(buffer[(figure, 0)].symbol(), "-", "{row}");
        assert_eq!(
            buffer[(figure, 0)].fg,
            super::super::style::NEGATIVE,
            "{row}"
        );

        let chain = column_of(&row, "Cash");
        assert_eq!(buffer[(chain, 0)].symbol(), "C", "{row}");
        assert_ne!(
            buffer[(chain, 0)].fg,
            super::super::style::NEGATIVE,
            "only the figure is colored: {row}"
        );
    }

    /// The minus goes outside the `$`, the way a ledger writes one. `$-42.00`
    /// is the other reading, and it is the one nothing else prints.
    #[test]
    fn a_negative_total_carries_its_sign_outside_the_dollar_sign() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.set_total(Cents(-4_200));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("Today -$42.00"), "{row}");
    }

    /// The filter terms stay contiguous and the figure stays last, so it is
    /// always in the same place whether or not a search is running.
    #[test]
    fn a_search_leaves_the_total_last_in_the_title() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.begin_search();
        ledger.push_search('F');
        ledger.set_total(Cents(95_000));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("Cash · Aug 2026 · CHK · /F · Today $950.00"),
            "{text}"
        );
    }

    /// The target and the delta go after the balance, in that order: the
    /// question the border answers left to right is "what do I have, what
    /// should I have, how far off am I".
    #[test]
    fn a_target_follows_the_balance_with_the_delta_after_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.set_total(Cents(116_000));
        ledger.set_target(Some(Cents(120_000)));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("Today $1,160.00 · Target $1,200.00 · Δ -$40.00"),
            "{text}"
        );
    }

    /// The delta is the one green figure in the app, and it is green on its
    /// own terms rather than on `amount_color`'s -- which would leave a
    /// surplus in the same no-color as every other positive figure.
    #[test]
    fn a_balance_above_its_target_reads_green() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.set_total(Cents(124_000));
        ledger.set_target(Some(Cents(120_000)));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        let figure = column_of(&row, "$40.00");
        assert_eq!(
            buffer[(figure, 0)].fg,
            super::super::style::POSITIVE,
            "{row}"
        );

        let target = column_of(&row, "$1,200.00");
        assert_ne!(
            buffer[(target, 0)].fg,
            super::super::style::POSITIVE,
            "only the delta is green: {row}"
        );
    }

    /// A balance below its target is money the ledger has not been told
    /// about, which is the same red every shortfall on screen wears.
    #[test]
    fn a_balance_below_its_target_reads_red() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.set_total(Cents(116_000));
        ledger.set_target(Some(Cents(120_000)));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        let figure = column_of(&row, "-$40.00");
        assert_eq!(
            buffer[(figure, 0)].fg,
            super::super::style::NEGATIVE,
            "{row}"
        );
    }

    /// `$0.00` is a figure to read; a dash is a state to see at a glance, and
    /// seeing it is the whole point of typing the target.
    #[test]
    fn a_reconciled_balance_draws_its_delta_as_a_dash() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.set_total(Cents(120_000));
        ledger.set_target(Some(Cents(120_000)));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("Target $1,200.00 · Δ -"), "{row}");
        assert!(!row.contains("Δ -$"), "not a figure: {row}");
        assert!(!row.contains("$0.00"), "not a figure: {row}");
    }

    /// An account with no target is every account until one is typed, so the
    /// border it draws is the one the screen has always drawn.
    #[test]
    fn a_ledger_with_no_target_keeps_the_border_it_had() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 8, 15);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.set_total(Cents(116_000));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("· CHK · Today $1,160.00"), "{row}");
        assert!(!row.contains("Target"), "{row}");
        assert!(!row.contains("Δ"), "{row}");
    }

    /// A title too long for the border is truncated, and the figure is at the
    /// end of it — so the digits that go missing are the whole total, silently.
    #[test]
    fn the_widest_plausible_title_fits_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 12, 28);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.next_account();
        ledger.begin_search();
        for c in "reimbursement".chars() {
            ledger.push_search(c);
        }
        ledger.set_total(Cents(-123_456_789));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("-$1,234,567.89"), "truncated: {row}");
    }

    /// The same title with a target on it, which is the longest the border
    /// ever gets: three figures and two more terms after the filter chain.
    /// The delta is last now, so it is the digits that go missing first.
    #[test]
    fn the_widest_plausible_title_with_a_target_fits_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let today = day(2026, 12, 28);
        let mut ledger = ledger(today);
        ledger.next_account();
        ledger.next_account();
        ledger.begin_search();
        for c in "reimbursement".chars() {
            ledger.push_search(c);
        }
        ledger.set_total(Cents(-123_456_789));
        ledger.set_target(Some(Cents(123_456_789)));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &ledger, today);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(
            row.contains("Today -$1,234,567.89 · Target $1,234,567.89 · Δ -$2,469,135.78"),
            "truncated: {row}"
        );
    }

    #[test]
    fn a_two_month_window_names_both_months() {
        assert_eq!(ledger(day(2026, 8, 3)).title(), "Cash · Jul–Aug 2026 · All");
    }

    /// A window straddling New Year names the year on both sides, since
    /// "Dec–Jan 2027" would misdate the December half.
    #[test]
    fn a_window_straddling_new_year_names_both_years() {
        assert_eq!(
            ledger(day(2026, 12, 28)).title(),
            "Cash · Dec 2026 – Jan 2027 · All"
        );
    }
}
