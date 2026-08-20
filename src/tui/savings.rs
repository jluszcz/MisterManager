use super::cursor::{Cursor, Scroll};
use super::month::{MonthCycle, YearMonth};
use super::search::{Search, SearchBox};
use crate::db::account::Account;
use crate::db::goal::GoalWithBalance;
use crate::db::{AccountId, GoalId};
use crate::money::Cents;
use crate::rate::Percent;
use anyhow::Result;
use chrono::NaiveDate;

/// One goal as the screen shows it. Every derived column is computed once,
/// when the goals are set, so filtering and paging cannot be a source of
/// arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub goal_id: GoalId,
    /// The container this goal sits in, as the Account column shows it. One
    /// value rather than an id, a name and a color kept in step by hand.
    pub container: super::Account,
    pub name: String,
    pub current: Cents,
    pub goal: Cents,
    pub percent: Option<Percent>,
    pub goal_date: Option<NaiveDate>,
    /// Past its goal date and still short. What the screen offers there is a
    /// close-out, not a roll forward.
    pub expired: bool,
    pub per_paycheck: Option<Cents>,
    /// Whether an interest posting weights this goal. Carried so `e` can
    /// prefill the form from the row it already has.
    pub interest_eligible: bool,
}

/// `current / goal`, as a whole percent rounded to nearest.
///
/// `None` for a non-positive target: `goal_cents` is user-editable and reaches
/// a divisor, which is the case `div_ceil` exists to refuse. The screen shows
/// `--` there rather than dividing.
///
/// `i128` because `current * 100` overflows an `i64` only above ~$922
/// trillion, but the doubling below halves even that headroom for no
/// benefit.
///
/// Uses `div_euclid`, not `/`, on the halfway-adjusted ratio: `goal` is
/// always positive here, so Euclidean division is floor division, which is
/// what "round to nearest" needs. Rust's `/` truncates toward zero instead,
/// which would round an overspent goal's negative `current` the wrong way.
pub fn percent_complete(current: Cents, goal: Cents) -> Option<Percent> {
    if goal <= Cents::ZERO {
        return None;
    }
    let goal = goal.0 as i128;
    let scaled = current.0 as i128 * 100;
    Some(Percent((scaled * 2 + goal).div_euclid(goal * 2) as i64))
}

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
    pub fn set_goals(&mut self, goals: Vec<GoalWithBalance>) -> Result<()> {
        let mut rows = Vec::with_capacity(goals.len());
        for g in goals {
            let per_paycheck = super::paycheck_ask(&g, self.today, self.period_days)?;
            rows.push(Row {
                goal_id: g.goal.id,
                container: super::Account::named(&self.accounts, g.goal.container_account_id),
                percent: percent_complete(g.current, g.goal.goal_cents),
                expired: g.goal.goal_date.is_some_and(|d| d < self.today)
                    && g.current < g.goal.goal_cents,
                name: g.goal.name,
                current: g.current,
                goal: g.goal.goal_cents,
                goal_date: g.goal.goal_date,
                per_paycheck,
                interest_eligible: g.goal.interest_eligible,
            });
        }
        self.all = rows;
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

    /// `Esc`: show every goal again, undated ones included.
    pub fn clear_month(&mut self) {
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
    fn refilter(&mut self) {
        let needle = self.search().to_lowercase();
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
            .filter(|(_, row)| needle.is_empty() || row.name.to_lowercase().contains(&needle))
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
/// Returns the viewport height in rows, which `App` records on the `Savings`
/// for `PageUp` and `PageDown` to move by.
pub fn render(frame: &mut Frame, area: Rect, savings: &Savings) -> usize {
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);

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
                optional(r.per_paycheck.map(Cents::to_whole_dollars)),
            ])
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
    let mut state = table_state(savings.selected_index(), savings.rows().len(), height);
    frame.render_stateful_widget(table, table_area, &mut state);

    let line = savings
        .excess()
        .iter()
        .map(|(id, excess)| {
            let marker = if super::is_reconciled(*excess) {
                "✓"
            } else {
                "!"
            };
            format!("{} {excess} {marker}", savings.account_name(*id))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    frame.render_widget(
        Paragraph::new(TextLine::from(line)).block(Block::bordered().title("Unallocated")),
        footer_area,
    );

    height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::Goal;
    use crate::tui::MIN_WIDTH;

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
    ) -> GoalWithBalance {
        GoalWithBalance {
            goal: Goal {
                id: GoalId(id),
                name: name.to_string(),
                container_account_id: AccountId(container),
                goal_cents: Cents(target),
                goal_date: date,
                recurring_goal_id: None,
                interest_eligible: true,
                closed: false,
                sort: id,
            },
            current: Cents(current),
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
        savings.clear_month();
        assert_eq!(savings.selected_month(), None);
        assert_eq!(names(&savings).len(), 5);
    }

    #[test]
    fn a_step_after_esc_re_enters_at_todays_month_not_the_one_left_behind() {
        let mut savings = dated();
        savings.next_month();
        savings.next_month();
        savings.clear_month();
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

    /// 87%, 97%, 0%, 106% -- rounded to the nearest whole percent, not
    /// truncated, or 13,000/15,000 would read 86%.
    #[test]
    fn the_percent_column_rounds_to_the_nearest_whole_percent() {
        let savings = savings();
        let percents: Vec<Option<Percent>> = savings.rows().iter().map(|r| r.percent).collect();
        assert_eq!(
            percents,
            vec![
                Some(Percent(87)),
                Some(Percent(97)),
                Some(Percent::ZERO),
                Some(Percent(106))
            ]
        );
    }

    /// `goal_cents` is user-editable and reaches a divisor. A zero target
    /// renders `--` rather than dividing.
    #[test]
    fn a_goal_with_a_zero_target_has_no_percent_rather_than_dividing() {
        assert_eq!(percent_complete(Cents(500), Cents::ZERO), None);
        assert_eq!(percent_complete(Cents(500), Cents(-100)), None);
        assert_eq!(
            percent_complete(Cents::ZERO, Cents(100)),
            Some(Percent::ZERO)
        );
    }

    /// An overspent goal has a negative `current`. Truncating division would
    /// round -14.9% to -14; the nearest whole percent is -15.
    #[test]
    fn an_overspent_goal_rounds_its_negative_percent_to_the_nearest_whole_percent() {
        assert_eq!(
            percent_complete(Cents(-149), Cents(1000)),
            Some(Percent(-15))
        );
    }

    /// Column F of the sheet goes blank for undated and already-met goals.
    #[test]
    fn per_paycheck_is_blank_for_undated_and_met_goals() {
        let savings = savings();
        let rows = savings.rows();
        assert_eq!(rows[0].per_paycheck, None, "Bill Payments is undated");
        assert_eq!(rows[1].per_paycheck, Some(Cents(800)), "Apple Watch");
        assert_eq!(rows[2].per_paycheck, Some(Cents(7_500)), "Dropbox");
        assert_eq!(rows[3].per_paycheck, None, "Emergency Savings is undated");
    }

    #[test]
    fn a_met_goal_with_a_future_date_has_no_per_paycheck_figure() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings
            .set_goals(vec![goal(
                1,
                1,
                "Couch",
                100_000,
                100_000,
                Some(day(2026, 12, 1)),
            )])
            .unwrap();
        assert_eq!(savings.rows()[0].per_paycheck, None);
    }

    /// A goal past its date and still short is the parent design's "flags
    /// expired goals". A goal past its date that is funded is simply done.
    #[test]
    fn only_a_past_dated_goal_that_is_still_short_is_marked_expired() {
        let mut savings = Savings::new(accounts(), today(), 14);
        savings
            .set_goals(vec![
                goal(1, 1, "Late", 5_000, 10_000, Some(day(2026, 7, 1))),
                goal(2, 1, "Met", 10_000, 10_000, Some(day(2026, 7, 1))),
                goal(3, 1, "Ahead", 0, 10_000, Some(day(2027, 7, 1))),
                goal(4, 1, "Undated", 0, 10_000, None),
            ])
            .unwrap();
        let expired: Vec<bool> = savings.rows().iter().map(|r| r.expired).collect();
        assert_eq!(expired, vec![true, false, false, false]);
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

        savings.backspace_search();
        savings.backspace_search();
        savings.backspace_search();
        assert_eq!(names(&savings).len(), 4);
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

    #[test]
    fn the_account_column_names_each_goals_container() {
        let savings = savings();
        let names: Vec<&str> = savings.rows().iter().map(|r| r.container.text()).collect();
        assert_eq!(names, ["Rainy Day", "Rainy Day", "Rainy Day", "Brokerage"]);
    }

    /// Drawn at `MIN_WIDTH`, the narrowest terminal this screen is laid out
    /// for. The column widths must leave the right-aligned `Goal Date` cell
    /// room for a full `YYYY-MM-DD` (plus the `!` marker on an expired goal) -- a
    /// right-aligned cell that gets truncated loses its *leading* characters,
    /// so a shortfall here silently turns `2026-11-27` into a wrong year
    /// rather than an obviously-broken string.
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
