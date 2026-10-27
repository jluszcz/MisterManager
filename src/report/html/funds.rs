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
//! **The bar is widths, settled here.** A charting library is the one thing
//! on a page read offline that could fail to arrive, so every segment's share
//! is computed now and the script only sets it, and the percentage is written
//! out in the legend so the bar is a picture of something the reader can also
//! read.

use crate::allocation::{self, Class};
use crate::db::fund_mix::Slice;
use crate::rate::BasisPoints;
use crate::report::view::{Accounts, Block, Cell, Row, TabView, whole};
use crate::report::{Allocation, Holding};

/// The summary's own header, in the Funds screen's column order and wording
/// -- and three of its seven columns short. The screen's tax columns
/// (`Taxable`, `Tax-deferred`, `Tax-free`) are a second question about the
/// same portfolio, where it is held rather than what it holds, and seven
/// columns do not fit a phone; the question the page is opened to answer is
/// the allocation against the age rule, which is these four.
const SUMMARY_HEADER: [&str; 4] = ["Class", "Target", "Actual", "\u{394}"];

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
const HOLDINGS_HEADER: [&str; 3] = ["Ticker", "Balance", "As of"];

/// Nothing entered: no investment account holds a holding.
///
/// Named apart from [`NO_COMPOSITION`], because telling the two nothings
/// apart is the whole of what the sentences do -- a test standing in one
/// branch says which by the constant it reads rather than by restating prose
/// it would then drift from.
const NO_HOLDINGS: &str = "No holdings are on record, so there is nothing to look through yet.";

/// Holdings, but no filing behind a single one of them: the state every
/// database is in until something populates `fund_mix`.
const NO_COMPOSITION: &str =
    "No fund composition has been fetched, so there is nothing to look through yet.";

fn plain_row(c: Vec<Cell>) -> Row {
    Row {
        c,
        k: None,
        h: None,
        d: None,
        m: None,
    }
}

/// A share, or the `--` this page draws an absence with.
///
/// **Never a zero.** A bond target with no birth date behind it is a
/// question rather than a share of nothing, and a cell reading `0.00%` would
/// answer it -- wrongly, and with a figure the Δ beside it would then measure
/// the whole portfolio against. The mark is the Savings tab's, because the
/// page has one: a second spelling of "nothing here" would be a difference a
/// reader had to work out the meaning of.
fn percent_cell(share: Option<BasisPoints>) -> Cell {
    Cell::Text(match share {
        Some(share) => format!("{share}%"),
        None => "--".to_string(),
    })
}

/// The same cell, `neg` where the portfolio is short of what the rule asks.
///
/// A gap is the only thing in this table a reader has to act on, and
/// `palette::NEGATIVE` is what the page already spells every other shortfall
/// with. `SummaryRow::delta` is `actual - target`, so short is the negative
/// one; the screen's own `delta` closure makes the identical test, which is
/// what keeps the two mediums from coloring opposite halves of one column.
fn delta_cell(share: Option<BasisPoints>) -> Cell {
    match share {
        Some(share) if share.0 < 0 => Cell::Toned(format!("{share}%"), "neg".to_string()),
        other => percent_cell(other),
    }
}

/// Class, Target, Actual, Δ. The untargeted rows carry no target and so no
/// gap: the age rule has nothing to say about cash or about money a filing
/// could not place.
///
/// The class labels take no color, where the bar below them does -- and
/// where the Funds screen's do. This page has the room for a legend under
/// its bar and spends it; a terminal does not, so the screen puts the color
/// on the label instead. Either way a class is named in its own color
/// exactly once.
fn summary_block(rows: &[allocation::SummaryRow]) -> Block {
    Block::Table {
        class: "fs",
        head: SUMMARY_HEADER.map(str::to_string).to_vec(),
        rows: rows
            .iter()
            .map(|row| {
                plain_row(vec![
                    Cell::Text(row.class.label().to_string()),
                    percent_cell(row.target),
                    percent_cell(Some(row.actual)),
                    delta_cell(row.delta),
                ])
            })
            .collect(),
    }
}

/// The whole of a look-through as one bar, and the legend that names it.
///
/// **This is the one thing in the summary that splits the bonds**, for the
/// reason the screen's is: the age rule produces a single bond number, so the
/// row beside it cannot say whether the share is domestic or foreign. What
/// the four leave over is drawn as the track the segments sit on rather than
/// as a fifth segment, so the bar cannot read as though these four were the
/// whole portfolio.
///
/// A segment is a width and its class's place in `Class::ALL`, which is the
/// order [`crate::palette::CLASSES`] is written in -- nothing stores that
/// position, so the two move together. Segments are cut at the *cumulative*
/// share and differenced, exactly as `tui::fund::summary_bar` cuts its
/// glyphs: a filing that over-foots leaves a negative residual and puts the
/// four classes past 100% between them, and a segment that ran backwards
/// would draw over the one before it. The legend quotes each class's own
/// share rather than the clamped segment, since a figure is what a reader
/// checks the picture against.
fn bar_block(slices: &[Slice]) -> Block {
    let (mut segments, mut legend) = (Vec::new(), Vec::new());
    let (mut cumulative, mut drawn) = (0i64, 0i64);
    for class in Class::ALL {
        let share = class.actual(slices);
        cumulative += share.0;
        let end = cumulative.clamp(0, BasisPoints::ONE.0).max(drawn);
        segments.push((BasisPoints(end - drawn).to_string(), class.index()));
        legend.push((format!("{} {share}%", class.label()), class.index()));
        drawn = end;
    }
    Block::Bar { segments, legend }
}

/// What the summary covers, when it does not cover everything:
/// [`crate::allocation::Allocation::coverage`]'s own wording, which is what
/// the screen puts in the panel's title.
fn coverage_note(lookthrough: &crate::allocation::Allocation, blocks: &mut Vec<Block>) {
    if let Some(coverage) = lookthrough.coverage() {
        blocks.push(Block::Note {
            text: coverage.to_string(),
        });
    }
}

/// One account's holdings: the ticker, the balance the owner typed, and the
/// filing the fund's composition was read out of. The `--` under `As of` is
/// the whole of what a holding outside the summary gets to say, and it is
/// enough: no filing on record is exactly what put it there.
fn holdings_block(rows: &[Holding]) -> Block {
    Block::Table {
        class: "fh",
        head: HOLDINGS_HEADER.map(str::to_string).to_vec(),
        rows: rows
            .iter()
            .map(|row| {
                let as_of = match row.as_of {
                    Some(date) => date.to_string(),
                    None => "--".to_string(),
                };
                plain_row(vec![
                    Cell::Text(row.ticker.clone()),
                    whole(row.balance),
                    Cell::Text(as_of),
                ])
            })
            .collect(),
    }
}

/// The portfolio, then one stacked section per investment account.
///
/// Each account carries both halves, because `Tab` on the screen narrows both
/// together; its summary is absent where it has no composition at all, a
/// table of dashes over every class saying only that a key has not been
/// pressed yet. Each empty state says which one it is in a sentence: an empty
/// panel would be a tab a reader taps and gets a blank page from, with no way
/// to tell a page with nothing to say from one that failed to build.
pub(super) fn view(allocation: &Allocation, acc: &mut Accounts) -> TabView {
    let mut blocks = Vec::new();
    if allocation.accounts.is_empty() {
        blocks.push(Block::Note {
            text: NO_HOLDINGS.to_string(),
        });
    } else {
        if allocation.summary.is_empty() {
            blocks.push(Block::Note {
                text: NO_COMPOSITION.to_string(),
            });
        } else {
            blocks.push(Block::Heading {
                text: Cell::Text("Allocation".to_string()),
            });
            coverage_note(&allocation.lookthrough, &mut blocks);
            blocks.push(summary_block(&allocation.summary));
            blocks.push(bar_block(&allocation.lookthrough.slices));
        }
        for a in &allocation.accounts {
            blocks.push(Block::Heading {
                text: acc.cell(&a.account),
            });
            if !a.summary.is_empty() {
                coverage_note(&a.lookthrough, &mut blocks);
                blocks.push(summary_block(&a.summary));
                blocks.push(bar_block(&a.lookthrough.slices));
            }
            blocks.push(holdings_block(&a.holdings));
        }
    }
    TabView {
        id: "funds",
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, snapshot};
    use super::{NO_COMPOSITION, NO_HOLDINGS, view};
    use crate::allocation::{self, Class};
    use crate::report::view::Accounts;
    use serde_json::json;

    fn blocks(a: &crate::report::Allocation) -> Vec<serde_json::Value> {
        json(&view(a, &mut Accounts::new()))["blocks"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn no_holdings_says_so() {
        let mut a = snapshot(vec![], 1_000).allocation;
        a.accounts.clear();
        a.summary.clear();
        assert_eq!(blocks(&a), vec![json!({"t": "Note", "text": NO_HOLDINGS})]);
    }

    #[test]
    fn no_composition_says_so() {
        let mut a = snapshot(vec![], 1_000).allocation;
        a.summary.clear();
        a.lookthrough = allocation::Allocation::default();
        let drawn = blocks(&a);
        assert_eq!(drawn[0], json!({"t": "Note", "text": NO_COMPOSITION}));
        assert_eq!(drawn[1]["t"], "Heading", "the accounts still list");
    }

    #[test]
    fn the_portfolio_is_a_heading_a_summary_a_bar_then_each_account() {
        let a = snapshot(vec![], 1_000).allocation;
        let drawn = blocks(&a);
        assert_eq!(drawn[0], json!({"t": "Heading", "text": "Allocation"}));
        let at = |i: usize| drawn[i]["t"].as_str().unwrap().to_string();
        let from = if drawn[1]["t"] == "Note" { 2 } else { 1 };
        assert_eq!((at(from), at(from + 1)), ("Table".into(), "Bar".into()));
        assert_eq!(drawn[from]["class"], "fs");
        assert_eq!(
            drawn[from]["head"],
            json!(["Class", "Target", "Actual", "\u{394}"])
        );
        let fh = drawn
            .iter()
            .find(|b| b["class"] == "fh")
            .expect("no holdings table");
        assert_eq!(fh["head"], json!(["Ticker", "Balance", "As of"]));
        assert!(
            fh["rows"].to_string().contains("\"--\""),
            "no as-of dash: {fh}"
        );
    }

    #[test]
    fn a_short_class_delta_is_negative() {
        let a = snapshot(vec![], 1_000).allocation;
        let drawn = blocks(&a);
        let rows = drawn.iter().find(|b| b["class"] == "fs").unwrap()["rows"].to_string();
        assert!(rows.contains(",\"neg\"]"), "no toned delta: {rows}");
    }

    #[test]
    fn the_bar_segments_never_exceed_the_whole() {
        let a = snapshot(vec![], 1_000).allocation;
        for block in blocks(&a).iter().filter(|b| b["t"] == "Bar") {
            let total: f64 = block["segments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s[0].as_str().unwrap().parse::<f64>().unwrap())
                .sum();
            assert!(total <= 100.0 + 1e-9, "{total}");
            assert_eq!(block["legend"].as_array().unwrap().len(), Class::ALL.len());
            assert!(
                block["legend"][0][0]
                    .as_str()
                    .unwrap()
                    .starts_with(Class::ALL[0].label())
            );
            assert_eq!(block["legend"][0][1], 0);
        }
    }

    #[test]
    fn coverage_is_a_note() {
        let a = snapshot(vec![], 1_000).allocation;
        let drawn = blocks(&a);
        assert!(
            drawn.iter().any(
                |b| b["t"] == "Note" && b["text"].as_str().unwrap().contains("3 of 4 holdings")
            ),
            "{drawn:?}"
        );
    }
}
