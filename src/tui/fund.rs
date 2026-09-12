//! Screen 6: the funds held across the owner's investment accounts.
//!
//! One row per holding -- the account it sits in, the ticker, the balance
//! the owner typed -- filtered by account and by a search over ticker and
//! account, and its form. What a fund is *made of* is read out of
//! `fund_mix`, keyed on the ticker rather than carried by the holding, so
//! one fetch prices every account that holds it; nothing here ever fetches
//! it. A holding with no mix on record and a fund holding no stock are two
//! different states, and neither is drawn as the other: `Row::stock_percent`
//! and `Row::as_of` are `None` exactly when no `fund_mix` row exists for the
//! ticker, and both columns draw the same `—` every other absence in the
//! app draws rather than a bar or a figure that would read as zero.
//!
//! Above that list sits the allocation summary: what the whole portfolio
//! holds by asset class, against what the age rule says it should. The rows
//! and the apportioning behind them are `crate::allocation`'s, not this
//! module's -- the report's Funds tab spells the same ones -- and what is
//! decided here is only what a terminal has to decide: the bar's glyphs, the
//! column widths, and how many lines the panel may take off the list.

use super::cursor::{Cursor, Viewport, impl_scroll};
use super::form::{AccountChoice, Field, Focused, FormFields, Step, next_in, parse_whole_amount};
use super::search::{Search, SearchBox};
use super::widget::{field_stack, render_fields};
use super::{
    Account, Chrome, GUTTER, Label, account_cell, render_table, right_header, whole_amount,
};
use crate::allocation::{self, Allocation, SummaryRow, TargetClass, UNTARGETED};
use crate::calc::fund::Targets;
use crate::db::account;
use crate::db::fund_mix::Slice;
use crate::db::{AccountId, HoldingId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Cell, Padding, Row as TableRow, Table};
use std::collections::HashMap;

/// One holding, as the Funds screen lists it.
///
/// `stock_percent` and `as_of` are `None` exactly when no `fund_mix` row
/// exists for `ticker` -- a fund nobody has asked SEC about yet. A fund
/// reported to hold no stock at all is `Some(BasisPoints::ZERO)`, never
/// `None`: zero and unknown are different states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: HoldingId,
    pub account_id: AccountId,
    pub account: Account,
    pub ticker: String,
    pub balance: Cents,
    pub stock_percent: Option<BasisPoints>,
    pub as_of: Option<NaiveDate>,
}

/// The Funds screen's view state: every holding, which account the `Tab`
/// filter has narrowed to, and where the cursor sits.
///
/// Holds no `Db`, the way `Ledger` and `Savings` do not: `App` runs the
/// queries -- the holdings, and the `fund_mix` row behind each ticker -- and
/// hands the built rows in through [`Funds::set_rows`].
pub struct Funds {
    accounts: Vec<account::Account>,
    /// Every composition on record, by ticker -- keyed the way `fund_mix`
    /// itself is, rather than copied onto each [`Row`], so two accounts
    /// holding one fund read one answer.
    mixes: HashMap<String, Vec<Slice>>,
    /// What the age rule asks for. Handed in beside the rows rather than
    /// derived here, `Funds` holding no `Db` -- and cached rather than taken
    /// per draw, since `Tab` recomputes the summary without a reload.
    targets: Targets,
    /// The look-through over whatever the filters leave, recomputed wherever
    /// [`Funds::refilter`] is: the summary answers for the rows under it, so
    /// a narrowing that moved one and not the other would put a portfolio's
    /// composition over one account's holdings.
    allocation: Allocation,
    /// `None` is the All filter, an id rather than an index into `accounts`
    /// for the reason `Savings::container` is one: nothing here has to be
    /// carried across a reload the way a ledger's position would be.
    account: Option<AccountId>,
    rows: Vec<Row>,
    /// Indices into `rows` that survive the account filter and the search.
    visible: Vec<usize>,
    search: SearchBox,
    cursor: Cursor,
}

impl Funds {
    pub fn new() -> Funds {
        Funds {
            accounts: Vec::new(),
            mixes: HashMap::new(),
            // An unconfigured database is a real state: no birth date on
            // record, and the split `crate::fund` opens on until an import
            // writes the sheet's own.
            targets: crate::calc::fund::targets(None, crate::fund::DEFAULT_INTL_EQUITY_SHARE),
            allocation: Allocation::default(),
            account: None,
            rows: Vec::new(),
            visible: Vec::new(),
            search: SearchBox::new(),
            cursor: Cursor::new(),
        }
    }

    /// Take a refreshed list of investment accounts, for the `Tab` filter and
    /// the account column.
    ///
    /// The filter is an id, so nothing has to be carried across the way a
    /// position would be -- an account the reload dropped simply narrows to
    /// nothing until `Tab` or `Esc`-equivalent cycling moves off it.
    pub fn set_accounts(&mut self, accounts: Vec<account::Account>) {
        self.accounts = accounts;
    }

    /// Take every holding across every investment account, already priced
    /// against whatever `fund_mix` rows are on record.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.refilter();
    }

    /// Take every composition on record, by ticker.
    pub fn set_mixes(&mut self, mixes: HashMap<String, Vec<Slice>>) {
        self.mixes = mixes;
        self.recompute_allocation();
    }

    /// Take the age rule's three shares, as of the day the app was opened on.
    pub fn set_targets(&mut self, targets: Targets) {
        self.targets = targets;
    }

    /// The portfolio look-through over the rows on screen, footing to
    /// [`BasisPoints::ONE`] -- or empty when nothing on screen has a mix on
    /// record, which is the state every database starts in.
    pub fn summary(&self) -> Vec<Slice> {
        self.allocation.slices.clone()
    }

    /// One targeted class as the summary draws it, or `None` when there is no
    /// composition to draw it against.
    ///
    /// Absent rather than zeroed, for the reason [`Row::stock_percent`] is:
    /// a portfolio nobody has fetched a mix for holds an unknown share of
    /// bonds, not none.
    pub fn summary_row(&self, class: TargetClass) -> Option<SummaryRow> {
        (!self.allocation.slices.is_empty())
            .then(|| SummaryRow::new(class, &self.allocation.slices, self.targets))
    }

    pub(super) fn allocation(&self) -> &Allocation {
        &self.allocation
    }

    fn recompute_allocation(&mut self) {
        let allocation = {
            let held: Vec<(Cents, Option<&[Slice]>)> = self
                .visible
                .iter()
                .map(|i| &self.rows[*i])
                .map(|row| (row.balance, self.mixes.get(&row.ticker).map(Vec::as_slice)))
                .collect();
            allocation::apportion(&held)
        };
        self.allocation = allocation;
    }

    /// The rows the account filter and the search leave, not every row
    /// fetched.
    pub fn rows(&self) -> Vec<&Row> {
        self.visible.iter().map(|i| &self.rows[*i]).collect()
    }

    pub fn selected(&self) -> Option<&Row> {
        self.visible
            .get(self.cursor.index())
            .map(|i| &self.rows[*i])
    }

    /// `Tab`: All -> each investment account, in `accounts` order -> All.
    pub fn next_account(&mut self) {
        self.account = match self.account {
            None => self.accounts.first().map(|a| a.id),
            Some(current) => match self.accounts.iter().position(|a| a.id == current) {
                Some(i) if i + 1 < self.accounts.len() => Some(self.accounts[i + 1].id),
                _ => None,
            },
        };
        self.refilter();
    }

    /// `BackTab`: the same cycle the other way -- All -> the last investment
    /// account -> its predecessor -> All.
    pub fn previous_account(&mut self) {
        self.account = match self.account {
            None => self.accounts.last().map(|a| a.id),
            Some(current) => match self.accounts.iter().position(|a| a.id == current) {
                Some(i) if i > 0 => Some(self.accounts[i - 1].id),
                _ => None,
            },
        };
        self.refilter();
    }

    /// The account the `Tab` filter is narrowed to, or `None` for All.
    ///
    /// `a` opens its form on this account: adding a holding while looking at
    /// one account and having the form default to a different one is a
    /// misfiled row with no ledger to catch it later.
    pub fn filter_account(&self) -> Option<AccountId> {
        self.account
    }

    /// An account's raw stored name, for the `/` filter to match against and
    /// for the delete confirmation's label -- never drawn to a screen, so it
    /// reaches neither a cell nor a demo's mask. The residual the crate's
    /// account-color guarantee names for exactly this reason: neither use is
    /// a display.
    pub(super) fn account_name(&self, id: AccountId) -> &str {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map_or("?", |a| a.name.as_str())
    }

    /// The account the `Tab` filter names, colored -- the border, and the
    /// title `a`/`e`/`d`'s forms default to.
    ///
    /// `Account::named`, matching the Account column: `Savings::title` is
    /// the precedent this mirrors, and naming an account by its code in the
    /// border while the column beside it spells the same account out in
    /// full would be one account said two ways in one frame.
    pub fn title(&self) -> Label {
        let mut title = match self.account {
            None => Label::plain("Funds · All"),
            Some(id) => Label::plain("Funds · ").account(Account::named(&self.accounts, id)),
        };
        if !self.search().is_empty() {
            title = title.text(format!(" · /{}", self.search()));
        }
        title
    }

    /// `Esc`: back out of the account filter to All -- a kept search is
    /// cleared first, through `search::escape_kept_filter`, the same order
    /// Ledger and Savings answer `Esc` in.
    pub fn clear_filters(&mut self) {
        self.account = None;
        self.refilter();
    }
}

impl Default for Funds {
    fn default() -> Funds {
        Funds::new()
    }
}

/// The account filter and the search, in one pass -- so the two cannot
/// narrow to different lists.
impl Search for Funds {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    /// A row answers to its ticker and to the account it sits in, matched
    /// against the account's raw stored name rather than what the screen
    /// draws -- the same split `search::searchable_amount` makes for a
    /// figure, so a needle still finds a real ticker and a real account
    /// while `mm --demo` is drawing pseudonyms over them.
    fn refilter(&mut self) {
        let matcher = self.matcher();
        let account = self.account;
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| account.is_none_or(|id| row.account_id == id))
            .filter(|(_, row)| {
                let text = format!("{} {}", row.ticker, self.account_name(row.account_id));
                matcher.matches(&text, &[])
            })
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
        self.recompute_allocation();
    }
}

impl_scroll!(Funds, visible);

/// How many glyph cells [`mix_bar`] fills for a whole fund's worth of stock
/// -- also its own column's width, so a bar never grows past it.
const MIX_BAR_WIDTH: usize = 14;

/// A 14-glyph bar, filled left to right by the stock share of a fund's
/// composition -- or the same `—` every other absence in the app draws, when
/// no mix is on record. A fund reported to hold no stock at all still draws
/// a full bar of the empty glyph, which is a different mark than the single
/// dash a fund nobody has asked SEC about draws -- the one thing this column
/// must never spell alike.
fn mix_bar(stock_percent: Option<BasisPoints>) -> String {
    let Some(percent) = stock_percent else {
        return "—".to_string();
    };
    let clamped = percent.0.clamp(0, BasisPoints::ONE.0) as u64;
    let filled = (clamped * MIX_BAR_WIDTH as u64 / BasisPoints::ONE.0 as u64) as usize;
    "█".repeat(filled) + &"░".repeat(MIX_BAR_WIDTH - filled)
}

/// The stock share, as the app's own percentage format -- or the `—` every
/// other absence in the app draws.
fn stock_percent_cell(stock_percent: Option<BasisPoints>) -> Cell<'static> {
    let text = match stock_percent {
        Some(bp) => format!("{bp}%"),
        None => "—".to_string(),
    };
    Cell::from(TextLine::from(text).right_aligned())
}

fn as_of_cell(as_of: Option<NaiveDate>) -> Cell<'static> {
    match as_of {
        Some(date) => Cell::from(date.to_string()),
        None => Cell::from("—"),
    }
}

/// How many of the screen's lines the summary panel costs the list: its
/// border, its header, and a row per class it has something to say about --
/// or none at all when it has nothing.
///
/// The first two terms are [`super::Chrome::lines`]'s arithmetic, said here
/// rather than borrowed: the panel is not a list, so it has no cursor to
/// hand `render_table` and no `Chrome` to ask. Whatever moves there has to
/// move here.
///
/// The panel is absent rather than empty when no holding on screen carries a
/// mix, which is what a database nobody has run the fetcher against looks
/// like. A bordered box of em dashes over every class would be the whole
/// screen's top third saying only that a key has not been pressed yet, and
/// the row that says it already sits under the cursor.
fn summary_lines(allocation: &Allocation) -> u16 {
    if allocation.slices.is_empty() {
        return 0;
    }
    let untargeted = allocation
        .slices
        .iter()
        .filter(|s| UNTARGETED.contains(&s.class))
        .count();
    2 + super::HEADER_LINES + (TargetClass::ALL.len() + untargeted) as u16
}

/// Class, Mix, Target, Actual, Δ -- the three classes the age rule targets,
/// then the ones it does not.
///
/// One table rather than a stack of bars: the rows are what the rule targets
/// and the bar beside them is what the portfolio actually holds, and reading
/// the second against the first is the only reason to draw either. The bar is
/// [`mix_bar`], the same fourteen glyphs the list below spends on one fund,
/// so a class's share and a holding's share are read off the same scale.
///
/// `Cash` keeps its row at zero and `Unclassified` does not have one: cash is
/// part of what the bar accounts for, while the classifier's residual is a
/// defect report, and a defect report reading "none" every time is one
/// nobody finishes reading.
fn render_summary(frame: &mut Frame, area: Rect, funds: &Funds) {
    let allocation = funds.allocation();
    let percent = |bp: Option<BasisPoints>| {
        Cell::from(
            TextLine::from(match bp {
                Some(bp) => format!("{bp}%"),
                None => "—".to_string(),
            })
            .right_aligned(),
        )
    };

    let mut rows: Vec<TableRow> = TargetClass::ALL
        .iter()
        .filter_map(|class| funds.summary_row(*class))
        .map(|row| {
            TableRow::new(vec![
                Cell::from(row.class.label()),
                Cell::from(mix_bar(Some(row.actual))),
                percent(row.target),
                percent(Some(row.actual)),
                percent(row.delta),
            ])
        })
        .collect();
    rows.extend(
        allocation
            .slices
            .iter()
            .filter(|slice| UNTARGETED.contains(&slice.class))
            .map(|slice| {
                TableRow::new(vec![
                    Cell::from(slice.class.label()),
                    Cell::from(mix_bar(Some(slice.weight))),
                    // No target and so no gap: the age rule says nothing
                    // about cash, and a target for money the classifier
                    // could not place would be a claim about nothing.
                    percent(None),
                    percent(Some(slice.weight)),
                    percent(None),
                ])
            }),
    );

    let header = TableRow::new(vec![
        Cell::from("Class"),
        Cell::from("Mix"),
        right_header("Target"),
        right_header("Actual"),
        right_header("Δ"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    // `Class` takes the single `Constraint::Min`; the three percentage
    // columns are sized for `100.00%` and, on the last, the sign a gap the
    // other way carries.
    let widths = [
        Constraint::Min(20),
        Constraint::Length(MIX_BAR_WIDTH as u16),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(8),
    ];

    let title = match allocation.coverage() {
        None => "Allocation".to_string(),
        Some(coverage) => format!("Allocation · {coverage}"),
    };
    frame.render_widget(
        Table::new(rows, widths)
            .header(header.height(super::HEADER_LINES))
            .block(
                Block::bordered()
                    .title(title)
                    .padding(Padding::right(GUTTER)),
            ),
        area,
    );
}

/// Account, Ticker, Balance, Mix, Stock%, As of, under the allocation summary.
///
/// `Account` takes the single `Constraint::Min` and absorbs the slack,
/// `tui::GUTTER` included; the other five are `Constraint::Length` sized to
/// their true content, the mix bar's own width chief among them -- it is
/// fixed and glyph-based, so it truncates from the right exactly like text.
pub(super) fn render(frame: &mut Frame, area: Rect, funds: &Funds) -> Viewport {
    let area = match summary_lines(funds.allocation()) {
        0 => area,
        lines => {
            let [summary, list] =
                Layout::vertical([Constraint::Length(lines), Constraint::Min(1)]).areas(area);
            render_summary(frame, summary, funds);
            list
        }
    };
    let visible = funds.rows();
    let rows: Vec<TableRow> = visible
        .iter()
        .map(|row| {
            TableRow::new(vec![
                account_cell(&row.account),
                Cell::from(crate::demo::text(&row.ticker).into_owned()),
                whole_amount(row.balance),
                Cell::from(mix_bar(row.stock_percent)),
                stock_percent_cell(row.stock_percent),
                as_of_cell(row.as_of),
            ])
        })
        .collect();

    let header = TableRow::new(vec![
        Cell::from("Account"),
        Cell::from("Ticker"),
        right_header("Balance"),
        Cell::from("Mix"),
        right_header("Stock%"),
        Cell::from("As of"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(7),
        Constraint::Length(11),
    ];

    render_table(
        frame,
        area,
        funds,
        Chrome::titled(super::label_line(&funds.title())).header(header),
        &widths,
        rows,
        visible.len(),
    )
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HoldingField {
    Account,
    Ticker,
    Balance,
}

impl HoldingField {
    /// Tab order, and the order the fields render in.
    pub const ORDER: [HoldingField; 3] = [
        HoldingField::Account,
        HoldingField::Ticker,
        HoldingField::Balance,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HoldingField::Account => "Account",
            HoldingField::Ticker => "Ticker",
            HoldingField::Balance => "Balance",
        }
    }
}

/// Adding or editing one holding. Backs `a` and `e`.
///
/// The account is a selector over the owner's investment accounts rather
/// than a text field, so a holding cannot be written against an account that
/// does not hold funds.
#[derive(Debug)]
pub struct HoldingForm {
    /// `Some` when editing an existing row, `None` when adding one.
    pub editing: Option<HoldingId>,
    pub focus: HoldingField,
    account: AccountChoice,
    ticker: Field,
    balance: Field,
}

impl HoldingForm {
    /// `preselected` is the account the screen is filtered to, if any, so `a`
    /// opens on the account being looked at rather than always on the first.
    pub(super) fn add(
        accounts: Vec<account::Account>,
        preselected: Option<AccountId>,
    ) -> Result<HoldingForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no investment account to hold a fund"
        );
        Ok(HoldingForm {
            editing: None,
            focus: HoldingField::Ticker,
            account: AccountChoice::preselected(accounts, preselected),
            ticker: Field::default(),
            balance: Field::default(),
        })
    }

    pub(super) fn edit(accounts: Vec<account::Account>, row: &Row) -> Result<HoldingForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no investment account to hold a fund"
        );
        Ok(HoldingForm {
            editing: Some(row.id),
            focus: HoldingField::Ticker,
            account: AccountChoice::given(accounts, row.account_id),
            ticker: Field::given(row.ticker.clone()),
            balance: Field::given(row.balance.to_string()),
        })
    }

    pub fn display(&self, field: HoldingField) -> Label {
        match field {
            HoldingField::Account => self.account.display(),
            HoldingField::Ticker => {
                Label::from(crate::demo::text(self.ticker.value()).into_owned())
            }
            HoldingField::Balance => Label::from(crate::demo::typed(self.balance.value())),
        }
    }

    /// The ticker is **uppercased here**, which is the one place the owner's
    /// typing becomes one.
    ///
    /// A ticker is a key in three places and none of them folds case:
    /// `holding`'s `UNIQUE (account_id, ticker)`, [`crate::db::holding::update`]'s
    /// duplicate guard, and `fund_mix`'s `PRIMARY KEY (ticker, asset_class)`,
    /// which is looked up by the string a holding carries. So `usm` and `USM`
    /// would be two holdings in one account, two entries in
    /// [`crate::db::holding::tickers`], and two independent compositions —
    /// with a mix fetched under one spelling never reaching a holding typed
    /// in the other. Normalising the typing is what folds the three at once,
    /// and tickers are written in capitals anyway.
    pub fn commit(&self) -> Result<(AccountId, String, Cents)> {
        let account = self.account.selected().context("no account is selected")?;
        let ticker = self.ticker.value().trim().to_uppercase();
        ensure!(!ticker.is_empty(), "ticker must not be empty");
        let balance = parse_whole_amount(self.balance.value())?;
        Ok((account.id, ticker, balance))
    }
}

impl FormFields for HoldingForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&HoldingField::ORDER, self.focus, step);
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            HoldingField::Account => Focused::Selector,
            HoldingField::Ticker => Focused::Text(&mut self.ticker),
            HoldingField::Balance => Focused::Text(&mut self.balance),
        }
    }

    fn cycle(&mut self, step: Step) {
        self.account.step(step);
    }
}

pub(super) fn render_holding(frame: &mut Frame, form: &mut HoldingForm) {
    let title = if form.editing.is_some() {
        "Edit holding — Tab field · Enter save · Esc cancel"
    } else {
        "Add holding — Tab field · Enter save · Esc cancel"
    };
    let caret = form.caret();
    let lines = field_stack(
        &HoldingField::ORDER,
        form.focus,
        caret,
        HoldingField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(frame, title, lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fund_mix::AssetClass;
    use crate::test_support::{day, investment, walk_until};
    use crate::tui::MIN_WIDTH;
    use crate::tui::form::{backspace_key, char_key};

    fn accounts() -> Vec<account::Account> {
        vec![investment(1, "BRK"), investment(2, "RET")]
    }

    fn fixture_row(
        id: i64,
        account_id: AccountId,
        accounts: &[account::Account],
        ticker: &str,
        dollars: i64,
    ) -> Row {
        Row {
            id: HoldingId(id),
            account_id,
            account: Account::named(accounts, account_id),
            ticker: ticker.to_string(),
            balance: Cents::from_dollars(dollars),
            stock_percent: None,
            as_of: None,
        }
    }

    /// Two holdings in the first account, one in the second -- so a test
    /// narrowing the `Tab` filter to one account sees the list actually
    /// shrink.
    fn funds() -> Funds {
        let all = accounts();
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![
            fixture_row(1, AccountId(1), &all, "USM", 10_000),
            fixture_row(2, AccountId(1), &all, "USB", 5_000),
            fixture_row(3, AccountId(2), &all, "ISM", 3_000),
        ]);
        funds
    }

    #[test]
    fn the_account_filter_cycles_through_each_investment_account_and_back_to_all() {
        let mut funds = funds();
        assert_eq!(funds.filter_account(), None);
        funds.next_account();
        assert_eq!(funds.filter_account(), Some(AccountId(1)));
        funds.next_account();
        assert_eq!(funds.filter_account(), Some(AccountId(2)));
        funds.next_account();
        assert_eq!(funds.filter_account(), None, "Tab must return to All");
    }

    #[test]
    fn back_tab_cycles_the_account_filter_the_other_way() {
        let mut funds = funds();
        funds.previous_account();
        assert_eq!(funds.filter_account(), Some(AccountId(2)));
        funds.previous_account();
        assert_eq!(funds.filter_account(), Some(AccountId(1)));
        funds.previous_account();
        assert_eq!(funds.filter_account(), None, "BackTab must return to All");
    }

    #[test]
    fn clear_filters_returns_the_account_filter_to_all() {
        let mut funds = funds();
        funds.next_account();
        assert_eq!(funds.filter_account(), Some(AccountId(1)));

        funds.clear_filters();
        assert_eq!(funds.filter_account(), None);
        assert_eq!(funds.rows().len(), 3, "the full list did not come back");
    }

    #[test]
    fn narrowing_to_one_account_shrinks_the_list_to_just_its_holdings() {
        let mut funds = funds();
        funds.next_account();
        let tickers: Vec<&str> = funds.rows().iter().map(|r| r.ticker.as_str()).collect();
        assert_eq!(tickers, vec!["USM", "USB"]);
    }

    #[test]
    fn the_search_matches_a_holdings_ticker() {
        let mut funds = funds();
        funds.begin_search();
        for c in "USB".chars() {
            funds.push_search(c);
        }
        let tickers: Vec<&str> = funds.rows().iter().map(|r| r.ticker.as_str()).collect();
        assert_eq!(tickers, vec!["USB"]);
    }

    /// "Long Haul" is `RET`'s fixture name, so a needle over the account
    /// reaches a holding the ticker alone would not.
    #[test]
    fn the_search_matches_the_account_a_holding_sits_in() {
        let mut funds = funds();
        funds.begin_search();
        for c in "Long".chars() {
            funds.push_search(c);
        }
        let tickers: Vec<&str> = funds.rows().iter().map(|r| r.ticker.as_str()).collect();
        assert_eq!(tickers, vec!["ISM"]);
    }

    #[test]
    fn a_position_past_the_end_of_a_shrinking_filter_moves_into_bounds() {
        use crate::tui::cursor::Scroll;
        let mut funds = funds();
        funds.select_last();
        assert_eq!(funds.selected_index(), 2);

        funds.begin_search();
        for c in "USM".chars() {
            funds.push_search(c);
        }
        assert_eq!(funds.selected_index(), 0);
        assert_eq!(funds.selected().unwrap().ticker, "USM");
    }

    /// A fund nobody has asked SEC about and a fund confirmed to hold no
    /// stock at all must never draw alike: the first is a question, the
    /// second is an answer.
    #[test]
    fn a_fund_never_fetched_and_a_fund_confirmed_to_hold_no_stock_draw_differently() {
        let all = accounts();
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![
            fixture_row(1, AccountId(1), &all, "USM", 10_000),
            Row {
                stock_percent: Some(BasisPoints::ZERO),
                as_of: Some(day(2026, 6, 30)),
                ..fixture_row(2, AccountId(1), &all, "USB", 5_000)
            },
        ]);

        let lines = drawn(&funds, 8);
        let never_fetched = lines
            .iter()
            .find(|l| l.contains("USM"))
            .expect("the never-fetched row is drawn");
        assert!(never_fetched.contains('—'), "{never_fetched:?}");
        assert!(!never_fetched.contains('░'), "{never_fetched:?}");

        let zero_stock = lines
            .iter()
            .find(|l| l.contains("USB"))
            .expect("the zero-stock row is drawn");
        assert!(
            zero_stock.contains(&"░".repeat(MIX_BAR_WIDTH)),
            "a fully unfilled bar should draw, not a dash: {zero_stock:?}"
        );
    }

    /// The portfolio the app fixture builds, at view level: `USM` never
    /// fetched, the bond fund and the international fund both priced.
    fn funds_with_mixes() -> Funds {
        let all = accounts();
        let slice = |class, weight| Slice {
            class,
            weight: BasisPoints(weight),
        };
        let bond = vec![
            slice(AssetClass::UsBond, 7_000),
            slice(AssetClass::IntlBond, 2_500),
            slice(AssetClass::Cash, 500),
        ];
        let intl = vec![
            slice(AssetClass::IntlStock, 9_500),
            slice(AssetClass::Cash, 500),
        ];
        let filed = day(2026, 6, 30);

        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_targets(crate::calc::fund::targets(Some(48), BasisPoints(4_000)));
        funds.set_mixes(HashMap::from([
            ("USB".to_string(), bond),
            ("ISM".to_string(), intl),
        ]));
        funds.set_rows(vec![
            fixture_row(1, AccountId(1), &all, "USM", 10_000),
            Row {
                stock_percent: Some(BasisPoints::ZERO),
                as_of: Some(filed),
                ..fixture_row(2, AccountId(1), &all, "USB", 5_000)
            },
            Row {
                stock_percent: Some(BasisPoints(9_500)),
                as_of: Some(filed),
                ..fixture_row(3, AccountId(2), &all, "ISM", 3_000)
            },
        ]);
        funds
    }

    fn drawn(funds: &Funds, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), funds);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// The summary sits above the list, so it spends the list's height as
    /// well as the screen's width. Both are checked at once and against
    /// `MIN_WIDTH` rather than a number: a right-aligned percentage one
    /// column short loses its *leading* digits, which reads as a smaller
    /// share rather than as a truncation.
    #[test]
    fn the_summary_and_the_list_both_fit_the_narrowest_terminal() {
        let funds = funds_with_mixes();
        let lines = drawn(&funds, 24);

        // $5,000 of a 70/25/5 bond fund and $3,000 of a 95/5 international
        // fund, over the $8,000 the two of them come to -- `USM` having no
        // mix on record is outside the denominator entirely.
        let bonds = lines
            .iter()
            .find(|l| l.contains("Bonds"))
            .expect("the Bonds row is drawn");
        let header = lines
            .iter()
            .find(|l| l.contains("Target"))
            .expect("the summary header is drawn");
        let header_ends = super::super::ends_in_order(header, &["Target", "Actual", "Δ"]);
        let row_ends = super::super::ends_in_order(bonds, &["18.00%", "59.37%", "-41.37%"]);
        assert_eq!(
            header_ends, row_ends,
            "the summary's columns over {bonds:?}"
        );

        // The class the unfetched fund would have carried, sitting at nothing
        // against a target of nearly half the portfolio.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("U.S. Stock") && l.contains("49.20%") && l.contains("0.00%")),
            "{lines:#?}"
        );
        // Cash is accounted for and carries no target; the classifier placed
        // everything, so there is no Unclassified row at all.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Cash") && l.contains("5.00%")),
            "{lines:#?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("Unclassified")),
            "{lines:#?}"
        );

        // And the list still draws under it, header and every holding.
        assert!(lines.iter().any(|l| l.contains("Account")), "{lines:#?}");
        for ticker in ["USM", "USB", "ISM"] {
            assert!(
                lines.iter().any(|l| l.contains(ticker)),
                "the summary crowded {ticker} off the list: {lines:#?}"
            );
        }
    }

    /// The panel is a claim about the whole list, so it says when it is not
    /// one: `BRK` holds the fund nobody has fetched, `RET` holds only a fund
    /// that has been.
    #[test]
    fn the_summary_title_names_its_coverage_only_while_something_is_missing() {
        let mut funds = funds_with_mixes();
        assert!(
            drawn(&funds, 24)
                .iter()
                .any(|l| l.contains("Allocation · 2 of 3 holdings")),
            "the partial coverage went unsaid"
        );

        walk_until!(
            funds.filter_account() == Some(AccountId(2)),
            funds.next_account()
        );
        let lines = drawn(&funds, 24);
        assert!(lines.iter().any(|l| l.contains("Allocation")), "{lines:#?}");
        assert!(
            !lines.iter().any(|l| l.contains("holdings")),
            "a complete summary counted itself: {lines:#?}"
        );
    }

    /// Every database starts with no composition on record, and a bordered
    /// box of em dashes over the top third of the screen would say only that
    /// a key has not been pressed yet.
    #[test]
    fn a_portfolio_with_no_mix_on_record_draws_no_summary_at_all() {
        let lines = drawn(&funds(), 24);
        assert!(
            !lines.iter().any(|l| l.contains("Allocation")),
            "{lines:#?}"
        );
        assert!(lines[0].contains("Funds"), "the list took the whole area");
    }

    #[test]
    fn the_right_aligned_headers_end_where_their_own_columns_do() {
        let all = vec![investment(1, "BRK")];
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![Row {
            stock_percent: Some(BasisPoints(6_234)),
            as_of: Some(day(2026, 6, 30)),
            ..fixture_row(1, AccountId(1), &all, "USM", 100)
        }]);

        // No mix on record, so no summary panel: the list still opens at the
        // top of the area, with its header on the line under the border.
        let lines = drawn(&funds, 6);
        let header = &lines[1];
        let row = &lines[2];

        let header_ends = super::super::ends_in_order(header, &["Balance", "Stock%"]);
        let row_ends = super::super::ends_in_order(row, &["100", "62.34%"]);
        assert_eq!(header_ends[0], row_ends[0], "Balance over {row:?}");
        assert_eq!(header_ends[1], row_ends[1], "Stock% over {row:?}");
    }

    fn focused(form: &mut HoldingForm, field: HoldingField) {
        walk_until!(form.focus == field, form.next_field());
    }

    fn typed(form: &mut HoldingForm, field: HoldingField, text: &str) {
        focused(form, field);
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    #[test]
    fn a_holding_form_commits_the_account_ticker_and_balance_typed() {
        let mut form = HoldingForm::add(accounts(), Some(AccountId(2))).unwrap();
        typed(&mut form, HoldingField::Ticker, "USM");
        typed(&mut form, HoldingField::Balance, "10000");

        let (account_id, ticker, balance) = form.commit().unwrap();
        assert_eq!(account_id, AccountId(2));
        assert_eq!(ticker, "USM");
        assert_eq!(balance, Cents::from_dollars(10_000));
    }

    #[test]
    fn a_holding_form_refuses_an_empty_ticker() {
        let mut form = HoldingForm::add(accounts(), None).unwrap();
        typed(&mut form, HoldingField::Balance, "100");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("ticker"), "{err}");
    }

    /// Goal and fund figures alike are typed in whole dollars -- a typo
    /// rather than a deliberate cents figure -- and `parse_whole_amount`
    /// refuses rather than rounds.
    #[test]
    fn a_holding_form_refuses_a_balance_carrying_cents() {
        let mut form = HoldingForm::add(accounts(), None).unwrap();
        typed(&mut form, HoldingField::Ticker, "USM");
        typed(&mut form, HoldingField::Balance, "100.50");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("100.50"), "{err}");
    }

    #[test]
    fn a_holding_form_with_no_investment_account_to_write_to_is_refused() {
        let err = HoldingForm::add(Vec::new(), None).unwrap_err();
        assert!(err.to_string().contains("investment account"), "{err}");
    }

    #[test]
    fn editing_a_holding_prefills_its_account_ticker_and_balance() {
        let all = accounts();
        let row = fixture_row(7, AccountId(2), &all, "ISM", 3_000);
        let mut form = HoldingForm::edit(all, &row).unwrap();

        assert_eq!(form.editing, Some(HoldingId(7)));
        assert_eq!(
            form.display(HoldingField::Account).plain_text(),
            "RET — Long Haul"
        );
        assert_eq!(form.display(HoldingField::Ticker).plain_text(), "ISM");
        assert_eq!(form.display(HoldingField::Balance).plain_text(), "3,000.00");

        focused(&mut form, HoldingField::Balance);
        for _ in 0.."3,000.00".len() {
            form.edit(backspace_key());
        }
        for c in "4000".chars() {
            form.edit(char_key(c));
        }
        let (_, _, balance) = form.commit().unwrap();
        assert_eq!(balance, Cents::from_dollars(4_000));
    }
}
