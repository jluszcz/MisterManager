//! The Funds tab: what the portfolio holds by asset class, against what the
//! age rule says it should.
//!
//! A spelling of the Funds screen rather than a reading of its own. The rows,
//! the four-class split and the apportioning behind them are
//! [`crate::allocation`]'s, the same list the screen draws; what is decided
//! here is only what this medium has to decide -- a bar of colors where the
//! screen spends glyphs, the legend that pairs those colors with the words,
//! and the accounts stacked rather than cycled, a page having no `Tab` to
//! offer.
//!
//! **The bar is CSS widths on a `<div>` and nothing else.** The page carries
//! no script and is read offline on a phone, so a charting library is the one
//! thing on it that could fail to arrive; every share is baked into a
//! `style="width:..."` at render time, and the percentage is written out
//! beside it so the bar is a picture of something the reader can also read.

use super::{account, escape, whole_money};
use crate::allocation::{self, UNTARGETED};
use crate::db::fund_mix::{AssetClass, Slice};
use crate::palette;
use crate::rate::BasisPoints;
use crate::report::{AccountAllocation, Allocation, Holding};

/// One class's color, as `#rrggbb`.
///
/// By its place in `AssetClass::ALL`, which is the order
/// [`palette::ASSET_CLASSES`] is written in -- nothing stores that position,
/// so the two move together.
fn color(class: AssetClass) -> String {
    let index = AssetClass::ALL
        .iter()
        .position(|c| *c == class)
        .expect("AssetClass::ALL names every variant");
    palette::hex(palette::ASSET_CLASSES[index])
}

/// A share, or the `--` this page draws an absence with.
///
/// **Never a zero.** A bond target with no birth date behind it is a
/// question rather than a share of nothing, and a cell reading `0.00%` would
/// answer it -- wrongly, and with a figure the Δ beside it would then measure
/// the whole portfolio against. The birth date the rule counts from is
/// imported from the workbook's `Constants` sheet, so a database nobody has
/// imported into has no age to derive a bond target from -- and until it
/// does, there is nothing here to state.
///
/// The mark is `optional_money`'s and `savings::percent`'s, because the page
/// has one. A reader meets this column beside those two with no way to hover
/// for an explanation, and a second spelling of "nothing here" would be a
/// difference they had to work out the meaning of.
fn percent(share: Option<BasisPoints>) -> String {
    match share {
        Some(share) => format!("<td class=\"n\">{share}%</td>"),
        None => "<td class=\"n\">--</td>".to_string(),
    }
}

/// The summary's own header, in the Funds screen's column order and wording,
/// each column carrying the class its cells below carry.
const SUMMARY_HEADER: [(&str, &str); 4] = [
    ("Class", ""),
    ("Target", "n"),
    ("Actual", "n"),
    ("\u{394}", "n"),
];

/// The holdings' header: the screen's own wording, three of its six columns
/// short.
///
/// `Account` is the heading above the table, so no column is spent on it.
/// `Mix` and `Stock%` are one quantity drawn twice -- a bar and the figure
/// beside it, both of them *one fund's* stock share -- and neither is here:
/// that share is `tui::app::funds::stock_share`'s two-class sum, private to
/// the module that feeds the screen, and spelling it again on this page would
/// be a second reading of what counts as stock.
///
/// **So a fund's own composition is not on this page anywhere.** The bar one
/// block up is a different quantity at a different granularity -- the whole
/// account apportioned across four classes -- and it cannot say which of the
/// rows below it is the bond fund. That is what the tab costs a reader on a
/// phone, and it is the price of not having two answers on record to one
/// question about one fund.
const HOLDINGS_HEADER: [(&str, &str); 3] = [("Ticker", ""), ("Balance", "n"), ("As of", "d")];

/// A header row, each cell aligned the way the column under it is -- a
/// header that took its own alignment would sit over the wrong edge of every
/// figure beneath it.
fn header(columns: &[(&str, &str)]) -> String {
    let cells: String = columns
        .iter()
        .map(|(label, class)| match class.is_empty() {
            true => format!("<th>{label}</th>"),
            false => format!("<th class=\"{class}\">{label}</th>"),
        })
        .collect();
    format!("<thead><tr>{cells}</tr></thead>")
}

/// Class, Target, Actual, Δ -- the three classes the age rule targets, then
/// the ones it says nothing about.
///
/// The untargeted rows carry no target and so no gap: the age rule has
/// nothing to say about cash, and a target for money a filing could not place
/// would be a claim about nothing. `Cash` keeps its row at zero and
/// `Unclassified` has one only when something went unplaced, which is
/// [`allocation::apportion`]'s doing rather than this table's -- a defect
/// report reading "none" every time is one nobody finishes reading.
///
/// The class labels take no color, where the bar below them does: `Bonds` is
/// two classes at once, and a row tinted as one of them would be a claim the
/// age rule cannot make.
fn summary(targeted: &[allocation::SummaryRow], slices: &[Slice]) -> String {
    let mut rows = header(&SUMMARY_HEADER);
    for row in targeted {
        rows.push_str(&format!(
            "<tr><td>{}</td>{}{}{}</tr>",
            row.class.label(),
            percent(row.target),
            percent(Some(row.actual)),
            percent(row.delta),
        ));
    }
    for slice in slices.iter().filter(|s| UNTARGETED.contains(&s.class)) {
        rows.push_str(&format!(
            "<tr><td>{}</td>{}{}{}</tr>",
            slice.class.label(),
            percent(None),
            percent(Some(slice.weight)),
            percent(None),
        ));
    }
    format!("<table>{rows}</table>")
}

/// The whole of a look-through as one bar, and the legend that names it.
///
/// **This is the one thing in the summary that splits the bonds**, for the
/// reason the screen's is: the age rule produces a single bond number, so the
/// row beside it cannot say whether the share is domestic or foreign. What
/// the four leave over is cash, the classifier's residual and whatever no
/// filing placed -- drawn as the track the segments sit on rather than as a
/// fifth segment, so the bar cannot read as though these four were the whole
/// portfolio.
///
/// Segments are cut at the *cumulative* share and differenced, exactly as
/// `tui::fund::summary_bar` cuts its glyphs: a filing that over-foots leaves
/// a negative `Unclassified` and puts the four classes past 100% between
/// them, and a segment that ran backwards would draw over the one before it.
/// The legend quotes each class's own share rather than the clamped segment,
/// since a figure is what a reader checks the picture against.
fn bar(slices: &[Slice]) -> String {
    let mut segments = String::new();
    let mut legend = String::new();
    let (mut cumulative, mut drawn) = (0i64, 0i64);
    for class in allocation::BAR_CLASSES {
        let (share, color) = (allocation::weight(slices, class), color(class));
        cumulative += share.0;
        let end = cumulative.clamp(0, BasisPoints::ONE.0).max(drawn);
        segments.push_str(&format!(
            "<span style=\"width:{}%;background:{color}\"></span>",
            BasisPoints(end - drawn),
        ));
        legend.push_str(&format!(
            "<span class=\"item\"><span class=\"key\" style=\"background:{color}\"></span>\
             {} {share}%</span>",
            class.label(),
        ));
        drawn = end;
    }
    format!("<div class=\"bar\">{segments}</div><p class=\"legend\">{legend}</p>")
}

/// What the summary above covers, when it does not cover everything.
///
/// [`crate::allocation::Allocation::coverage`]'s own wording, which is what
/// the screen puts in the panel's title -- drawn only when it has something
/// to say, for the reason stated there.
fn coverage(lookthrough: &crate::allocation::Allocation) -> String {
    match lookthrough.coverage() {
        Some(coverage) => format!("<p class=\"stamp\">{coverage}</p>"),
        None => String::new(),
    }
}

/// One account's holdings: the ticker, the balance the owner typed, and the
/// filing the fund's composition was read out of.
///
/// The `--` under `As of` is the whole of what a holding outside the summary
/// gets to say, and it is enough: no filing on record is exactly what put it
/// there, and the coverage line above has already counted it. The page's own
/// absence mark, for [`percent`]'s reason -- one spelling per page.
fn holdings(rows: &[Holding]) -> String {
    let mut table = header(&HOLDINGS_HEADER);
    for row in rows {
        let as_of = match row.as_of {
            Some(date) => escape(&date.to_string()),
            None => "--".to_string(),
        };
        table.push_str(&format!(
            "<tr><td class=\"w\">{}</td>{}<td class=\"d\">{as_of}</td></tr>",
            escape(&row.ticker),
            whole_money(row.balance),
        ));
    }
    format!("<table>{table}</table>")
}

/// One account: its own look-through, and the holdings behind it.
///
/// Both halves, because `Tab` on the screen narrows both together. The
/// summary is absent where the account has no composition at all, which is
/// what the screen does with the panel when nothing on it carries a mix --
/// the rows are still worth listing, and a table of dashes over every class
/// says only that a key has not been pressed yet.
fn section(account_allocation: &AccountAllocation) -> String {
    let mut html = format!("<h3>{}</h3>", account(&account_allocation.account));
    if !account_allocation.summary.is_empty() {
        html.push_str(&coverage(&account_allocation.lookthrough));
        html.push_str(&summary(
            &account_allocation.summary,
            &account_allocation.lookthrough.slices,
        ));
        html.push_str(&bar(&account_allocation.lookthrough.slices));
    }
    html.push_str(&holdings(&account_allocation.holdings));
    html
}

/// Nothing entered: no investment account holds a holding.
///
/// Named rather than written into [`sections`], and named apart from
/// [`NO_COMPOSITION`], because telling the two nothings apart is the whole
/// of what the sentences do -- a test standing in one branch says which by
/// the constant it reads rather than by restating prose it would then drift
/// from.
const NO_HOLDINGS: &str =
    "<p>No holdings are on record, so there is nothing to look through yet.</p>";

/// Holdings, but no filing behind a single one of them: the state every
/// database is in until something populates `fund_mix`.
const NO_COMPOSITION: &str =
    "<p>No fund composition has been fetched, so there is nothing to look through yet.</p>";

/// The portfolio, then one stacked section per investment account.
///
/// Each empty state says which one it is in a sentence. An empty
/// `<section>` would be a tab a reader taps and gets a blank page from, with
/// no way to tell a page with nothing to say from one that failed to build --
/// the same reason the screen draws `—` in the mix column rather than leaving
/// the row's cells empty.
pub(super) fn sections(allocation: &Allocation) -> String {
    if allocation.accounts.is_empty() {
        return NO_HOLDINGS.to_string();
    }
    let mut html = String::new();
    if allocation.summary.is_empty() {
        html.push_str(NO_COMPOSITION);
    } else {
        html.push_str("<h3>Allocation</h3>");
        html.push_str(&coverage(&allocation.lookthrough));
        html.push_str(&summary(
            &allocation.summary,
            &allocation.lookthrough.slices,
        ));
        html.push_str(&bar(&allocation.lookthrough.slices));
    }
    html.extend(allocation.accounts.iter().map(section));
    html
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{self, panel, snapshot};
    use super::super::page;
    use crate::allocation::{self, SummaryRow, TargetClass};
    use crate::db::fund_mix::{AssetClass, Slice};
    use crate::money::Cents;
    use crate::palette;
    use crate::rate::BasisPoints;

    /// The fixture's page, and the Funds panel of it.
    fn funds_panel(snapshot: &crate::report::Snapshot) -> String {
        panel(&page(snapshot), "funds").to_string()
    }

    /// Every control on this page is CSS, and a tab that reached for a
    /// script would be the one thing on a phone's offline copy that could
    /// fail to arrive. The bar is the reason to check it here as well as
    /// page-wide: a stacked bar is the shape somebody reaches for a chart
    /// library to draw.
    #[test]
    fn the_allocation_tab_carries_no_script() {
        let html = page(&snapshot(vec![], 1_000));
        assert!(!html.contains("<script"), "the report grew a script");
        assert!(!html.contains("http"), "the tab reaches out of itself");
    }

    /// Color alone is a key a reader has to hold, and the report is read
    /// beside a screen that spends glyphs rather than colors -- so every
    /// class the bar draws is named in words and priced in figures beside
    /// it.
    #[test]
    fn every_asset_class_bar_has_a_width_and_a_percentage_beside_it() {
        let snapshot = snapshot(vec![], 1_000);
        let panel = funds_panel(&snapshot);
        let slices = &snapshot.allocation.lookthrough.slices;
        for class in AssetClass::ALL {
            // Nothing in the fixture's filings went unplaced, and a residual
            // reading zero is a defect report nobody finishes reading.
            if class == AssetClass::Unclassified {
                continue;
            }
            assert!(
                panel.contains(class.label()),
                "{} is not labelled",
                class.label()
            );
        }
        for class in allocation::BAR_CLASSES {
            let share = allocation::weight(slices, class);
            assert!(
                panel.contains(&format!(
                    "width:{share}%;background:{}",
                    super::color(class)
                )),
                "{} has no segment: {panel}",
                class.label()
            );
            assert!(
                panel.contains(&format!("{} {share}%", class.label())),
                "{} has no percentage beside its segment: {panel}",
                class.label()
            );
        }
    }

    /// The tab is a spelling of the screen: the same three rows, off the
    /// same `crate::fund::targets_from_db`, and a page quoting a target the
    /// terminal did not would be two answers to one question about one age.
    #[test]
    fn the_allocation_tab_draws_the_same_targets_the_screen_does() {
        let snapshot = snapshot(vec![], 1_000);
        let panel = funds_panel(&snapshot);
        assert!(!snapshot.allocation.summary.is_empty(), "nothing to check");
        for row in &snapshot.allocation.summary {
            let Some(target) = row.target else { continue };
            assert!(
                panel.contains(&format!("{target}%")),
                "{:?}'s target is missing from the page",
                row.class
            );
            assert!(
                panel.contains(&format!("{}%", row.actual)),
                "{:?}'s actual is missing from the page",
                row.class
            );
        }
    }

    /// No birth date on record is a question rather than a zero, so neither
    /// the target nor the gap from it states a figure -- the screen's own
    /// answer, spelled in this page's own mark.
    #[test]
    fn a_bond_target_with_no_birth_date_states_no_figure_in_either_of_its_cells() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.allocation = fixture::funds(crate::calc::fund::targets(None, BasisPoints(4_000)));
        let actual = TargetClass::Bonds.actual(&snapshot.allocation.lookthrough.slices);
        assert!(
            funds_panel(&snapshot).contains(&format!(
                "<td>Bonds</td><td class=\"n\">--</td><td class=\"n\">{actual}%</td>\
                 <td class=\"n\">--</td>"
            )),
            "the bond row states a figure it has none of: {}",
            funds_panel(&snapshot)
        );
    }

    /// A holding no filing covers is outside every figure above it, so the
    /// page says how much it is summarising and the row itself says which
    /// holding is missing.
    #[test]
    fn a_holding_with_no_filing_is_outside_the_summary_and_says_so_on_its_own_row() {
        let snapshot = snapshot(vec![], 1_000);
        let panel = funds_panel(&snapshot);
        assert!(
            panel.contains("3 of 4 holdings"),
            "the coverage went unreported: {panel}"
        );
        assert!(
            panel.contains(
                "<td class=\"w\">UNC</td><td class=\"n\">1,000</td><td class=\"d\">--</td>"
            ),
            "the unpriced holding does not say so: {panel}"
        );
    }

    /// A filing that does not foot is a miss, and a miss shows up as the
    /// labelled row it is. A portfolio nothing went unplaced in draws no
    /// such row at all -- the rule the two Planning transfer footers follow.
    #[test]
    fn an_unclassified_row_is_drawn_only_when_a_filing_left_something_unplaced() {
        let whole = snapshot(vec![], 1_000);
        assert!(
            !funds_panel(&whole).contains(AssetClass::Unclassified.label()),
            "a residual of nothing was reported"
        );

        let mut short = snapshot(vec![], 1_000);
        let placed = vec![Slice {
            class: AssetClass::UsStock,
            weight: BasisPoints(9_900),
        }];
        short.allocation.lookthrough =
            allocation::apportion(&[(Cents::from_dollars(1_000), Some(placed.as_slice()))]);
        short.allocation.summary = TargetClass::ALL
            .iter()
            .map(|class| {
                SummaryRow::new(
                    *class,
                    &short.allocation.lookthrough.slices,
                    fixture::targets(),
                )
            })
            .collect();
        let panel = funds_panel(&short);
        assert!(
            panel.contains(AssetClass::Unclassified.label()),
            "the gap went unreported: {panel}"
        );
        assert!(panel.contains("1.00%"), "the gap has no figure: {panel}");
    }

    /// The same rule with the sign flipped: a mix claiming more than the
    /// whole of itself leaves a negative residual and puts the four bar
    /// classes past 100% between them. The row states the negative share --
    /// that is what says the composition over-foots -- while the bar's own
    /// segments are cut at the cumulative share and clamped, so none of them
    /// runs backwards over the one before it.
    #[test]
    fn a_mix_that_over_foots_states_a_negative_residual_and_draws_no_backwards_segment() {
        let mut over = snapshot(vec![], 1_000);
        let claimed = vec![Slice {
            class: AssetClass::UsStock,
            weight: BasisPoints(10_100),
        }];
        over.allocation.lookthrough =
            allocation::apportion(&[(Cents::from_dollars(1_000), Some(claimed.as_slice()))]);
        over.allocation.summary = TargetClass::ALL
            .iter()
            .map(|class| {
                SummaryRow::new(
                    *class,
                    &over.allocation.lookthrough.slices,
                    fixture::targets(),
                )
            })
            .collect();

        let panel = funds_panel(&over);
        assert!(
            panel.contains(AssetClass::Unclassified.label()),
            "the excess went unreported: {panel}"
        );
        assert!(
            panel.contains("-1.00%"),
            "the excess is not stated as a negative share: {panel}"
        );
        assert!(
            !panel.contains("width:-"),
            "a segment ran backwards over the one before it: {panel}"
        );
    }

    /// `Tab` on the screen narrows the summary and the list together, and a
    /// page has no `Tab` -- so each account is a section carrying both,
    /// under its own name in its own color.
    #[test]
    fn each_investment_account_carries_its_own_summary_and_its_own_holdings() {
        let snapshot = snapshot(vec![], 1_000);
        let panel = funds_panel(&snapshot);
        for (color, name, ticker) in [
            (crate::db::account::AccountColor::Copper, "Holdings", "USM"),
            (crate::db::account::AccountColor::Violet, "Long Haul", "USB"),
        ] {
            let heading = format!(
                "<h3><span style=\"color:{}\">{name}</span></h3>",
                palette::hex(palette::account(color))
            );
            let at = panel
                .find(&heading)
                .unwrap_or_else(|| panic!("no {name} section: {panel}"));
            let section = &panel[at..];
            assert!(
                section.contains(&format!("<td class=\"w\">{ticker}</td>")),
                "{name} lost its holdings: {section}"
            );
            assert!(
                section.contains("<th class=\"n\">Target</th>"),
                "{name} lost its own summary: {section}"
            );
        }
        // The second account holds no stock at all, so its own summary must
        // differ from the portfolio's rather than repeating it.
        assert!(
            panel.contains("<td>Bonds</td><td class=\"n\">18.00%</td><td class=\"n\">100.00%</td>"),
            "an account's summary is the whole portfolio's: {panel}"
        );
    }

    /// A ticker is owner-typed text and reaches the page the way a goal name
    /// does: escaped, or the table it sits in ends at its own row.
    #[test]
    fn a_ticker_reaches_the_page_escaped() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.allocation.accounts[0].holdings[0].ticker = "US<b>M".to_string();
        let panel = funds_panel(&snapshot);
        assert!(
            panel.contains("US&lt;b&gt;M"),
            "the ticker was not escaped: {panel}"
        );
        assert!(!panel.contains("<b>"), "the ticker reached the page raw");
    }

    /// A database nobody has entered a holding into is not a tab that failed
    /// to build -- and it is not a database whose funds nobody has fetched a
    /// filing for either. Both draw one sentence and no table, so the
    /// sentence itself is the only thing that tells them apart.
    #[test]
    fn a_database_with_no_holdings_says_so_in_a_sentence() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.allocation = crate::report::Allocation {
            lookthrough: allocation::Allocation::default(),
            summary: Vec::new(),
            accounts: Vec::new(),
        };
        let panel = funds_panel(&snapshot);
        assert_eq!(panel, super::NO_HOLDINGS, "the wrong nothing, or a table");
    }

    /// Holdings with no filing behind any of them still list: the screen
    /// draws the rows and no summary panel, which is what a database nobody
    /// has run the fetcher against looks like.
    #[test]
    fn a_portfolio_with_no_filing_on_record_lists_its_holdings_and_draws_no_summary() {
        let mut snapshot = snapshot(vec![], 1_000);
        for account in &mut snapshot.allocation.accounts {
            account.lookthrough = allocation::apportion(
                &account
                    .holdings
                    .iter()
                    .map(|h| (h.balance, None))
                    .collect::<Vec<_>>(),
            );
            account.summary = Vec::new();
            for holding in &mut account.holdings {
                holding.as_of = None;
            }
        }
        snapshot.allocation.lookthrough = allocation::Allocation::default();
        snapshot.allocation.summary = Vec::new();

        let panel = funds_panel(&snapshot);
        assert!(
            panel.contains("<td class=\"w\">USM</td>"),
            "the holdings vanished with the filings: {panel}"
        );
        assert!(
            !panel.contains("<th class=\"n\">Target</th>"),
            "a summary was drawn against no composition: {panel}"
        );
        // The other nothing: these holdings exist, and a sentence saying
        // none do would send the owner looking for rows on the screen in
        // front of them.
        assert!(
            panel.contains(super::NO_COMPOSITION),
            "the wrong nothing: {panel}"
        );
    }
}
