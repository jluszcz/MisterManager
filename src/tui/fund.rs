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
use crate::allocation::{self, Allocation, Class, Held, SummaryRow};
use crate::calc::fund::Targets;
use crate::db::account;
use crate::db::account::TaxTreatment;
use crate::db::fund_mix::Slice;
use crate::db::{AccountId, HoldingId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Cell, Padding, Paragraph, Row as TableRow, Table};
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
    /// What the fund calls itself in its own filing, or `None` for a ticker
    /// nobody has fetched -- the same `None` as `stock_percent` and `as_of`
    /// beside it, and for the same reason. A mix written before there was a
    /// column to hold a name reads the same way, and the next `g` fills it.
    pub name: Option<String>,
    pub balance: Cents,
    pub stock_percent: Option<BasisPoints>,
    pub as_of: Option<NaiveDate>,
    /// How the account holding it is taxed.
    ///
    /// `Some` for every account a holding can live in -- the schema's paired
    /// `CHECK` puts a treatment on exactly the investment accounts -- and the
    /// `Option` is the shape `db::account` hands over rather than a state
    /// this screen can reach.
    pub tax_treatment: Option<TaxTreatment>,
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

    /// One class as the summary draws it, or `None` when there is no
    /// composition to draw it against.
    ///
    /// Absent rather than zeroed, for the reason [`Row::stock_percent`] is:
    /// a portfolio nobody has fetched a mix for holds an unknown share of
    /// bonds, not none.
    pub fn summary_row(&self, class: Class) -> Option<SummaryRow> {
        (!self.allocation.slices.is_empty())
            .then(|| SummaryRow::new(class, &self.allocation.slices, self.targets))
    }

    pub(super) fn allocation(&self) -> &Allocation {
        &self.allocation
    }

    fn recompute_allocation(&mut self) {
        let allocation = {
            let held: Vec<Held<'_>> = self
                .visible
                .iter()
                .map(|i| &self.rows[*i])
                .map(|row| Held {
                    balance: row.balance,
                    treatment: row.tax_treatment,
                    mix: self.mixes.get(&row.ticker).map(Vec::as_slice),
                })
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

    /// What the holdings on screen come to.
    ///
    /// **A sum over the rows, where the ledgers' own total deliberately is
    /// not.** `Ledger::set_total` takes a figure `App` queried, because a
    /// ledger's rows are a window onto a dated account and a balance is a
    /// `SUM(cents) WHERE date <= today` that no window may narrow. A holding
    /// carries one typed, undated balance and nothing sums them anywhere
    /// else -- an investment account is banded off the Overview precisely so
    /// that no such sum reaches Net -- so here the rows *are* the figure, and
    /// it answers for whatever the account filter and the search have left,
    /// exactly as the allocation panel above does.
    pub fn total(&self) -> Cents {
        self.rows().iter().map(|row| row.balance).sum()
    }

    /// The oldest filing behind the holdings on screen, or `None` while
    /// nothing on screen has been fetched.
    ///
    /// The *oldest*, because it is the one that bounds the rest: every
    /// figure the summary above states is as current as its stalest input,
    /// and a stamp quoting the newest would say the portfolio is fresher
    /// than it is. A holding with no filing at all is not a date and cannot
    /// make one older -- what it costs the reading is
    /// [`Allocation::coverage`], which is already drawn in the panel's own
    /// title.
    ///
    /// Over the rows the filters leave rather than every row fetched, the
    /// same narrowing [`Funds::recompute_allocation`] answers to: the stamp
    /// sits in this list's border, so it is a claim about this list.
    pub fn as_of(&self) -> Option<NaiveDate> {
        self.rows().iter().filter_map(|row| row.as_of).min()
    }

    /// The account the `Tab` filter names, colored -- the border, and the
    /// title `a`/`e`/`d`'s forms default to.
    ///
    /// `Account::named`, matching the Account column: `Savings::title` is
    /// the precedent this mirrors, and naming an account by its code in the
    /// border while the column beside it spells the same account out in
    /// full would be one account said two ways in one frame.
    ///
    /// The filing stamp comes last of the terms here, past the `/` filter,
    /// because the two are different kinds of thing: everything before it
    /// says which rows these are, and it says how old they are. It is in the
    /// border rather than a column for the reason the column was dropped --
    /// the date is one fact about the fetch, not one per holding, and
    /// spending a column on it repeated the same day down the whole list.
    ///
    /// The total is not here but in [`title_line`], for [`Ledger::title`]'s
    /// reason: it takes a color no [`Label`] segment can carry. That also
    /// settles the order -- the stamp reads *into* the figure rather than
    /// out of it, and a date sitting after a balance would read as the day
    /// that balance was struck, which is a claim nothing in this app makes
    /// about a typed holding.
    ///
    /// [`Ledger::title`]: super::ledger::Ledger::title
    pub fn title(&self) -> Label {
        let mut title = match self.account {
            None => Label::plain("Funds · All"),
            Some(id) => Label::plain("Funds · ").account(Account::named(&self.accounts, id)),
        };
        if !self.search().is_empty() {
            title = title.text(format!(" · /{}", self.search()));
        }
        match self.as_of() {
            Some(date) => title.text(format!(" · as of {date}")),
            None => title,
        }
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

    /// A row answers to its ticker, to the fund that ticker names, and to the
    /// account it sits in -- matched against the account's raw stored name
    /// rather than what the screen draws, the same split
    /// `search::searchable_amount` makes for a figure, so a needle still
    /// finds a real ticker and a real account while `mm --demo` is drawing
    /// pseudonyms over them.
    ///
    /// The fund name is in the needle's reach because it is on screen:
    /// a column a reader can see and cannot search is a column that reads as
    /// broken, and `bond` finding every bond fund is the search this one
    /// makes possible. A row with no name yet answers to the other two, as
    /// it did before there was a name to answer to.
    fn refilter(&mut self) {
        let matcher = self.matcher();
        let account = self.account;
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| account.is_none_or(|id| row.account_id == id))
            .filter(|(_, row)| {
                // Joined rather than interpolated, so a row with no name yet
                // reads as the two words it has: the needle is matched as a
                // substring, and a doubled space between the ticker and the
                // account is a needle spanning the two that no longer finds
                // a row it used to.
                let text = [
                    Some(row.ticker.as_str()),
                    row.name.as_deref(),
                    Some(self.account_name(row.account_id)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<&str>>()
                .join(" ");
                matcher.matches(&text, &[])
            })
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
        self.recompute_allocation();
    }
}

impl_scroll!(Funds, visible);

/// How wide the `Fund` column is.
///
/// The one column on any screen whose content comes off a filing rather than
/// out of a closed set, so it cannot be measured the way `accounts::widths`
/// measures its five. What it is instead is **everything the row can spare**:
/// at [`super::MIN_WIDTH`] the five fixed columns and the chrome leave
/// sixty-eight for this and `Account` together, and `Account`'s own
/// `Constraint::Min` claims twenty of them.
///
/// Wide rather than narrow because of *where* a fund name carries its
/// meaning. `... Target Retirement 2045 Fund` differs from the row above it
/// in the year, four characters from the end -- so a column cut anywhere
/// short of the whole draws a column of rows that all read alike, which is
/// the one thing this column exists not to do. Every issuer's own name leads
/// every one of those rows and is the least informative word in it, which is
/// what makes them so long; dropping it would let this be narrow, and the
/// words that do the dropping are institutions this repository may not name.
/// So the screen pays for it in width instead, and `src/fund_label.rs` says
/// the same thing from the other end.
///
/// It can still truncate, on a name longer than this or a terminal narrower
/// than `MIN_WIDTH`, and this is the column where that is affordable: it is
/// left-aligned prose, so a cut takes the tail rather than the leading digits
/// a right-aligned figure would lose. `Ticker` beside it is the row's actual
/// identity and is never cut.
const FUND_NAME_WIDTH: u16 = 48;

/// What a fund calls itself, or the `—` every other absence in the app draws.
///
/// Masked through `crate::demo::text` beside the ticker it names: a fund's
/// name identifies a real holding at least as plainly as its symbol does, so
/// a demo drawing one and hiding the other would hide neither.
fn fund_name_cell(name: Option<&str>) -> Cell<'static> {
    Cell::from(match name {
        Some(name) => crate::demo::text(name).into_owned(),
        None => "—".to_string(),
    })
}

/// The stock share to the nearest whole percent -- or the `—` every other
/// absence in the app draws.
///
/// Whole where the summary above spends two decimals, and the two are
/// answering different questions. A summary share is read *against* the
/// target beside it, where a point is a gap worth acting on; this column is
/// read *down*, one line per holding, and what the reader is doing there is
/// telling the bond fund from the stock fund. The hundredths are the
/// filing's own rounding, and a column of them is ten figures of noise over
/// the one digit that separates 0% from 99%. [`BasisPoints::whole_percent`]
/// is the spelling, on the type for the reason the other one is.
fn stock_percent_cell(stock_percent: Option<BasisPoints>) -> Cell<'static> {
    let text = match stock_percent {
        Some(bp) => format!("{}%", bp.whole_percent()),
        None => "—".to_string(),
    };
    Cell::from(TextLine::from(text).right_aligned())
}

/// How many of the screen's lines the summary panel costs the list: its
/// border, its header, a row per [`Class`], the gap, and a line per bar [`bars`]
/// has to draw -- or none at all when it has nothing to say.
///
/// The bar count is read from the same function that draws them, rather than
/// being a constant either could be wrong about: how many there are depends
/// on how many treatments hold anything, so a filter narrowing to one
/// account shrinks the panel and hands the list the lines back.
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
    2 + super::HEADER_LINES + Class::ALL.len() as u16 + BAR_GAP + bars(allocation).len() as u16
}

/// The blank line between the last class row and the first bar.
///
/// The table is figures and the bars are pictures of those figures, and with
/// nothing between them `Other` and `Total` read as two more rows of one
/// list -- the left-hand column of labels running straight on. One line is
/// what separates two blocks inside a panel that has only one border to
/// spend.
const BAR_GAP: u16 = 1;

/// How many glyph cells the four-class bar spends.
///
/// Fixed rather than the panel's width: a glyph run truncates from the right
/// like text, where a cell sized off the terminal would reflow every segment
/// as the window moved. Forty is what makes the smallest segment worth
/// drawing -- one glyph is 2.5%, and a sliver below that is the thing the bar
/// exists to show.
const SUMMARY_BAR_WIDTH: usize = 40;

/// What every bar is named on its left, and how wide that column is.
///
/// The widest of the four labels, so the bars start at one column and can be
/// read down as well as across -- four bars whose left edges disagreed would
/// be four scales rather than one.
const BAR_LABEL_WIDTH: usize = 12;

/// What the four leave over -- nothing, on the `Total` bar, now that
/// [`Class::Other`] is one of them. It is the track the segments are laid on,
/// so a bar that cannot fill its width still draws to the same length: a
/// treatment holding nothing is a row of this, which is the honest drawing of
/// a pot with nothing in it.
const BAR_REST: &str = "\u{2591}";

/// One bar's segments, as cell counts that sum to [`SUMMARY_BAR_WIDTH`] or
/// less.
///
/// Cut at the *cumulative* share and differenced, rather than each rounded on
/// its own: that is what keeps them summing to the bar's own width, so the
/// tail is exactly what the classes leave rather than a glyph of accumulated
/// rounding.
///
/// `denominator` is what the shares are *of*, and it is what makes one
/// function draw both kinds of bar. The `Total` bar passes
/// [`BasisPoints::ONE`], its slices already being shares of the whole; a
/// treatment passes what that treatment holds, so its own four classes fill
/// the width and can be read against the bar above it. A denominator of
/// nothing is a treatment holding nothing, and draws no segments at all.
fn segment_widths(shares: [BasisPoints; Class::ALL.len()], denominator: BasisPoints) -> Vec<usize> {
    let width = SUMMARY_BAR_WIDTH as i64;
    if denominator.0 <= 0 {
        return vec![0; Class::ALL.len()];
    }
    let mut widths = Vec::with_capacity(Class::ALL.len());
    let (mut cumulative, mut drawn) = (0i64, 0i64);
    for share in shares {
        cumulative += share.0;
        // Monotonic whatever the weights are: a mix that over-foots puts the
        // classes past the denominator between them, and a bar that ran
        // backwards would draw a segment over the one before it.
        let end = (cumulative.clamp(0, denominator.0) * width / denominator.0).max(drawn);
        widths.push((end - drawn) as usize);
        drawn = end;
    }
    widths
}

/// What one segment reads inside itself: its own share as a whole percent, or
/// nothing at all.
///
/// **Nothing rather than something abbreviated.** A share is only written
/// where the segment can hold it with a column of space on either side, which
/// is what stops a figure touching the segment next to it and reading as part
/// of it. Below that the segment says what it has always said -- its length,
/// against the three beside it -- and the row above the bars states every
/// figure exactly.
///
/// Centred in what is left, with the odd column going to the right, so a
/// figure sits where the eye expects rather than against one edge.
fn segment_text(width: usize, share: Option<String>) -> String {
    let Some(share) = share.filter(|s| s.chars().count() + 2 * BAR_TEXT_PADDING <= width) else {
        return " ".repeat(width);
    };
    let spare = width - share.chars().count();
    format!(
        "{}{share}{}",
        " ".repeat(spare / 2),
        " ".repeat(spare - spare / 2)
    )
}

/// Columns of clear space a share needs on each side of it to be written
/// inside its own segment.
///
/// One. It is not whitespace for its own sake: two segments are told apart by
/// their colors, and a figure flush against the join reads as belonging to
/// whichever of the two the eye lands on first.
const BAR_TEXT_PADDING: usize = 1;

/// One bar: one segment per [`Class`], in the order the rows above are
/// listed and in the color those rows' labels carry, so a segment and its row
/// are the same statement.
///
/// **Drawn as background rather than as a glyph run.** A `\u{2588}` cannot have a
/// figure written inside it, and the figures are the whole reason a reader
/// can take a share off the bar instead of tracking a segment up to the
/// table. What is lost is nothing: a filled background and a run of full
/// blocks are the same rectangle, and the track beside them keeps its own
/// glyph, being the one part of the bar that is not a quantity.
fn summary_bar(
    shares: [BasisPoints; Class::ALL.len()],
    denominator: BasisPoints,
) -> Vec<Span<'static>> {
    let widths = segment_widths(shares, denominator);
    let mut spans = Vec::new();
    for ((class, share), width) in Class::ALL.iter().zip(shares).zip(&widths) {
        if *width == 0 {
            continue;
        }
        let percent = (denominator.0 > 0)
            .then(|| BasisPoints(share.0 * BasisPoints::ONE.0 / denominator.0))
            .map(|share| format!("{}%", share.whole_percent()));
        spans.push(Span::styled(
            segment_text(*width, percent),
            Style::default()
                .bg(super::style::class(*class))
                .fg(super::style::on_class(*class)),
        ));
    }
    let drawn: usize = widths.iter().sum();
    spans.push(Span::raw(BAR_REST.repeat(SUMMARY_BAR_WIDTH - drawn)));
    spans
}

/// A bar with its name in front of it, as one line.
fn labelled_bar(bar: &Bar) -> TextLine<'static> {
    let label = bar.label;
    let mut spans = vec![Span::raw(format!("{label:<BAR_LABEL_WIDTH$} "))];
    spans.extend(summary_bar(bar.shares, bar.denominator));
    TextLine::from(spans)
}

/// One bar: what it is called, the four shares it draws, and what they are
/// shares *of*.
struct Bar {
    label: &'static str,
    shares: [BasisPoints; Class::ALL.len()],
    denominator: BasisPoints,
}

/// Every bar the panel draws, in order.
///
/// **A treatment holding nothing gets no bar.** An empty row of track states
/// a fact the tax column above it already states, and it costs a line of a
/// panel the list is paying for -- and it is the ordinary case the moment
/// `Tab` narrows to one account, an account having exactly one treatment.
/// Three rows of track under the one bar that says anything is the screen
/// answering a question about pots that are not on it.
///
/// **`Total` is drawn only when more than one treatment is left.** With one,
/// it is that treatment's bar with a different word in front of it: the same
/// four shares over the same denominator, since the whole of what is on
/// screen is what that pot holds. Two identical bars is the reader being
/// asked to compare something with itself.
///
/// Nothing qualifying at all leaves `Total` alone. That is a portfolio whose
/// holdings sit in accounts stating no treatment, which `apportion` puts in
/// the classes and in no column and the schema's paired `CHECK` makes
/// unreachable through the app -- so it is the drawing of a database that
/// should not exist, and it draws what it can rather than nothing.
fn bars(allocation: &Allocation) -> Vec<Bar> {
    let total = Bar {
        label: "Total",
        shares: Class::ALL.map(|class| class.actual(&allocation.slices)),
        denominator: BasisPoints::ONE,
    };
    let held: Vec<Bar> = TaxTreatment::ALL
        .iter()
        .map(|treatment| Bar {
            label: treatment.label(),
            shares: Class::ALL.map(|class| allocation.class_in(class, *treatment)),
            denominator: allocation.treatment_total(*treatment),
        })
        .filter(|bar| bar.denominator > BasisPoints::ZERO)
        .collect();

    match held.len() {
        0 => vec![total],
        1 => held,
        _ => std::iter::once(total).chain(held).collect(),
    }
}

/// Class, Target, Actual, Δ, with the four bars beneath.
///
/// One table rather than a stack of bars: the rows are what the age rule
/// targets and the bar is what the portfolio holds, and reading the second
/// against the first is the only reason to draw either.
///
/// **The bars take lines of their own rather than a column of the table.** A
/// fourteen-glyph cell splits four ways into three glyphs each, which cannot
/// show a bond split at all, and each bar is *one* statement about a whole
/// portfolio where a table column is one per row.
///
/// **There are four of them, and the three under `Total` are the tax columns
/// asked the other way round.** A column states each treatment's classes
/// against the whole portfolio, which is what makes the grid foot in both
/// directions; it is also what stops a reader comparing one treatment's shape
/// to another's without doing the division themselves. Each bar does that
/// division -- normalised to what its own pot holds -- so "is the Roth
/// stock-heavier than the 401k" is a question answered by looking.
///
/// **The class names carry the bar's colors, and the bar has no legend.** A
/// legend is a third copy of the four words -- the rows already name them,
/// in the order the segments run -- and it cost the line beside the bar,
/// which on a narrow terminal is the first thing to truncate. Tinting the
/// label instead pairs a segment with its row directly, so the name a reader
/// looks up is the one carrying the color.
///
/// **Every class keeps its row, whatever it holds.** The four are the
/// vocabulary the age rule is stated in, so a zero is an answer rather than
/// an absence -- and `Other` is drawn even at nothing, cash and the
/// classifier's residual now sharing it.
///
/// The three tax columns are a second question about the same portfolio:
/// not what it holds but where it is held, which is the one thing the age
/// rule has no opinion on and the owner has the most control over. They are
/// columns rather than a table of their own because each is a share of the
/// same denominator as `Actual` beside it -- a row reads across to its own
/// total, and a column reads down to what that treatment holds.
fn render_summary(frame: &mut Frame, area: Rect, funds: &Funds) {
    let allocation = funds.allocation();
    let share = |bp: Option<BasisPoints>| {
        TextLine::from(match bp {
            Some(bp) => format!("{bp}%"),
            None => "—".to_string(),
        })
        .right_aligned()
    };
    let percent = |bp: Option<BasisPoints>| Cell::from(share(bp));
    // Red where the portfolio is short of what the rule asks and plain where
    // it is not: a gap is the only thing in this table a reader has to act
    // on, and `palette::NEGATIVE` is what every other shortfall in the app
    // already spells it with. `delta` is `actual - target`, so short is the
    // negative one.
    //
    // Through `super::tinted` rather than `Cell::style`, the rule every
    // colored cell in the crate answers to: a cell's own style covers its
    // padding as well as its text, which on the cursor row turns into a
    // solid block the full width of the column.
    let delta = |bp: Option<BasisPoints>| {
        let color = bp.filter(|bp| bp.0 < 0).map(|_| super::style::negative());
        super::tinted(share(bp), color)
    };

    let rows: Vec<TableRow> = Class::ALL
        .iter()
        .filter_map(|class| funds.summary_row(*class))
        .map(|row| {
            let mut cells = vec![
                // The label is what names the bar's segment below, so it
                // carries that segment's color -- and it carries it the way
                // every other colored cell does, on the text rather than on
                // the cell.
                super::tinted(
                    TextLine::from(row.class.label()),
                    Some(super::style::class(row.class)),
                ),
                percent(row.target),
                percent(Some(row.actual)),
                delta(row.delta),
            ];
            cells.extend(
                TaxTreatment::ALL
                    .iter()
                    .map(|treatment| percent(Some(allocation.class_in(row.class, *treatment)))),
            );
            TableRow::new(cells)
        })
        .collect();

    let mut header_cells = vec![
        Cell::from("Class"),
        right_header("Target"),
        right_header("Actual"),
        right_header("Δ"),
    ];
    header_cells.extend(
        TaxTreatment::ALL
            .iter()
            .map(|treatment| right_header(treatment.label())),
    );
    let header = TableRow::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    // `Class` takes the single `Constraint::Min`; the percentage columns are
    // sized for `100.00%` and, on the Δ, the sign a gap the other way
    // carries. The three tax columns are sized for their own headings, which
    // are wider than the figures under them.
    let mut widths = vec![
        Constraint::Min(20),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(8),
    ];
    widths.extend(
        TaxTreatment::ALL
            .iter()
            .map(|treatment| Constraint::Length(treatment.label().len() as u16 + 1)),
    );

    let title = match allocation.coverage() {
        None => "Allocation".to_string(),
        Some(coverage) => format!("Allocation · {coverage}"),
    };
    // The block is drawn first and the two halves into what it leaves, rather
    // than handed to the table: the bar is not a row, and a table owning the
    // border would have no line below itself to put one on.
    let block = Block::bordered()
        .title(title)
        .padding(Padding::right(GUTTER));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Each normalised to what its own pot holds: the table above compares
    // them against one denominator and these compare them against
    // themselves, which is the reading a column of figures cannot give.
    let drawn = bars(allocation);
    let [table_area, _gap, bar_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(BAR_GAP),
        Constraint::Length(drawn.len() as u16),
    ])
    .areas(inner);
    frame.render_widget(
        Table::new(rows, widths).header(header.height(super::HEADER_LINES)),
        table_area,
    );
    frame.render_widget(
        Paragraph::new(drawn.iter().map(labelled_bar).collect::<Vec<TextLine>>()),
        bar_area,
    );
}

/// The fewest lines the list is left before the summary gives up the screen
/// to it: the list's own chrome -- two border lines and a header -- and the
/// one row a cursor has to be able to sit on.
///
/// The panel is a fixed height and the list takes what is left, so on a
/// short enough terminal the list is left nothing, and there is no key that
/// hides the panel to get it back. A summary of rows the reader cannot reach
/// is the wrong half to keep: the rows are what every other key on this
/// screen acts on, and the summary is a reading of them. Nothing here is
/// state, so the panel returns the moment the window does.
const LIST_FLOOR: u16 = 2 + super::HEADER_LINES + 1;

/// Account, Ticker, Fund, Balance, Stock%, Tax, under the allocation summary.
///
/// `Account` takes the single `Constraint::Min` and absorbs the slack,
/// `tui::GUTTER` included; the other five are `Constraint::Length` sized to
/// their true content, the mix bar's own width chief among them -- it is
/// fixed and glyph-based, so it truncates from the right exactly like text.
/// [`Funds::title`]'s spans, and then the balance those filters leave.
///
/// The ledgers' own title does the same and for the same reason: a figure
/// carries [`super::style::amount_color`], which a bare [`Label`] cannot --
/// only an account segment takes a color there. It goes last, so it sits in
/// the same place whether or not a search is running and whether or not
/// anything has a filing behind it.
///
/// Whole dollars, through [`super::whole_money_span`], because every balance
/// in the column below is a [`super::whole_amount`]: a title carrying cents
/// would be the one figure on the screen at another precision, and it would
/// not foot against the rows a reader adds up by eye.
fn title_line(funds: &Funds) -> TextLine<'static> {
    let mut spans = super::label_line(&funds.title()).spans;
    spans.push(Span::raw(" · "));
    spans.push(super::whole_money_span(funds.total()));
    TextLine::from(spans)
}

pub(super) fn render(frame: &mut Frame, area: Rect, funds: &Funds) -> Viewport {
    let area = match summary_lines(funds.allocation()) {
        lines if lines > 0 && area.height >= lines + LIST_FLOOR => {
            let [summary, list] =
                Layout::vertical([Constraint::Length(lines), Constraint::Min(1)]).areas(area);
            render_summary(frame, summary, funds);
            list
        }
        _ => area,
    };
    let visible = funds.rows();
    let rows: Vec<TableRow> = visible
        .iter()
        .map(|row| {
            TableRow::new(vec![
                account_cell(&row.account),
                Cell::from(crate::demo::text(&row.ticker).into_owned()),
                fund_name_cell(row.name.as_deref()),
                whole_amount(row.balance),
                stock_percent_cell(row.stock_percent),
                super::tax_treatment_cell(row.tax_treatment),
            ])
        })
        .collect();

    let header = TableRow::new(vec![
        Cell::from("Account"),
        Cell::from("Ticker"),
        Cell::from("Fund"),
        right_header("Balance"),
        right_header("Stock%"),
        Cell::from("Tax"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let widths = [
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Length(FUND_NAME_WIDTH),
        Constraint::Length(14),
        Constraint::Length(7),
        super::label_width("Tax", TaxTreatment::ALL.iter().map(|t| t.label())),
    ];

    render_table(
        frame,
        area,
        funds,
        Chrome::titled(title_line(funds)).header(header),
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
            name: Some(crate::test_support::fund_name(ticker).to_string()),
            balance: Cents::from_dollars(dollars),
            stock_percent: None,
            as_of: None,
            tax_treatment: Some(TaxTreatment::Taxable),
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

        let zero_stock = lines
            .iter()
            .find(|l| l.contains("USB"))
            .expect("the zero-stock row is drawn");
        assert!(
            zero_stock.contains("0%"),
            "a fund reported to hold no stock should say so, not dash: {zero_stock:?}"
        );
        assert!(
            !zero_stock.contains('—'),
            "and must not spell it the way an unfetched fund does: {zero_stock:?}"
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

    /// The summary is a fixed height above a list that takes what is left,
    /// so on a short terminal it can leave the list nothing -- and no key on
    /// this screen hides it. The rows are what the other keys act on, so they
    /// are the half that keeps the screen.
    #[test]
    fn a_terminal_too_short_for_both_keeps_the_list_and_drops_the_summary() {
        let funds = funds_with_mixes();
        let lines = drawn(&funds, summary_lines(funds.allocation()) + LIST_FLOOR - 1);

        assert!(
            !lines.iter().any(|l| l.contains("Target")),
            "the summary yields: {lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("Ticker")),
            "the list keeps its header: {lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("USM")),
            "and a holding to sit on: {lines:#?}"
        );
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
        let row_ends = super::super::ends_in_order(bonds, &["18.00%", "59.37%", "41.37%"]);
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
        // `Other` is accounted for and carries no target -- the age rule
        // says nothing about cash, and here it is all the row holds.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Other") && l.contains("5.00%")),
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

    /// Every class represented, so the bar has four segments to draw rather
    /// than three and a gap. `funds_with_mixes` deliberately does not: it
    /// mirrors the app fixture, whose unfetched `USM` is what leaves the U.S.
    /// stock row at nothing.
    fn funds_fully_priced() -> Funds {
        let mut funds = funds_with_mixes();
        let mut mixes = HashMap::from([(
            "USM".to_string(),
            vec![Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints::ONE,
            }],
        )]);
        for (ticker, slices) in funds.mixes.clone() {
            mixes.insert(ticker, slices);
        }
        funds.set_mixes(mixes);
        funds
    }

    /// $10,000 all U.S. stock, $5,000 of a 70/25/5 bond fund and $3,000 of a
    /// 95/5 international fund, over $18,000: 55.56 U.S. stock, 15.83
    /// international, 19.45 + 6.94 bonds and 2.22 cash.
    ///
    /// In class order that is 55.56 / 15.83 / 26.39 / 2.22, and the bar cuts
    /// at the cumulative share -- so its segments are that split scaled to
    /// forty cells, 22 / 6 / 11 / 1, with nothing left for the track.
    #[test]
    fn the_total_bar_is_one_run_of_four_segments_in_the_portfolios_own_proportions() {
        let funds = funds_fully_priced();
        let summary = funds.summary();
        assert_eq!(
            allocation::weight(&summary, AssetClass::UsStock),
            BasisPoints(5_556)
        );

        let bar = summary_bar(
            Class::ALL.map(|class| class.actual(&summary)),
            BasisPoints::ONE,
        );
        let widths: Vec<usize> = bar.iter().map(|s| s.content.chars().count()).collect();
        assert_eq!(widths, vec![22, 6, 11, 1, 0]);
        assert_eq!(
            widths.iter().sum::<usize>(),
            SUMMARY_BAR_WIDTH,
            "the segments and the track have to fill the bar"
        );
        for (span, class) in bar.iter().zip(Class::ALL) {
            assert_eq!(
                span.style.bg,
                Some(super::super::style::class(class)),
                "{class:?} is drawn on another class's color"
            );
        }
    }

    /// The equities lead and sit together, then bonds, then what the rule
    /// says nothing about -- and the colors are written in the same order, so
    /// a reorder of one without the other repaints every segment rather than
    /// moving it.
    #[test]
    fn the_bar_runs_equities_then_bonds_then_the_rest() {
        assert_eq!(
            Class::ALL,
            [Class::UsStock, Class::IntlStock, Class::Bonds, Class::Other]
        );
        assert_eq!(
            crate::palette::CLASSES.len(),
            Class::ALL.len(),
            "the color table and the class list came apart"
        );
    }

    /// A share is written inside its own segment only where a column of space
    /// fits on either side of it: flush against the join, a figure reads as
    /// belonging to whichever neighbour the eye lands on first.
    #[test]
    fn a_segment_writes_its_share_only_where_a_column_of_space_fits_either_side() {
        assert_eq!(segment_text(5, Some("56%".to_string())), " 56% ");
        assert_eq!(
            segment_text(4, Some("56%".to_string())),
            "    ",
            "three characters in four columns leaves no padding at all"
        );
        assert_eq!(segment_text(1, Some("2%".to_string())), " ");
        assert_eq!(segment_text(3, None), "   ");
    }

    /// Centred in what is left, the odd column going to the right.
    #[test]
    fn a_share_sits_in_the_middle_of_the_segment_it_names() {
        assert_eq!(segment_text(8, Some("56%".to_string())), "  56%   ");
        assert_eq!(segment_text(7, Some("56%".to_string())), "  56%  ");
    }

    /// Each treatment's bar is normalised to what *that* treatment holds, so
    /// its four classes fill the width and can be read against the bar above
    /// it. The tax columns in the table state the same figures against one
    /// denominator, which is what makes the grid foot and what stops two
    /// treatments being compared without arithmetic.
    #[test]
    fn a_treatments_bar_is_normalised_to_what_that_treatment_holds() {
        // A pot holding a quarter of the portfolio, all of it in one class,
        // fills its own bar rather than a quarter of it.
        let quarter_in_stock = [
            BasisPoints(2_500),
            BasisPoints::ZERO,
            BasisPoints::ZERO,
            BasisPoints::ZERO,
        ];
        let widths: Vec<usize> = summary_bar(quarter_in_stock, BasisPoints(2_500))
            .iter()
            .map(|s| s.content.chars().count())
            .collect();
        assert_eq!(widths, vec![SUMMARY_BAR_WIDTH, 0]);

        // And against the whole portfolio it is the quarter it is.
        let widths: Vec<usize> = summary_bar(quarter_in_stock, BasisPoints::ONE)
            .iter()
            .map(|s| s.content.chars().count())
            .collect();
        assert_eq!(
            widths,
            vec![SUMMARY_BAR_WIDTH / 4, SUMMARY_BAR_WIDTH * 3 / 4]
        );
    }

    /// A treatment with nothing in it is a row of track: no segment claims a
    /// cell, and nothing is divided by nothing on the way there.
    #[test]
    fn a_treatment_holding_nothing_draws_an_empty_bar_rather_than_dividing_by_zero() {
        let bar = summary_bar([BasisPoints::ZERO; 4], BasisPoints::ZERO);
        assert_eq!(bar.len(), 1, "a segment was drawn for an empty pot");
        assert_eq!(bar[0].content.chars().count(), SUMMARY_BAR_WIDTH);
    }

    /// The bar rows of the panel, in order, so a test can name them without
    /// matching on a word a fund name might also carry.
    fn panel_lines(funds: &Funds) -> Vec<String> {
        let lines = drawn(funds, 24);
        let panel = usize::from(summary_lines(funds.allocation()));
        let count = bars(funds.allocation()).len();
        lines[panel - 1 - count..panel - 1].to_vec()
    }

    /// The same portfolio held across two treatments, one per account, so
    /// more than one bar has anything to draw and `Tab` narrows to exactly
    /// one of them.
    ///
    /// `funds_fully_priced` deliberately holds everything in one treatment,
    /// which is both what a filter leaves and what most of the tests above
    /// are about.
    fn funds_across_two_treatments() -> Funds {
        let mut funds = funds_fully_priced();
        let rows: Vec<Row> = funds
            .rows
            .iter()
            .cloned()
            .map(|row| match row.account_id == AccountId(2) {
                true => Row {
                    tax_treatment: Some(TaxTreatment::TaxFree),
                    ..row
                },
                false => row,
            })
            .collect();
        funds.set_rows(rows);
        funds
    }

    /// Named, `Total` first and then one per treatment in the order the
    /// table's columns run -- so a column and the bar under it are the same
    /// pot.
    #[test]
    fn every_bar_is_named_and_the_treatments_follow_the_tables_own_column_order() {
        let funds = funds_across_two_treatments();
        let bars = panel_lines(&funds);

        assert_eq!(
            bars.len(),
            3,
            "two treatments hold something, so they and Total are the bars"
        );
        for (line, label) in bars.iter().zip(["Total", "Taxable", "Tax-free"]) {
            assert!(
                line.contains(label),
                "a bar is not named {label:?}: {line:?}"
            );
        }
    }

    /// A treatment holding nothing states a fact its own tax column already
    /// states, and costs a line of a panel the list is paying for. Narrowing
    /// to one account is where that stops being an edge case: an account has
    /// exactly one treatment, so three of the four bars would be track.
    #[test]
    fn a_treatment_holding_nothing_gets_no_bar_at_all() {
        let funds = funds_across_two_treatments();
        let labels: Vec<&str> = TaxTreatment::ALL
            .iter()
            .map(|t| t.label())
            .filter(|label| panel_lines(&funds).iter().any(|l| l.contains(label)))
            .collect();
        assert_eq!(labels, vec!["Taxable", "Tax-free"]);
    }

    /// With one treatment left, `Total` *is* that treatment's bar -- the same
    /// four shares over the same denominator, since the whole of what is on
    /// screen is what that pot holds. Two identical bars is a reader being
    /// asked to compare something with itself, so the pot keeps its name and
    /// `Total` goes.
    #[test]
    fn one_treatment_left_is_drawn_once_under_its_own_name() {
        let mut funds = funds_across_two_treatments();
        walk_until!(
            funds.filter_account() == Some(AccountId(2)),
            funds.next_account()
        );

        let bars = panel_lines(&funds);
        assert_eq!(bars.len(), 1, "{bars:#?}");
        assert!(bars[0].contains("Tax-free"), "{bars:#?}");
        assert!(
            !bars[0].contains("Total"),
            "the filtered pot is drawn twice: {bars:#?}"
        );
    }

    /// The table is figures and the bars are pictures of them; with nothing
    /// between, `Other` and the first bar read as two rows of one list, the
    /// left-hand labels running straight on.
    #[test]
    fn a_blank_line_separates_the_last_class_row_from_the_first_bar() {
        let funds = funds_across_two_treatments();
        let lines = drawn(&funds, 24);
        let last_class = lines
            .iter()
            .position(|l| l.contains(Class::Other.label()))
            .expect("the Other row is drawn");
        let first_bar = lines
            .iter()
            .position(|l| l.contains("Total"))
            .expect("the Total bar is drawn");

        assert_eq!(
            first_bar - last_class,
            1 + usize::from(BAR_GAP),
            "the gap between the table and the bars is wrong"
        );
        let gap = &lines[last_class + 1];
        assert!(
            gap.trim_matches(|c| c == '│' || c == ' ').is_empty(),
            "the separating line is not blank: {gap:?}"
        );
    }

    /// A segment and the row above it are the same statement, so the row's
    /// own label is what names the color -- there is no legend, and the lines
    /// the bars sit on are the bars and their names.
    #[test]
    fn each_class_label_carries_its_own_segments_color_and_the_bars_have_no_legend() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let funds = funds_fully_priced();
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &funds);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let line = |y: u16| {
            (0..MIN_WIDTH)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };

        for class in Class::ALL {
            let y = (0..24)
                .find(|y| line(*y).contains(class.label()))
                .unwrap_or_else(|| panic!("{class:?} has no row"));
            let x = (0..MIN_WIDTH)
                .find(|x| buffer[(*x, y)].symbol() == &class.label()[..1])
                .expect("the label starts somewhere");
            assert_eq!(
                buffer[(x, y)].style().fg,
                Some(super::super::style::class(class)),
                "{class:?}'s label is not drawn in its segment's color"
            );
        }

        for bar in panel_lines(&funds) {
            for class in Class::ALL {
                assert!(
                    !bar.contains(class.label()),
                    "{class:?} is named again beside a bar: {bar:?}"
                );
            }
        }

        let bonds = funds.summary_row(Class::Bonds).expect("a Bonds row").actual;
        assert_eq!(
            bonds,
            BasisPoints(1_945 + 694),
            "the row and the segment are one statement"
        );
    }

    /// The summary is only as current as its stalest input, so the stamp is
    /// the *oldest* filing behind the rows on screen -- and the holding with
    /// no filing at all makes it no older, that being what the panel's own
    /// coverage line already reports.
    #[test]
    fn the_border_stamps_the_oldest_filing_behind_the_rows_on_screen() {
        let all = accounts();
        let mut funds = funds_with_mixes();
        funds.set_rows(vec![
            fixture_row(1, AccountId(1), &all, "USM", 10_000),
            Row {
                stock_percent: Some(BasisPoints::ZERO),
                as_of: Some(day(2026, 3, 31)),
                ..fixture_row(2, AccountId(1), &all, "USB", 5_000)
            },
            Row {
                stock_percent: Some(BasisPoints(9_500)),
                as_of: Some(day(2026, 6, 30)),
                ..fixture_row(3, AccountId(2), &all, "ISM", 3_000)
            },
        ]);

        assert_eq!(funds.title().plain_text(), "Funds · All · as of 2026-03-31");

        // Narrowed to the account holding only the newer filing, the stamp
        // follows the rows the way the summary above them does.
        walk_until!(
            funds.filter_account() == Some(AccountId(2)),
            funds.next_account()
        );
        assert!(
            funds.title().plain_text().ends_with("as of 2026-06-30"),
            "{}",
            funds.title().plain_text()
        );
    }

    /// A database nobody has run the fetcher against has no filing to quote,
    /// and a border reading `as of —` would be a question drawn as an answer.
    #[test]
    fn a_portfolio_with_no_filing_on_record_stamps_no_date_at_all() {
        assert_eq!(funds().title().plain_text(), "Funds · All");
    }

    /// The border totals whatever the filters leave, not every holding
    /// fetched -- the panel above it answers for the same rows, and a
    /// header that kept quoting the whole portfolio while the list showed
    /// one account would be the only thing on screen not narrowing with
    /// `Tab`.
    #[test]
    fn the_border_totals_the_holdings_the_filters_leave() {
        let mut funds = funds();
        assert_eq!(funds.total(), Cents::from_dollars(18_000));

        funds.next_account();
        assert_eq!(
            funds.total(),
            Cents::from_dollars(15_000),
            "the total did not follow the account filter"
        );

        funds.clear_filters();
        funds.begin_search();
        for c in "ISM".chars() {
            funds.push_search(c);
        }
        assert_eq!(
            funds.total(),
            Cents::from_dollars(3_000),
            "the total did not follow the search"
        );
    }

    /// And it is drawn, in whole dollars, past every filter term the title
    /// carries -- so it sits in one place whether or not a search is running
    /// and whether or not anything has a filing behind it.
    #[test]
    fn the_total_is_drawn_last_in_the_border_in_whole_dollars() {
        let funds = funds_with_mixes();
        let border = drawn(&funds, 24)
            .into_iter()
            .find(|l| l.contains("Funds ·"))
            .expect("the list's border is drawn");

        let stamp = border.find("as of").expect("the filing stamp");
        let total = border
            .find("$18,000")
            .unwrap_or_else(|| panic!("the total is not drawn in whole dollars: {border:?}"));
        assert!(
            stamp < total,
            "the stamp reads out of the figure: {border:?}"
        );
        assert!(
            !border.contains("$18,000.00"),
            "the border quotes cents its own column does not: {border:?}"
        );
    }

    /// A fund names itself in its own filing, so the column arrives with the
    /// composition and is `—` until a `g` has fetched one -- the same absence
    /// `Stock%` beside it draws, rather than a second way of saying nothing.
    #[test]
    fn a_fund_with_no_filing_fetched_draws_no_name_and_one_with_a_filing_draws_it() {
        let all = accounts();
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![
            Row {
                name: None,
                ..fixture_row(1, AccountId(1), &all, "USM", 10_000)
            },
            fixture_row(2, AccountId(1), &all, "USB", 5_000),
        ]);

        let lines = drawn(&funds, 8);
        let header = lines
            .iter()
            .find(|l| l.contains("Ticker"))
            .expect("the list header is drawn");
        assert!(header.contains("Fund"), "{header:?}");

        let named = lines
            .iter()
            .find(|l| l.contains("USB"))
            .expect("the named row is drawn");
        assert!(
            named.contains(crate::test_support::fund_name("USB")),
            "a fetched fund does not say what it is: {named:?}"
        );

        let unnamed = lines
            .iter()
            .find(|l| l.contains("USM"))
            .expect("the unnamed row is drawn");
        assert!(
            unnamed.contains('—'),
            "an unfetched fund drew something other than the absence mark: {unnamed:?}"
        );
    }

    /// The `Tax` column holds one of a closed set, so it is sized off that
    /// set: the widest treatment, drawn whole.
    ///
    /// It passes at any width the labels happen to fit in, which is the
    /// point -- the column was a hardcoded `13` with `Tax-deferred`, twelve
    /// characters, the longest thing it could hold, and the only test over
    /// it named `Taxable`, seven. One character of slack, and nothing
    /// measuring it: a variant renamed longer would have truncated here with
    /// nothing going red. The Accounts screen paid for exactly that on its
    /// `Kind` column.
    #[test]
    fn the_widest_tax_treatment_is_drawn_whole_at_the_minimum_width() {
        let all = accounts();
        let widest = TaxTreatment::ALL
            .iter()
            .max_by_key(|t| t.label().chars().count())
            .expect("a treatment");
        let account = all
            .iter()
            .find(|a| a.tax_treatment.is_some())
            .expect("an investment account")
            .id;

        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![Row {
            tax_treatment: Some(*widest),
            ..fixture_row(1, account, &all, "USM", 10_000)
        }]);

        let row = drawn(&funds, 6)
            .into_iter()
            .find(|l| l.contains("USM"))
            .expect("the row is drawn");
        assert!(
            row.contains(widest.label()),
            "the treatment is cut: {row:?}"
        );
    }

    /// The width claim, made against a drawn row rather than arithmetic: a
    /// name as long as the longest a real filing carries has to reach the
    /// screen whole at `MIN_WIDTH`, with `Account` beside it still holding
    /// its own `Constraint::Min`.
    ///
    /// The name is invented and its length is the point -- forty-four
    /// characters, which is what a target-date fund's issuer, its words and
    /// its year come to. A column cut short of that draws every target-date
    /// row alike, the year being the last four characters of the name.
    #[test]
    fn the_longest_fund_name_a_filing_carries_is_drawn_whole_at_the_minimum_width() {
        let long = "Acme Institutional Target Retirement 2045 Fund";
        assert!(
            long.chars().count() <= usize::from(FUND_NAME_WIDTH),
            "the fixture outgrew the column it is measuring: {}",
            long.chars().count()
        );

        let all = accounts();
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![Row {
            name: Some(long.to_string()),
            ..fixture_row(1, AccountId(1), &all, "USM", 10_000)
        }]);

        let lines = drawn(&funds, 6);
        let row = lines
            .iter()
            .find(|l| l.contains("USM"))
            .expect("the row is drawn");
        assert!(row.contains(long), "the name is cut: {row:?}");
        assert!(
            row.contains(&Account::named(&all, AccountId(1)).text()),
            "the account beside it lost its own column: {row:?}"
        );
    }

    /// A column a reader can see and cannot search reads as broken, and the
    /// name is what makes "every bond fund" a needle at all -- neither the
    /// ticker nor the account says so.
    #[test]
    fn the_search_matches_the_fund_a_ticker_names() {
        let mut funds = funds();
        funds.begin_search();
        for c in "Bond".chars() {
            funds.push_search(c);
        }
        let tickers: Vec<&str> = funds.rows().iter().map(|r| r.ticker.as_str()).collect();
        assert_eq!(tickers, vec!["USB"], "`Bond` is in no ticker or account");
    }

    /// A needle is matched as a substring of the row's three words, so a row
    /// that has no name yet answers to the two it has -- a name's empty
    /// place left in the haystack would put two spaces where a reader typed
    /// one.
    #[test]
    fn a_row_with_no_name_yet_still_answers_to_its_ticker_and_its_account() {
        let all = accounts();
        let mut funds = Funds::new();
        funds.set_accounts(all.clone());
        funds.set_rows(vec![Row {
            name: None,
            ..fixture_row(1, AccountId(1), &all, "USM", 10_000)
        }]);

        funds.begin_search();
        let account = funds.account_name(AccountId(1)).to_string();
        for c in format!("USM {account}").chars() {
            funds.push_search(c);
        }
        assert_eq!(funds.rows().len(), 1, "{:?}", funds.rows());
    }

    /// The list answers "what do I hold and how is it taxed", and the two
    /// columns it used to spend on the mix bar and the filing date answered
    /// neither: the bar restated the `Stock%` beside it, and the date is on
    /// the report for the one reader who wants it.
    #[test]
    fn the_list_names_each_holdings_tax_treatment_and_spends_no_column_on_a_bar() {
        let funds = funds_with_mixes();
        let lines = drawn(&funds, 24);
        let header = lines
            .iter()
            .find(|l| l.contains("Ticker"))
            .expect("the list header is drawn");

        assert!(header.contains("Tax"), "{header:?}");
        assert!(!header.contains("Mix"), "{header:?}");
        assert!(!header.contains("As of"), "{header:?}");

        let row = lines
            .iter()
            .find(|l| l.contains("USB"))
            .expect("a holding is drawn");
        assert!(
            row.contains(TaxTreatment::Taxable.label()),
            "a holding does not say how it is taxed: {row:?}"
        );
    }

    /// The Δ is the one cell on the panel a reader has to act on, so being
    /// short of the target reads as a color rather than only as a sign.
    ///
    /// Which way the sign runs is half the claim: `actual - target`, so the
    /// fixture's over-weight bond row is a *positive* number in no color and
    /// the U.S. stock row it starves is the negative one. Subtracted the
    /// other way both halves of this test invert, which is exactly the
    /// reading the column is not allowed to have.
    #[test]
    fn a_shortfall_in_the_delta_column_is_drawn_in_the_negative_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let funds = funds_with_mixes();
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &funds);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The Bonds row runs over target, so its Δ is positive; U.S. Stock
        // holds nothing against a target of nearly half the portfolio, so
        // its Δ is the shortfall.
        let row_of = |label: &str| {
            (0..24)
                .find(|y| {
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(label)
                })
                .unwrap_or_else(|| panic!("{label} is drawn"))
        };
        let colors = |y: u16| {
            (0..MIN_WIDTH)
                .filter(|x| buffer[(*x, y)].symbol() != " ")
                .map(|x| buffer[(x, y)].style().fg)
                .collect::<Vec<_>>()
        };

        assert!(
            colors(row_of("U.S. Stock")).contains(&Some(super::super::style::negative())),
            "a class short of its target draws no warning"
        );
        assert!(
            !colors(row_of("Bonds")).contains(&Some(super::super::style::negative())),
            "a class held past its target was drawn as a shortfall"
        );
    }

    /// A `None` target has to reach the cells, not only the model: a bond
    /// share nobody has a birth date for is a question, and `0.00%` in either
    /// column would read as advice.
    #[test]
    fn a_bond_target_with_no_birth_date_draws_an_em_dash_in_both_of_its_cells() {
        let mut funds = funds_with_mixes();
        funds.set_targets(crate::calc::fund::targets(None, BasisPoints(4_000)));

        let lines = drawn(&funds, 24);
        let bonds = lines
            .iter()
            .find(|l| l.contains("Bonds"))
            .expect("the Bonds row is drawn");
        super::super::ends_in_order(bonds, &["—", "59.37%", "—"]);
        // The tax columns beyond the Δ genuinely read zero here -- the
        // fixture holds everything in one treatment -- so the claim is about
        // the two cells the target owns, not the whole row.
        let through_delta = &bonds[..bonds.find('—').expect("the target cell") + "—".len()];
        assert!(
            !through_delta.contains("0.00%"),
            "a missing target drew a zero: {bonds:?}"
        );
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
        let row_ends = super::super::ends_in_order(row, &["100", "62%"]);
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
