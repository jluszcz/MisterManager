use super::cursor::{Cursor, Scroll, Viewport};
use super::month::{MonthCycle, YearMonth};
use super::search::{Search, SearchBox};
use crate::db::account::Account;
use crate::db::{AccountId, GoalId};
use crate::goal::Funding;
use crate::money::Cents;
use crate::rate::Percent;
use crate::savings::{Row, is_reconciled};
use anyhow::Result;
use chrono::NaiveDate;

/// The Savings screen's view state: one flat list of every open goal, which
/// container it is filtered to, and where the cursor sits.
///
/// Holds no `Db` and no ratatui, exactly as `Ledger` does not: `App` runs the
/// queries and hands the results in.
pub struct Savings {
    accounts: Vec<Account>,
    containers: Vec<AccountId>,
    /// `None` is the All filter. Deliberately an id and not an index into
    /// `containers`, which `App` re-queries on every reload.
    container: Option<AccountId>,
    excess: Vec<(AccountId, Cents)>,
    all: Vec<Row>,
    /// The months `[` and `]` cycle through: every month the dated goals
    /// span, rebuilt whenever the goals are.
    month: MonthCycle<YearMonth>,
    /// Indices into `all` that survive the container filter, the month, and
    /// the search.
    visible: Vec<usize>,
    search: SearchBox,
    cursor: Cursor,
    today: NaiveDate,
    period_days: i64,
}

impl Savings {
    pub fn new(accounts: Vec<Account>, today: NaiveDate, period_days: i64) -> Savings {
        Savings {
            accounts,
            containers: Vec::new(),
            container: None,
            excess: Vec::new(),
            all: Vec::new(),
            month: MonthCycle::new(Vec::new(), YearMonth::of(today)),
            visible: Vec::new(),
            search: SearchBox::new(),
            cursor: Cursor::new(),
            today,
            period_days,
        }
    }

    /// Take every open goal, in `goal::all_with_balances` order, and build the
    /// derived columns once.
    ///
    /// Every column that asks "how far along is this goal" reads `target`, so
    /// a taxed goal is measured against what the item costs at the register.
    pub fn set_goals(&mut self, goals: Vec<Funding>) -> Result<()> {
        self.all = crate::savings::rows(goals, &self.accounts, self.today, self.period_days)?;
        self.rebuild_months();
        self.refilter();
        Ok(())
    }

    /// Take a refreshed account list, for the names the rows carry.
    ///
    /// The container filter is an id rather than an index, so nothing here
    /// has to be carried across -- unlike the ledgers', which is a position.
    /// The rows cache their container's name, so `App` re-sets the goals
    /// after this rather than the other way round.
    pub fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
    }

    pub fn set_containers(&mut self, containers: Vec<AccountId>) {
        self.containers = containers;
        self.refilter();
    }

    pub fn set_excess(&mut self, excess: Vec<(AccountId, Cents)>) {
        self.excess = excess;
    }

    pub fn excess(&self) -> &[(AccountId, Cents)] {
        &self.excess
    }

    /// The container's name as text, for the two callers that cannot take an
    /// `Account`: the reconciliation footer below, a status strip rather
    /// than a place a reader looks to identify an account, and
    /// `App::open_allocate`'s prefill for the Allocation modal's
    /// `container_name`, which draws into that modal's body. The second is
    /// the residual, listed with its reason in `src/tui/CLAUDE.md`'s
    /// account-color section -- `AllocationForm` is outside this guarantee.
    pub fn account_name(&self, id: AccountId) -> &str {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map_or("?", |a| a.name.as_str())
    }

    /// `Tab`: All -> each container in `goal::containers` order -> All.
    pub fn next_container(&mut self) {
        self.container = match self.container {
            None => self.containers.first().copied(),
            Some(current) => match self.containers.iter().position(|id| *id == current) {
                Some(i) if i + 1 < self.containers.len() => Some(self.containers[i + 1]),
                _ => None,
            },
        };
        self.refilter();
    }

    /// `BackTab`: the same cycle the other way -- All -> the last container
    /// in `goal::containers` order -> its predecessor -> All.
    pub fn previous_container(&mut self) {
        self.container = match self.container {
            None => self.containers.last().copied(),
            Some(current) => match self.containers.iter().position(|id| *id == current) {
                Some(i) if i > 0 => Some(self.containers[i - 1]),
                _ => None,
            },
        };
        self.refilter();
    }

    pub fn selected_container(&self) -> Option<AccountId> {
        self.container
    }

    /// The months `[` and `]` step through: every month from the earliest
    /// `goal_date` to the latest, and where a step out of All enters.
    ///
    /// Empty months are kept, so the cycle is one unbroken calendar rather
    /// than a list that skips. Stepping enters at today's month, or at
    /// whichever end of the span is nearer when today falls outside it --
    /// every goal being dated next year is a filter that should still open
    /// somewhere real. No dated goals at all leaves the cycle empty, and an
    /// empty cycle cannot be stepped out of All.
    fn rebuild_months(&mut self) {
        let dated = || self.all.iter().filter_map(|row| row.goal_date);
        let today = YearMonth::of(self.today);
        let (Some(first), Some(last)) = (dated().min(), dated().max()) else {
            self.month.set_months(Vec::new(), today);
            return;
        };
        let (first, last) = (YearMonth::of(first), YearMonth::of(last));
        self.month
            .set_months(YearMonth::range(first, last), today.clamp(first, last));
    }

    /// `]`: forward a month, the last dated month wrapping to the first.
    pub fn next_month(&mut self) {
        self.month.next();
        self.refilter();
    }

    /// `[`: back a month, the first dated month wrapping to the last.
    pub fn previous_month(&mut self) {
        self.month.previous();
        self.refilter();
    }

    /// `Esc`: show every goal again, undated ones included -- both filters
    /// at once, whichever of them is set. The screen narrows two ways and
    /// the title shows them side by side, so one key that clears whatever is
    /// there is a reflex where "Esc means month, Tab back around to All"
    /// asks the owner to remember which of the two narrowed the list they
    /// are looking at.
    pub fn clear_filters(&mut self) {
        self.container = None;
        self.month.clear();
        self.refilter();
    }

    pub fn selected_month(&self) -> Option<YearMonth> {
        self.month.selected()
    }

    /// The container a new goal or a worksheet defaults to: the `Tab` filter,
    /// or the first container when it is All.
    pub fn default_container(&self) -> Option<AccountId> {
        self.container.or_else(|| self.containers.first().copied())
    }

    pub fn rows(&self) -> Vec<&Row> {
        self.visible.iter().map(|i| &self.all[*i]).collect()
    }

    pub fn selected(&self) -> Option<&Row> {
        self.visible.get(self.cursor.index()).map(|i| &self.all[*i])
    }

    /// Put the cursor on a goal by id, leaving it where it is if the goal is
    /// not among the visible rows.
    ///
    /// By id and not by index, for the one caller that needs it: `K` and `J`
    /// reorder the rows under the cursor, so the index it held before the
    /// move names a different goal after it.
    pub fn select_goal(&mut self, id: GoalId) {
        if let Some(index) = self.visible.iter().position(|i| self.all[*i].goal_id == id) {
            self.cursor.select(index);
        }
    }

    pub fn title(&self) -> Label {
        let mut title = match self.container {
            None => Label::plain("Savings · All"),
            Some(id) => {
                Label::plain("Savings · ").account(super::Account::named(&self.accounts, id))
            }
        };
        if let Some(month) = self.month.selected() {
            title = title.text(format!(" · {}", month.label()));
        }
        if !self.search().is_empty() {
            title = title.text(format!(" · /{}", self.search()));
        }
        title
    }
}

impl Search for Savings {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    /// The container filter, the month, and the search, in one pass — so the
    /// three cannot narrow to different lists. Also the hook `Tab` and `[`/`]`
    /// call when they move.
    ///
    /// A row answers to its name and to the two figures it is *about*. `%` and
    /// `$/Pay` are derived from those two and are deliberately not offered:
    /// a needle reaching a readout would narrow through a column nobody was
    /// searching. The date is not offered either — `[`/`]` is its filter.
    fn refilter(&mut self) {
        let matcher = self.matcher();
        let container = self.container;
        let month = self.month.selected();
        self.visible = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, row)| container.is_none_or(|id| row.container.id() == id))
            // A goal with no date belongs to no month, so a month filter
            // drops it: All is the only place it can be seen.
            .filter(|(_, row)| month.is_none_or(|m| row.goal_date.is_some_and(|d| m.contains(d))))
            .filter(|(_, row)| matcher.matches(&row.name, &[row.current, row.goal]))
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
    }
}

impl Scroll for Savings {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// The rows the container filter and the search left, not every goal.
    fn row_count(&self) -> usize {
        self.visible.len()
    }
}

use super::{Label, account_cell, label_line, right_header, table_state, whole_amount};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Cell, Paragraph, Row as TableRow, Table};

/// A right-aligned cell, or an em dash where the sheet leaves the cell blank.
fn optional(text: Option<String>) -> Cell<'static> {
    Cell::from(TextLine::from(text.unwrap_or_else(|| "—".to_string())).right_aligned())
}

/// How funded a goal is, on [`super::style::percent_color`]'s ramp.
///
/// A goal with no positive target has no percentage to place on the ramp, so
/// it stays the plain em dash rather than being colored as if it were at zero.
fn percent(percent: Option<Percent>) -> Cell<'static> {
    match percent {
        Some(p) => super::tinted(
            TextLine::from(format!("{}%", p.0)).right_aligned(),
            Some(super::style::percent_color(p)),
        ),
        None => optional(None),
    }
}

/// The goal date, marked when the goal is past it and still short.
fn goal_date(row: &Row) -> Option<String> {
    row.goal_date
        .map(|d| format!("{d}{}", if row.expired { "!" } else { "" }))
}

/// One flat list of every open goal, with the container reconciliation below.
///
/// The table's money columns are whole dollars ([`super::whole_amount`]). The
/// reconciliation is not: it exists to show sub-dollar drift, which truncation
/// would erase.
///
/// Returns the [`Viewport`] it drew — the height `PageUp` and `PageDown` move
/// by, and the row the next draw starts from — which `App` records on the
/// `Savings`.
pub(super) fn render(frame: &mut Frame, area: Rect, savings: &Savings) -> Viewport {
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);

    // The band is the *row's* style rather than any cell's, which is the one
    // place a color may cover padding: it is the row that is marked, so a
    // band stopping at the last glyph of each column would read as seven
    // marks. Every cell patches over it, so the account tint and the funding
    // ramp are untouched -- and the bold is what survives the cursor's
    // `REVERSED`, which swaps the band's two halves away.
    let band = super::style::favorite().add_modifier(Modifier::BOLD);
    let rows: Vec<TableRow> = savings
        .rows()
        .iter()
        .map(|r| {
            TableRow::new(vec![
                account_cell(&r.container),
                Cell::from(r.name.clone()),
                whole_amount(r.current),
                whole_amount(r.goal),
                percent(r.percent),
                optional(goal_date(r)),
                optional(r.per_paycheck.map(crate::demo::whole_figure)),
            ])
            .style(if r.favorite { band } else { Style::default() })
        })
        .collect();

    // Every column but the two names is right-aligned, so every header but
    // those two is too.
    let header = TableRow::new(vec![
        Cell::from("Account"),
        Cell::from("Goal"),
        right_header("Current"),
        right_header("Goal"),
        right_header("%"),
        right_header("Goal Date"),
        right_header("$/Pay"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    // `Account` holds the longest container name (`Brokerage`) and a gap.
    // The money columns pay for most of it out of padding they never used --
    // a whole-dollar amount runs to ten characters -- and `Goal` gives up the
    // last one. `Goal Date` cannot help: it is a ten-character date plus the
    // overdue marker.
    let widths = [
        Constraint::Length(11),
        Constraint::Min(17),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(4),
        Constraint::Length(11),
        Constraint::Length(7),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ")
        .block(Block::bordered().title(label_line(&savings.title())));

    // Two borders and the header row are not available to data rows.
    let height = usize::from(table_area.height).saturating_sub(3);
    let (mut state, viewport) = table_state(savings, savings.rows().len(), height);
    frame.render_stateful_widget(table, table_area, &mut state);

    let line = savings
        .excess()
        .iter()
        .map(|(id, excess)| {
            let marker = if is_reconciled(*excess) { "✓" } else { "!" };
            format!(
                "{} {} {marker}",
                savings.account_name(*id),
                crate::demo::figure(*excess)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    frame.render_widget(
        Paragraph::new(TextLine::from(line)).block(Block::bordered().title("Unallocated")),
        footer_area,
    );

    viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::GoalId;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::Goal;
    use crate::goal::Funding;
    use crate::tui::MIN_WIDTH;
    use crate::tui::form::backspace_key;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn today() -> NaiveDate {
        day(2026, 8, 12)
    }

    fn accounts() -> Vec<Account> {
        vec![
            Account {
                id: AccountId(1),
                code: "SAV".into(),
                name: "Rainy Day".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            Account {
                id: AccountId(2),
                code: "BKR".into(),
                name: "Brokerage".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: None,
            },
        ]
    }

    /// `id` doubles as the sort key, so goals arrive in the order
    /// `all_with_balances` would return them.
    fn goal(
        id: i64,
        container: i64,
        name: &str,
        current: i64,
        target: i64,
        date: Option<NaiveDate>,
    ) -> Funding {
        Funding {
            goal: Goal {
                id: GoalId(id),
                name: name.to_string(),
                container_account_id: AccountId(container),
                base_cents: Cents(target),
                goal_date: date,
                recurring_goal_id: None,
                interest_eligible: true,
                closed: false,
                sort: id,
                favorite: false,
                taxed: false,
            },
            current: Cents(current),
            target: Cents(target),
        }
    }

    /// The four rows of the design's screen mock, at its own today.
    fn savings() -> Savings {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Bill Payments", 1_300_000, 1_500_000, None),
                goal(2, 1, "Apple Watch", 48_500, 50_000, Some(day(2026, 9, 1))),
                goal(3, 1, "Dropbox", 0, 15_000, Some(day(2026, 9, 1))),
                goal(4, 2, "Emergency Savings", 10_600_195, 10_000_000, None),
            ])
            .unwrap();
        savings.set_excess(vec![(AccountId(1), Cents(23)), (AccountId(2), Cents::ZERO)]);
        savings
    }

    fn names(savings: &Savings) -> Vec<&str> {
        savings.rows().iter().map(|r| r.name.as_str()).collect()
    }

    /// Goals whose dates span August to October 2026, with today inside the
    /// span and one undated goal in each container.
    fn dated() -> Savings {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Bill Payments", 1_200_000, 1_500_000, None),
                goal(2, 1, "Apple Watch", 49_100, 50_500, Some(day(2026, 8, 20))),
                goal(3, 1, "Dropbox", 0, 12_800, Some(day(2026, 9, 1))),
                goal(
                    4,
                    2,
                    "Emergency Savings",
                    10_600_195,
                    10_000_000,
                    Some(day(2026, 10, 15)),
                ),
                goal(5, 2, "Lego", 5_000, 20_000, Some(day(2026, 8, 3))),
            ])
            .unwrap();
        savings
    }

    fn month(year: i32, month: u32) -> YearMonth {
        YearMonth::of(day(year, month, 1))
    }

    #[test]
    fn the_screen_opens_on_every_month() {
        assert_eq!(dated().selected_month(), None);
    }

    #[test]
    fn the_first_step_filters_to_todays_month() {
        let mut savings = dated();
        savings.next_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 8)));
    }

    #[test]
    fn stepping_back_out_of_all_lands_on_the_same_month_as_stepping_forward() {
        let mut savings = dated();
        savings.previous_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 8)));
    }

    /// The whole point of the filter: a goal with no date has no month to be
    /// shown under, so All is the only place it appears.
    #[test]
    fn a_goal_with_no_date_shows_only_in_all() {
        let mut savings = dated();
        assert!(names(&savings).contains(&"Bill Payments"));
        for _ in 0..3 {
            savings.next_month();
            assert!(!names(&savings).contains(&"Bill Payments"));
        }
    }

    #[test]
    fn a_month_shows_the_goals_dated_within_it() {
        let mut savings = dated();
        savings.next_month();
        assert_eq!(names(&savings), vec!["Apple Watch", "Lego"]);
    }

    /// The cycle is the span the dated goals cover, empty months included --
    /// not the set of months that happen to hold a goal.
    #[test]
    fn stepping_past_the_last_dated_month_wraps_to_the_first() {
        let mut savings = dated();
        for _ in 0..3 {
            savings.next_month();
        }
        assert_eq!(savings.selected_month(), Some(month(2026, 10)));
        savings.next_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 8)));
    }

    #[test]
    fn stepping_before_the_first_dated_month_wraps_to_the_last() {
        let mut savings = dated();
        savings.next_month();
        savings.previous_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 10)));
    }

    #[test]
    fn esc_shows_every_goal_again() {
        let mut savings = dated();
        savings.next_month();
        savings.clear_filters();
        assert_eq!(savings.selected_month(), None);
        assert_eq!(names(&savings).len(), 5);
    }

    /// The screen narrows two ways, and `Esc` is the one key out of either.
    #[test]
    fn esc_clears_the_container_filter() {
        let mut savings = dated();
        savings.next_container();
        assert_eq!(savings.selected_container(), Some(AccountId(1)));
        savings.clear_filters();
        assert_eq!(savings.selected_container(), None);
        assert_eq!(names(&savings).len(), 5);
    }

    #[test]
    fn esc_clears_the_container_and_the_month_in_one_press() {
        let mut savings = dated();
        savings.next_container();
        savings.next_month();
        assert_eq!(savings.selected_container(), Some(AccountId(1)));
        assert_eq!(savings.selected_month(), Some(month(2026, 8)));
        savings.clear_filters();
        assert_eq!(savings.selected_container(), None);
        assert_eq!(savings.selected_month(), None);
        assert_eq!(names(&savings).len(), 5);
    }

    /// `Esc` clears what the two filters narrowed and leaves the search
    /// alone: the box has its own `Esc`, and it is the same box the ledgers
    /// and the worksheet use.
    #[test]
    fn esc_leaves_the_search_alone() {
        let mut savings = dated();
        for c in "lego".chars() {
            savings.push_search(c);
        }
        savings.next_container();
        savings.clear_filters();
        assert_eq!(savings.search(), "lego");
        assert_eq!(names(&savings), vec!["Lego"]);
    }

    #[test]
    fn a_step_after_esc_re_enters_at_todays_month_not_the_one_left_behind() {
        let mut savings = dated();
        savings.next_month();
        savings.next_month();
        savings.clear_filters();
        savings.next_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 8)));
    }

    /// Today is August 2026 and nothing is dated before September, so there
    /// is no August in the cycle to enter at.
    #[test]
    fn the_first_step_enters_at_the_nearest_month_when_today_is_outside_the_span() {
        let mut savings = savings();
        savings.next_month();
        assert_eq!(savings.selected_month(), Some(month(2026, 9)));
    }

    #[test]
    fn with_no_dated_goals_the_month_never_leaves_all() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![goal(
                1,
                1,
                "Bill Payments",
                1_200_000,
                1_500_000,
                None,
            )])
            .unwrap();
        savings.next_month();
        assert_eq!(savings.selected_month(), None);
        assert_eq!(names(&savings), vec!["Bill Payments"]);
    }

    #[test]
    fn the_month_narrows_within_the_container_filter() {
        let mut savings = dated();
        savings.next_container();
        savings.next_month();
        assert_eq!(names(&savings), vec!["Apple Watch"]);
    }

    #[test]
    fn the_month_narrows_within_the_search() {
        let mut savings = dated();
        for c in "lego".chars() {
            savings.push_search(c);
        }
        savings.next_month();
        assert_eq!(names(&savings), vec!["Lego"]);
    }

    /// The container slot already spells "All", so a second one beside it
    /// would read as a bug rather than as a filter that is off.
    #[test]
    fn the_title_names_the_month_only_once_one_is_selected() {
        let mut savings = dated();
        assert_eq!(savings.title().plain_text(), "Savings \u{b7} All");
        savings.next_month();
        assert_eq!(
            savings.title().plain_text(),
            "Savings \u{b7} All \u{b7} Aug 2026"
        );
    }

    #[test]
    fn tab_cycles_all_then_each_container_then_back_to_all() {
        let mut savings = savings();
        assert_eq!(savings.selected_container(), None);
        assert_eq!(names(&savings).len(), 4);

        savings.next_container();
        assert_eq!(savings.selected_container(), Some(AccountId(1)));
        assert_eq!(names(&savings), ["Bill Payments", "Apple Watch", "Dropbox"]);

        savings.next_container();
        assert_eq!(savings.selected_container(), Some(AccountId(2)));
        assert_eq!(names(&savings), ["Emergency Savings"]);

        savings.next_container();
        assert_eq!(savings.selected_container(), None, "Tab must return to All");
    }

    #[test]
    fn back_tab_cycles_all_then_each_container_in_reverse() {
        let mut savings = savings();

        savings.previous_container();
        assert_eq!(savings.selected_container(), Some(AccountId(2)));
        assert_eq!(names(&savings), ["Emergency Savings"]);

        savings.previous_container();
        assert_eq!(savings.selected_container(), Some(AccountId(1)));
        assert_eq!(names(&savings), ["Bill Payments", "Apple Watch", "Dropbox"]);

        savings.previous_container();
        assert_eq!(
            savings.selected_container(),
            None,
            "BackTab must return to All"
        );
        assert_eq!(names(&savings).len(), 4);
    }

    /// The two directions are one cycle, not two: whatever `Tab` reached,
    /// `BackTab` undoes.
    #[test]
    fn back_tab_undoes_a_tab_from_anywhere_in_the_container_cycle() {
        let mut savings = savings();
        for _ in 0..3 {
            let before = savings.selected_container();
            savings.next_container();
            savings.previous_container();
            assert_eq!(savings.selected_container(), before);
            savings.next_container();
        }
    }

    /// The containers come from the data on every reload. Storing the filter
    /// as an index would silently point at a different container if the list
    /// changed underneath it.
    #[test]
    fn a_container_filter_survives_the_container_list_being_requeried() {
        let mut savings = savings();
        savings.next_container();
        savings.next_container();
        assert_eq!(savings.selected_container(), Some(AccountId(2)));

        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        assert_eq!(savings.selected_container(), Some(AccountId(2)));
        assert_eq!(names(&savings), ["Emergency Savings"]);
    }

    #[test]
    fn search_matches_names_case_insensitively_and_narrows_the_list() {
        let mut savings = savings();
        savings.begin_search();
        for c in "APP".chars() {
            savings.push_search(c);
        }
        assert_eq!(names(&savings), ["Apple Watch"]);

        savings.edit_search(backspace_key());
        savings.edit_search(backspace_key());
        savings.edit_search(backspace_key());
        assert_eq!(names(&savings).len(), 4);
    }

    /// The figure the row is *at*, typed without the separators the column
    /// draws.
    #[test]
    fn search_matches_a_goals_current_balance() {
        let mut savings = savings();
        savings.begin_search();
        for c in "485".chars() {
            savings.push_search(c);
        }
        assert_eq!(names(&savings), ["Apple Watch"]);
    }

    #[test]
    fn search_matches_a_goals_target() {
        let mut savings = savings();
        savings.begin_search();
        for c in "150.00".chars() {
            savings.push_search(c);
        }
        assert_eq!(names(&savings), ["Dropbox"]);
    }

    /// `%` and `$/Pay` are readouts derived from the two figures beside them,
    /// and a needle reaching them would hit rows through a column the owner
    /// was not searching. Apple Watch sits at 97%; nothing answers to it.
    #[test]
    fn search_does_not_reach_the_derived_columns() {
        let mut savings = savings();
        savings.begin_search();
        for c in "97".chars() {
            savings.push_search(c);
        }
        assert!(names(&savings).is_empty());
    }

    /// A selection left past the end would make `a`, `c` and `e` operate on
    /// nothing -- or, worse, on whatever later lands at that index.
    #[test]
    fn a_shrinking_filter_moves_the_selection_into_bounds() {
        let mut savings = savings();
        savings.select_last();
        assert_eq!(savings.selected().unwrap().name, "Emergency Savings");

        savings.begin_search();
        for c in "Dropbox".chars() {
            savings.push_search(c);
        }
        assert_eq!(savings.selected_index(), 0);
        assert_eq!(savings.selected().unwrap().name, "Dropbox");
    }

    #[test]
    fn an_empty_result_has_nothing_selected() {
        let mut savings = savings();
        savings.begin_search();
        for c in "zzz".chars() {
            savings.push_search(c);
        }
        assert!(savings.rows().is_empty());
        assert!(savings.selected().is_none());
    }

    /// Drawn at `MIN_WIDTH`, the narrowest terminal this screen is laid out
    /// for. The column widths must leave the right-aligned `Goal Date` cell
    /// room for a full `YYYY-MM-DD` (plus the `!` marker on an expired goal) -- a
    /// right-aligned cell that gets truncated loses its *leading* characters,
    /// so a shortfall here silently turns `2026-11-27` into a wrong year
    /// rather than an obviously-broken string.
    /// A demo blocks the money and nothing else: the `%` column is a shape
    /// rather than a sum, and a goal's name, date and container are already
    /// the owner's own words.
    #[test]
    fn a_demo_blocks_the_figures_and_keeps_the_percentages() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install(true);
        let savings = savings();
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &savings);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!text.contains("13,000"), "a balance survived: {text}");
        assert!(!text.contains("15,000"), "a target survived: {text}");
        assert!(
            !text.contains("0.23"),
            "the unallocated footer survived: {text}"
        );
        assert!(text.contains("██████"), "nothing was blocked: {text}");
        assert!(text.contains("106%"), "the percentages must stay: {text}");
        assert!(
            text.contains("Apple Watch"),
            "the goal names must stay: {text}"
        );
        assert!(
            text.contains("2026-09-01"),
            "the goal dates must stay: {text}"
        );
    }

    #[test]
    fn the_goal_date_column_is_not_truncated_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Future", 0, 10_000, Some(day(2026, 11, 27))),
                goal(2, 1, "Late", 5_000, 10_000, Some(day(2026, 7, 1))),
            ])
            .unwrap();

        let backend = TestBackend::new(MIN_WIDTH, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &savings);
            })
            .unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("2026-11-27"), "{text}");
        assert!(text.contains("2026-07-01!"), "{text}");
    }

    /// The Account column is tinted by the goal's *container*, which is the
    /// account it names. Tinting by `goal_id` -- the other id on the row --
    /// would give three goals in one container three colors and make the
    /// column say nothing.
    #[test]
    fn the_account_column_is_colored_by_the_container_not_the_goal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        savings
            .set_goals(vec![
                goal(7, 1, "Lego", 0, 10_000, None),
                goal(8, 2, "Emergency", 0, 10_000, None),
            ])
            .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &savings);
            })
            .unwrap();

        // Left border, then the two-column highlight symbol, then `Acct`.
        // Row 0 is the border, row 1 the header.
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(3, 2)].symbol(), "R", "{:?}", buffer[(3, 2)]);
        assert_eq!(
            buffer[(3, 2)].fg,
            super::super::style::account_color(AccountId(1), None)
        );
        assert_eq!(buffer[(3, 3)].symbol(), "B", "{:?}", buffer[(3, 3)]);
        assert_eq!(
            buffer[(3, 3)].fg,
            super::super::style::account_color(AccountId(2), None)
        );
    }

    /// The ramp reaches the screen, and a goal with no percentage to place on
    /// it keeps the plain em dash -- coloring `--` would put an unfunded red
    /// on a goal that has no target rather than no money.
    #[test]
    fn the_percent_column_is_colored_by_the_funding_ramp() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;

        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Empty", 0, 10_000, None),
                goal(2, 1, "Funded", 10_000, 10_000, None),
                goal(3, 1, "No target", 5_000, 0, None),
            ])
            .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 9)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &savings);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // The `%` sign ends each figure, so the cell before it is colored too;
        // finding the sign locates the column without restating the widths.
        let percent_sign = |y: u16| {
            (0..MIN_WIDTH)
                .find(|x| buffer[(*x, y)].symbol() == "%")
                .unwrap_or_else(|| panic!("no percent sign on row {y}"))
        };
        assert_eq!(
            buffer[(percent_sign(2), 2)].fg,
            super::super::style::percent_color(Percent::ZERO)
        );
        assert_eq!(
            buffer[(percent_sign(3), 3)].fg,
            super::super::style::percent_color(Percent::ONE_HUNDRED)
        );

        let dash = (0..MIN_WIDTH)
            .find(|x| buffer[(*x, 4)].symbol() == "—")
            .expect("the goal with no target renders an em dash");
        assert_eq!(buffer[(dash, 4)].fg, Color::Reset);
    }

    /// The same goal, marked. A helper rather than an eighth parameter on
    /// `goal`: every other test in this file is about a goal that is not
    /// favorited, and a `false` on each of them would say nothing.
    fn favorited(mut g: Funding) -> Funding {
        g.goal.favorite = true;
        g
    }

    /// The mark is a highlight and nothing else: it does not sort a goal up,
    /// and it does not survive a filter the goal itself would not.
    #[test]
    fn favoriting_a_goal_moves_it_nowhere() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Bill Payments", 1_300_000, 1_500_000, None),
                goal(2, 1, "Apple Watch", 48_500, 50_000, Some(day(2026, 9, 1))),
                favorited(goal(3, 1, "Dropbox", 0, 15_000, Some(day(2026, 9, 1)))),
                favorited(goal(
                    4,
                    2,
                    "Emergency Savings",
                    10_600_195,
                    10_000_000,
                    None,
                )),
            ])
            .unwrap();

        assert_eq!(
            names(&savings),
            vec![
                "Bill Payments",
                "Apple Watch",
                "Dropbox",
                "Emergency Savings"
            ]
        );

        savings.next_container();
        assert_eq!(
            names(&savings),
            vec!["Bill Payments", "Apple Watch", "Dropbox"],
            "a favorited goal in another container is still filtered out"
        );
    }

    /// The four-goal fixture with the third goal marked, drawn at
    /// `MIN_WIDTH`. Rows sit at `y = 2..6`, under the border and the header.
    fn banded() -> Savings {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1)]);
        savings
            .set_goals(vec![
                goal(1, 1, "Bill Payments", 1_300_000, 1_500_000, None),
                goal(2, 1, "Apple Watch", 48_500, 50_000, Some(day(2026, 9, 1))),
                favorited(goal(3, 1, "Dropbox", 0, 15_000, Some(day(2026, 9, 1)))),
            ])
            .unwrap();
        savings
    }

    /// Where a word starts on a drawn row. `column_of` and not `str::find`:
    /// the border is three bytes and one column, so a byte offset lands two
    /// columns right of the word asked for.
    fn word_at(buffer: &ratatui::buffer::Buffer, y: u16, word: &str) -> u16 {
        let line: String = (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect();
        super::super::column_of(&line, word)
    }

    fn band_buffer(savings: &Savings) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 9)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), savings);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// A band, not a tint: it runs the row's whole width, padding and gaps
    /// included, which is exactly what a cell-level color may never do.
    #[test]
    fn a_favorited_row_is_banded_across_its_whole_width() {
        let buffer = band_buffer(&banded());
        // Inside the two borders. Row 4 is the third goal.
        for x in 1..MIN_WIDTH - 1 {
            assert_eq!(
                buffer[(x, 4)].bg,
                super::super::style::FAVORITE_BG,
                "column {x}: {:?}",
                buffer[(x, 4)]
            );
        }
    }

    /// A band on every row is a band nobody reads. Row 3 is the unmarked
    /// goal directly above the marked one.
    #[test]
    fn an_unfavorited_row_carries_no_band() {
        use ratatui::style::Color;

        let buffer = band_buffer(&banded());
        for x in 1..MIN_WIDTH - 1 {
            assert_eq!(
                buffer[(x, 3)].bg,
                Color::Reset,
                "column {x}: {:?}",
                buffer[(x, 3)]
            );
        }
    }

    /// The band is the row's base style and every cell patches over it, so
    /// the account tint and the funding ramp are unchanged by it -- the row
    /// still says which account and how funded, in the colors it always did.
    #[test]
    fn the_band_leaves_the_cell_colors_on_top_of_it() {
        let buffer = band_buffer(&banded());
        // Left border, then the two-column highlight symbol, then `Acct`.
        assert_eq!(buffer[(3, 4)].symbol(), "R", "{:?}", buffer[(3, 4)]);
        assert_eq!(
            buffer[(3, 4)].fg,
            super::super::style::account_color(AccountId(1), None)
        );

        let percent_sign = (0..MIN_WIDTH)
            .find(|x| buffer[(*x, 4)].symbol() == "%")
            .expect("the marked goal shows a percentage");
        assert_eq!(
            buffer[(percent_sign, 4)].fg,
            super::super::style::percent_color(Percent::ZERO)
        );
    }

    /// A cell the band alone colors takes the band's own foreground, or the
    /// row would be the terminal's default text on a fixed background --
    /// readable under one theme and invisible under the other.
    #[test]
    fn a_banded_cell_with_no_color_of_its_own_takes_the_bands_foreground() {
        let buffer = band_buffer(&banded());
        let name = word_at(&buffer, 4, "Dropbox");
        assert_eq!(buffer[(name, 4)].fg, super::super::style::FAVORITE_FG);
    }

    /// The cursor's `REVERSED` is patched over the row after its cells draw,
    /// so it swaps the band's two halves and a marked row under the cursor
    /// would otherwise be indistinguishable from any other cursor row. The
    /// bold is what survives the swap and keeps the mark readable there.
    #[test]
    fn a_favorited_row_under_the_cursor_is_still_marked() {
        let mut savings = banded();
        savings.select_last();
        let buffer = band_buffer(&savings);

        let cell = &buffer[(word_at(&buffer, 4, "Dropbox"), 4)];
        assert!(cell.modifier.contains(Modifier::REVERSED), "{cell:?}");
        assert!(cell.modifier.contains(Modifier::BOLD), "{cell:?}");
    }

    /// Every rendered line of the four-goal fixture, inside the border.
    fn drawn(savings: &Savings) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), savings);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..12)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// Both ends of the widest row survive `MIN_WIDTH`: the longest container
    /// name in the `Account` column, and the longest goal name beside it.
    /// The `Account` column was paid for out of the money columns' padding,
    /// so this is the test that catches taking one character too many.
    #[test]
    fn the_widest_account_and_goal_names_are_whole_at_the_minimum_width() {
        let text = drawn(&savings()).join("\n");
        assert!(text.contains("Brokerage"), "{text}");
        assert!(text.contains("Emergency Savings"), "{text}");
    }

    /// A right-aligned column's header belongs over its own figures. Left
    /// over right, `Current` sat at the far side of a ten-wide column from
    /// every number in it, reading as a label for the goal name beside it.
    ///
    /// The Apple Watch row is the one with a figure in all five right-aligned
    /// columns, so it is the row every header is measured against.
    #[test]
    fn every_right_aligned_header_ends_where_its_own_column_does() {
        let lines = drawn(&savings());
        let header = super::super::ends_in_order(
            &lines[1],
            &[
                "Account",
                "Goal",
                "Current",
                "Goal",
                "%",
                "Goal Date",
                "$/Pay",
            ],
        );
        let row = super::super::ends_in_order(
            &lines[3],
            &[
                "Rainy Day",
                "Apple Watch",
                "485",
                "500",
                "97%",
                "2026-09-01",
                "8",
            ],
        );
        // The two name columns are left-aligned; the five after them are not.
        assert_eq!(
            header[2..],
            row[2..],
            "header {:?}\nrow    {:?}",
            lines[1],
            lines[3]
        );
    }

    /// Current, Goal and `$/Pay` drop the cents rather than rounding: the
    /// 13,000.00 balance reads 13,000 and the 106,001.95 one reads 106,001.
    #[test]
    fn the_money_columns_are_whole_dollars() {
        let text = drawn(&savings()).join("\n");
        assert!(text.contains("13,000"), "{text}");
        assert!(text.contains("106,001"), "{text}");
        assert!(!text.contains("13,000.00"), "{text}");
        assert!(!text.contains("106,001.95"), "{text}");
    }

    /// The reconciliation is about amounts smaller than a dollar, so
    /// truncating it would leave the line reading `Rainy Day 0 ✓` and saying
    /// nothing.
    #[test]
    fn the_unallocated_footer_keeps_its_cents() {
        let text = drawn(&savings()).join("\n");
        assert!(text.contains("Rainy Day 0.23"), "{text}");
    }

    #[test]
    fn the_title_names_the_container_filter_and_the_search() {
        let mut savings = savings();
        assert_eq!(savings.title().plain_text(), "Savings · All");
        savings.next_container();
        assert_eq!(savings.title().plain_text(), "Savings · Rainy Day");
        savings.begin_search();
        savings.push_search('D');
        assert_eq!(savings.title().plain_text(), "Savings · Rainy Day · /D");
    }

    /// The container in the title is the same account the Account column
    /// names, so it is the same color there too -- a title is where a reader
    /// looks to find out which container they are in.
    #[test]
    fn the_savings_title_names_its_container_as_an_account() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings.set_containers(vec![AccountId(1), AccountId(2)]);
        savings.next_container();
        let title = savings.title();
        assert_eq!(title.plain_text(), "Savings · Rainy Day");
        assert_eq!(title.accounts().len(), 1);
        assert_eq!(title.accounts()[0].id(), AccountId(1));
    }

    /// `All` is not an account and takes no color: coloring it would make the
    /// unfiltered screen look like it was filtered to something.
    #[test]
    fn the_unfiltered_savings_title_names_no_account() {
        let savings = Savings::new(accounts(), today(), 14);
        assert_eq!(savings.title().plain_text(), "Savings · All");
        assert!(savings.title().accounts().is_empty());
    }
}
