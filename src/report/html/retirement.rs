//! The Retirement tab: a spelling of [`crate::retirement`], the same model
//! screen 8 draws. A status wears an inline color off
//! [`palette::standing`], the way every other tab colors a figure. Under the
//! tables, the screen's two charts, spelled from
//! [`crate::balance_history::Charts`] as inline SVG.

use super::{account, escape, money};
use crate::balance_history::{CASH_TITLE, Chart, Charts, INVESTMENT_TITLE, NO_HISTORY};
use crate::calc::Month;
use crate::calc::retirement::{self, BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::palette;
use crate::report::view::{Accounts, Block, Cell, Row, TabView, money as money_cell};
use crate::retirement::Retirement;

/// What a figure that cannot be stated draws as -- the screen's word too.
const ABSENT: &str = "--";

/// A whole-dollar figure with its `$`. Every dollar figure on this tab
/// carries one -- the balances, the gaps, the salary and the milestone
/// targets -- so a column of balances cannot read as a column of multiples
/// beside it.
fn dollars(cents: Cents) -> String {
    format!("${}", cents.trunc_to_dollar().to_whole_dollars())
}

/// [`dollars`] in a money cell, colored as every other money cell is.
fn dollar_cell(cents: Cents) -> String {
    money(dollars(cents), cents.trunc_to_dollar())
}

fn compact(cents: Cents) -> String {
    format!("${}", retirement::compact(cents))
}

fn dollar_band(band: Band<Cents>) -> String {
    match band.low == band.high {
        true => compact(band.low),
        false => format!("{}{BAND_DASH}{}", compact(band.low), compact(band.high)),
    }
}

fn status(status: Status, short_by: Option<Cents>) -> String {
    let text = match short_by {
        Some(gap) => format!("{} {}", status.label(), dollars(gap)),
        None => status.label().to_string(),
    };
    format!(
        "<td style=\"color:{}\">{}</td>",
        palette::hex(palette::standing(status)),
        escape(&text)
    )
}

/// The two standings, or the one thing standing between the owner and them.
fn standing(r: &Retirement) -> String {
    if r.held.is_empty() {
        return "<p>Mark investment accounts as Retirement on the Accounts screen.</p>".into();
    }
    let Some(now) = r.now() else {
        return "<p>Set a birth date on the Retirement screen.</p>".into();
    };
    let saved = match (r.multiple(), r.saved_status()) {
        (Some(multiple), Some((s, gap))) => format!(
            "<tr><td>Saved</td>{}<td class=\"n\">{}</td><td class=\"n\">{}</td>{}</tr>",
            dollar_cell(r.saved),
            escape(&multiple.to_string()),
            escape(&now.saved.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Saved</td>{}<td colspan=\"3\">Set a salary on the Retirement screen.</td></tr>",
            dollar_cell(r.saved)
        ),
    };
    let tax_free = match (r.tax_free_share(), r.tax_free_status()) {
        (Some(share), Some((s, gap))) => format!(
            "<tr><td>Tax-free</td>{}<td class=\"n\">{}%</td><td class=\"n\">{}</td>{}</tr>",
            dollar_cell(r.tax_free),
            share.tenth_percent(),
            escape(&now.tax_free.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Tax-free</td>{}<td colspan=\"3\">{ABSENT}</td></tr>",
            dollar_cell(r.tax_free)
        ),
    };
    format!(
        "<table><thead><tr><th></th><th class=\"n\">Balance</th><th class=\"n\">Now</th>\
         <th class=\"n\">Target</th><th></th></tr></thead>\
         <tbody>{saved}{tax_free}</tbody></table>"
    )
}

fn milestones(r: &Retirement) -> String {
    let rows: String = r
        .rows
        .iter()
        .map(|row| {
            let label = match row.now {
                true => format!("Now ({})", row.age),
                false => format!("By {}", row.age),
            };
            let multiple = match r.salary {
                Some(_) => row.saved.to_string(),
                None => ABSENT.to_string(),
            };
            let target = row
                .saved_dollars
                .map(dollar_band)
                .unwrap_or_else(|| ABSENT.to_string());
            format!(
                "<tr><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
                escape(&label),
                escape(&multiple),
                escape(&target),
                escape(&row.tax_free.to_string())
            )
        })
        .collect();
    format!(
        "<table class=\"bands\"><thead><tr><th>Milestone</th><th class=\"n\">× salary</th>\
         <th class=\"n\">Saved $</th><th class=\"n\">Tax-free</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

fn accounts(r: &Retirement) -> String {
    let rows: String = r
        .held
        .iter()
        .map(|h| {
            let share = h
                .share_of(r.saved)
                .map_or(ABSENT.to_string(), |s| format!("{s}%"));
            format!(
                "<tr><td>{}</td><td>{}</td>{}<td class=\"n\">{share}</td></tr>",
                account(&h.account),
                escape(&h.tax_label()),
                dollar_cell(h.balance)
            )
        })
        .collect();
    format!(
        "<table><thead><tr><th>Account</th><th>Tax</th><th class=\"n\">Balance</th>\
         <th class=\"n\">Share</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

/// The SVG's own coordinate space. It scales to the panel's width, so these
/// are proportions rather than pixels: wide enough for a legible line on a
/// phone, the left margin holding the widest compact figure and the bottom
/// one the month labels.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 180.0;
const LEFT: f64 = 46.0;
const RIGHT: f64 = 8.0;
const TOP: f64 = 8.0;
const BOTTOM: f64 = 20.0;

/// Where a month sits across the plot. A single month is centred, as on the
/// screen, rather than pinned to the left edge.
fn x((first, last): (Month, Month), month: Month) -> f64 {
    let plot = WIDTH - LEFT - RIGHT;
    match first == last {
        true => LEFT + plot / 2.0,
        false => {
            let span = f64::from(last.ordinal() - first.ordinal());
            LEFT + f64::from(month.ordinal() - first.ordinal()) / span * plot
        }
    }
}

/// Where a figure sits up the plot -- `charts.cents` being the same for
/// both charts, a height is one figure on either.
fn y((low, high): (Cents, Cents), cents: Cents) -> f64 {
    let plot = HEIGHT - TOP - BOTTOM;
    TOP + (high.0 - cents.0) as f64 / (high.0 - low.0) as f64 * plot
}

/// How a line is inked. The page's own markup spells a color on the element;
/// the data form leaves it to the element's class and the stylesheet.
#[derive(Clone, Copy)]
enum Ink<'a> {
    Hex(&'a str),
    Class,
}

/// One line, as a polyline -- and, where it is a single month, a dot, since
/// a polyline of one point draws nothing.
fn line(charts: &Charts, points: &[(Month, Cents)], ink: Ink, class: &str) -> String {
    let coords: Vec<(f64, f64)> = points
        .iter()
        .map(|(m, c)| (x(charts.months, *m), y(charts.cents, *c)))
        .collect();
    match coords.as_slice() {
        [(cx, cy)] => {
            let fill = match ink {
                Ink::Hex(hex) => format!(" fill=\"{hex}\""),
                Ink::Class => String::new(),
            };
            format!("<circle class=\"{class}\" cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"3\"{fill}/>")
        }
        _ => {
            let points: Vec<String> = coords
                .iter()
                .map(|(px, py)| format!("{px:.1},{py:.1}"))
                .collect();
            let stroke = match ink {
                Ink::Hex(hex) => format!(" stroke=\"{hex}\""),
                Ink::Class => String::new(),
            };
            format!(
                "<polyline class=\"{class}\" points=\"{}\" fill=\"none\"{stroke} \
                 stroke-width=\"2\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
                points.join(" ")
            )
        }
    }
}

/// The gridlines with their figures and the month labels under them: what
/// every chart draws before its first line.
fn frame(charts: &Charts) -> String {
    let mut svg = String::new();
    for cents in charts.ticks() {
        let at = y(charts.cents, cents);
        svg.push_str(&format!(
            "<line class=\"grid\" x1=\"{LEFT}\" x2=\"{}\" y1=\"{at:.1}\" y2=\"{at:.1}\"/>\
             <text class=\"axis\" x=\"{}\" y=\"{at:.1}\" text-anchor=\"end\" \
             dominant-baseline=\"middle\">{}</text>",
            WIDTH - RIGHT,
            LEFT - 4.0,
            escape(&compact(cents))
        ));
    }
    let (first, last) = charts.months;
    let base = HEIGHT - 4.0;
    match first == last {
        true => svg.push_str(&format!(
            "<text class=\"axis\" x=\"{:.1}\" y=\"{base}\" text-anchor=\"middle\">{}</text>",
            x(charts.months, first),
            first.label()
        )),
        false => svg.push_str(&format!(
            "<text class=\"axis\" x=\"{LEFT}\" y=\"{base}\" text-anchor=\"start\">{}</text>\
             <text class=\"axis\" x=\"{}\" y=\"{base}\" text-anchor=\"end\">{}</text>",
            first.label(),
            WIDTH - RIGHT,
            last.label()
        )),
    }
    svg
}

/// One chart: gridlines at the three figures the screen labels, the
/// accounts in their own colors, the Total over them where there is one, and
/// a legend under it.
///
/// The Total is drawn last so it is never hidden under an account it sums,
/// and carries the `total` class so the dark scheme can swap its ink; its
/// legend entry is a swatch beside plain text, being no account's.
fn chart(title: &str, chart: &Chart, charts: &Charts, empty: &str) -> String {
    let heading = format!("<h4>{}</h4>", escape(title));
    if chart.series.is_empty() {
        return format!("{heading}<p class=\"note\">{}</p>", escape(empty));
    }
    let mut svg = frame(charts);
    let mut legend = String::new();
    for series in &chart.series {
        let color = series
            .account
            .render_with(|_, color| palette::hex(palette::account(color)));
        svg.push_str(&line(charts, &series.points, Ink::Hex(&color), "series"));
        legend.push_str(&format!(
            "<span class=\"item\"><span class=\"key\" style=\"background:{color}\"></span>{}</span>",
            account(&series.account)
        ));
    }
    if !chart.total.is_empty() {
        let total = palette::hex(palette::TOTAL);
        svg.push_str(&line(charts, &chart.total, Ink::Hex(&total), "total"));
        legend.push_str(&format!(
            "<span class=\"item\"><span class=\"key total\" style=\"background:{total}\"></span>Total</span>"
        ));
    }
    format!(
        "{heading}<svg class=\"chart\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"img\" \
         aria-label=\"{}\">{svg}</svg><p class=\"legend\">{legend}</p>",
        escape(title)
    )
}

/// Both charts, stacked: side by side they would each get half a phone.
/// The whole history, since a page has no key to move a window -- and the
/// same message as the screen's where a chart has nothing at all.
fn charts(charts: Option<&Charts>) -> String {
    let Some(charts) = charts else {
        return [CASH_TITLE, INVESTMENT_TITLE]
            .iter()
            .map(|title| format!("<h4>{title}</h4><p class=\"note\">{NO_HISTORY}</p>"))
            .collect();
    };
    format!(
        "{}{}",
        chart(CASH_TITLE, &charts.cash, charts, NO_HISTORY),
        chart(INVESTMENT_TITLE, &charts.investment, charts, NO_HISTORY)
    )
}

pub(super) fn panel(r: &Retirement, history: Option<&Charts>) -> String {
    // The milestones stand on their own, as on the screen; only the account
    // list needs something marked.
    let held = match r.held.is_empty() {
        true => String::new(),
        false => accounts(r),
    };
    format!(
        "<h3>{}</h3>{}{}{held}{}",
        escape(&title(r)),
        standing(r),
        milestones(r),
        charts(history)
    )
}

fn plain_row(c: Vec<Cell>) -> Row {
    Row {
        c,
        k: None,
        h: None,
        d: None,
        m: None,
    }
}

/// [`dollar_cell`] as a cell.
fn dollar_data(cents: Cents) -> Cell {
    money_cell(
        palette::Sense::Natural,
        dollars(cents),
        cents.trunc_to_dollar(),
    )
}

/// [`status`] as a cell: the tone is a class, `neg`, `mid` or `pos`.
fn status_cell(status: Status, short_by: Option<Cents>) -> Cell {
    let text = match short_by {
        Some(gap) => format!("{} {}", status.label(), dollars(gap)),
        None => status.label().to_string(),
    };
    let tone = match status {
        Status::Short => "neg",
        Status::OnTrack => "mid",
        Status::Ahead => "pos",
    };
    Cell::Toned(text, tone.to_string())
}

fn text(s: impl Into<String>) -> Cell {
    Cell::Text(s.into())
}

/// [`standing`] as a block: the table, or the sentence in its place.
fn standing_block(r: &Retirement) -> Block {
    if r.held.is_empty() {
        return Block::Note {
            text: "Mark investment accounts as Retirement on the Accounts screen.".into(),
        };
    }
    let Some(now) = r.now() else {
        return Block::Note {
            text: "Set a birth date on the Retirement screen.".into(),
        };
    };
    let saved = match (r.multiple(), r.saved_status()) {
        (Some(multiple), Some((s, gap))) => plain_row(vec![
            text("Saved"),
            dollar_data(r.saved),
            text(multiple.to_string()),
            text(now.saved.to_string()),
            status_cell(s, gap),
        ]),
        _ => plain_row(vec![
            text("Saved"),
            dollar_data(r.saved),
            text("Set a salary on the Retirement screen."),
        ]),
    };
    let tax_free = match (r.tax_free_share(), r.tax_free_status()) {
        (Some(share), Some((s, gap))) => plain_row(vec![
            text("Tax-free"),
            dollar_data(r.tax_free),
            text(format!("{}%", share.tenth_percent())),
            text(now.tax_free.to_string()),
            status_cell(s, gap),
        ]),
        _ => plain_row(vec![
            text("Tax-free"),
            dollar_data(r.tax_free),
            text(ABSENT),
        ]),
    };
    Block::Table {
        class: "rs",
        head: ["", "Balance", "Now", "Target", ""]
            .map(String::from)
            .to_vec(),
        rows: vec![saved, tax_free],
    }
}

fn milestones_block(r: &Retirement) -> Block {
    let rows = r
        .rows
        .iter()
        .map(|row| {
            let label = match row.now {
                true => format!("Now ({})", row.age),
                false => format!("By {}", row.age),
            };
            let multiple = match r.salary {
                Some(_) => row.saved.to_string(),
                None => ABSENT.to_string(),
            };
            let target = row
                .saved_dollars
                .map(dollar_band)
                .unwrap_or_else(|| ABSENT.to_string());
            plain_row(vec![
                text(label),
                text(multiple),
                text(target),
                text(row.tax_free.to_string()),
            ])
        })
        .collect();
    Block::Table {
        class: "rm",
        head: ["Milestone", "\u{d7} salary", "Saved $", "Tax-free"]
            .map(String::from)
            .to_vec(),
        rows,
    }
}

fn accounts_block(r: &Retirement, acc: &mut Accounts) -> Block {
    let rows = r
        .held
        .iter()
        .map(|h| {
            let share = h
                .share_of(r.saved)
                .map_or(ABSENT.to_string(), |s| format!("{s}%"));
            plain_row(vec![
                acc.cell(&h.account),
                text(h.tax_label()),
                dollar_data(h.balance),
                text(share),
            ])
        })
        .collect();
    Block::Table {
        class: "ra",
        head: ["Account", "Tax", "Balance", "Share"]
            .map(String::from)
            .to_vec(),
        rows,
    }
}

/// [`chart`] as data: the same coordinates, every line inked by class
/// (`ln a-<color>`, `ln total`) rather than by a hex on the element.
fn chart_block(
    title: &str,
    chart: &Chart,
    charts: &Charts,
    empty: &str,
    acc: &mut Accounts,
) -> Vec<Block> {
    if chart.series.is_empty() {
        return vec![
            Block::Subheading {
                text: title.to_string(),
            },
            Block::Note {
                text: empty.to_string(),
            },
        ];
    }
    let mut svg = frame(charts);
    let mut legend = Vec::new();
    for series in &chart.series {
        let class = series
            .account
            .render_with(|_, color| format!("ln a-{}", color.as_str()));
        svg.push_str(&line(charts, &series.points, Ink::Class, &class));
        legend.push(acc.cell(&series.account));
    }
    if !chart.total.is_empty() {
        svg.push_str(&line(charts, &chart.total, Ink::Class, "ln total"));
        legend.push(Cell::Toned("Total".to_string(), "total".to_string()));
    }
    vec![Block::Chart {
        title: title.to_string(),
        svg,
        legend,
    }]
}

/// The Retirement tab as data, in the order [`panel`] draws it.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn view(r: &Retirement, history: Option<&Charts>, acc: &mut Accounts) -> TabView {
    let mut blocks = vec![
        Block::Heading {
            text: Cell::Text(title(r)),
        },
        standing_block(r),
        milestones_block(r),
    ];
    if !r.held.is_empty() {
        blocks.push(accounts_block(r, acc));
    }
    match history {
        Some(charts) => {
            for (name, chart) in [
                (CASH_TITLE, &charts.cash),
                (INVESTMENT_TITLE, &charts.investment),
            ] {
                blocks.extend(chart_block(name, chart, charts, NO_HISTORY, acc));
            }
        }
        None => {
            for name in [CASH_TITLE, INVESTMENT_TITLE] {
                blocks.push(Block::Subheading {
                    text: name.to_string(),
                });
                blocks.push(Block::Note {
                    text: NO_HISTORY.to_string(),
                });
            }
        }
    }
    TabView {
        id: "retirement",
        blocks,
    }
}

/// The heading both forms open on.
fn title(r: &Retirement) -> String {
    let mut title = String::from("Retirement");
    if let Some(age) = r.age {
        title.push_str(&format!(" \u{b7} Age {age}"));
    }
    if let Some(salary) = r.salary {
        title.push_str(&format!(" \u{b7} Salary {}", dollars(salary)));
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::retirement::Status;
    use crate::db::AccountId;
    use crate::db::account::AccountColor;
    use crate::db::account::TaxTreatment;
    use crate::retirement::{Held, rows};

    /// Thirty-seven, $330,000 saved against a $100,000 salary: short of
    /// 3.40× by $10,000, with nothing held tax-free.
    fn retirement() -> Retirement {
        let accounts = super::super::fixture::accounts();
        let saved = Cents::from_dollars(330_000);
        let salary = Cents::from_dollars(100_000);
        Retirement {
            age: Some(37),
            salary: Some(salary),
            saved,
            tax_free: Cents::ZERO,
            held: vec![Held {
                account: crate::account_label::Account::named(&accounts, AccountId(3)),
                treatment: TaxTreatment::TaxDeferred,
                tax_free: Cents::ZERO,
                tax_free_share: crate::rate::BasisPoints::ZERO,
                balance: saved,
            }],
            rows: rows(Some(37), Some(salary)),
        }
    }

    #[test]
    fn a_short_standing_wears_the_negative_color_and_names_the_gap() {
        let html = panel(&retirement(), None);
        assert!(
            html.contains(&palette::hex(palette::standing(Status::Short))),
            "{html}"
        );
        assert!(html.contains("Short $10,000"), "{html}");
    }

    #[test]
    fn the_table_starts_at_now() {
        let html = panel(&retirement(), None);
        assert!(html.contains("Now (37)"), "{html}");
        assert!(!html.contains("By 35"), "{html}");
        assert!(html.contains("4.00 – 4.50×"), "{html}");
        // 4.00–4.50× of $100,000.
        assert!(html.contains("$400K – $450K"), "{html}");
    }

    #[test]
    fn nothing_marked_says_where_to_mark_it() {
        let html = panel(&Retirement::default(), None);
        assert!(
            html.contains("Mark investment accounts as Retirement"),
            "{html}"
        );
    }

    #[test]
    fn every_dollar_figure_carries_its_sign() {
        let html = panel(&retirement(), None);
        assert!(html.contains(">$330,000</td>"), "{html}");
        assert!(!html.contains(">330,000</td>"), "{html}");
    }

    #[test]
    fn every_header_is_aligned_with_its_column() {
        let html = panel(&retirement(), None);
        for table in super::super::fixture::tables(&html) {
            assert_eq!(
                super::super::fixture::misaligned_headers(table),
                Vec::<String>::new(),
                "{table}"
            );
        }
    }

    /// Nothing held tax-free against 11.0% of 37's $340,000 target.
    #[test]
    fn a_short_tax_free_standing_names_the_dollars_of_the_target() {
        let html = panel(&retirement(), None);
        assert!(html.contains("Short $37,400"), "{html}");
    }

    fn month(m: u32) -> Month {
        Month::of(crate::test_support::day(2026, m, 1))
    }

    /// Rainy Day alone from March through August, and two investment
    /// accounts: Holdings from July, Long Haul recorded only in August. The
    /// investment Total is $10K in July and $20K in August.
    fn charts() -> Charts {
        use crate::balance_history::{History, Series};
        let accounts = super::super::fixture::accounts();
        let named = |id| crate::account_label::Account::named(&accounts, AccountId(id));
        let history = History {
            cash: vec![Series {
                account: named(1),
                points: (3..=8)
                    .map(|m| (month(m), Cents::from_dollars(1_000 * i64::from(m))))
                    .collect(),
            }],
            credit: Vec::new(),
            investment: vec![
                Series {
                    account: named(2),
                    points: vec![
                        (month(7), Cents::from_dollars(10_000)),
                        (month(8), Cents::from_dollars(12_000)),
                    ],
                },
                Series {
                    account: named(3),
                    points: vec![(month(8), Cents::from_dollars(8_000))],
                },
            ],
        };
        Charts::whole(&history).unwrap()
    }

    #[test]
    fn each_chart_draws_its_accounts_in_their_colors_and_the_total_in_black() {
        let charts = charts();
        let html = panel(&retirement(), Some(&charts));
        assert_eq!(html.matches("<svg class=\"chart\"").count(), 2, "{html}");
        for color in [
            AccountColor::Teal,
            AccountColor::Copper,
            AccountColor::Violet,
        ] {
            let hex = palette::hex(palette::account(color));
            assert!(
                html.contains(&format!("stroke=\"{hex}\""))
                    || html.contains(&format!("fill=\"{hex}\"")),
                "{hex} not drawn: {html}"
            );
        }
        assert!(
            html.contains("class=\"total\" points="),
            "the investment Total is no line: {html}"
        );
        assert!(html.contains(&palette::hex(palette::TOTAL)), "{html}");
    }

    /// Cash is one account, whose Total would be its own line again drawn
    /// over it in black.
    #[test]
    fn a_chart_of_one_account_draws_no_total() {
        let charts = charts();
        let html = panel(&retirement(), Some(&charts));
        assert_eq!(html.matches(">Total</span>").count(), 1, "{html}");
        assert_eq!(html.matches("class=\"total\"").count(), 1, "{html}");
    }

    /// Long Haul's one month is drawn at August on an axis running from
    /// March, and both charts top out at the investment Total's $20K rather
    /// than cash's $8K.
    #[test]
    fn the_two_charts_share_their_months_and_their_dollars() {
        let charts = charts();
        let html = panel(&retirement(), Some(&charts));
        assert_eq!(html.matches(">Mar 2026</text>").count(), 2, "{html}");
        assert_eq!(html.matches(">$20K</text>").count(), 2, "{html}");
        assert!(!html.contains(">$8K</text>"), "{html}");
        let right = format!("cx=\"{:.1}\"", WIDTH - RIGHT);
        assert!(html.contains(&right), "Long Haul is not at August: {html}");
    }

    #[test]
    fn with_nothing_recorded_each_chart_says_so() {
        let html = panel(&retirement(), None);
        assert_eq!(html.matches("Nothing recorded yet").count(), 2, "{html}");
        assert!(!html.contains("<svg"), "{html}");
    }

    mod view_tests {
        use super::super::super::super::view::Accounts;
        use super::super::super::fixture::json;
        use super::super::view;
        use super::{charts, retirement};
        use crate::money::Cents;
        use crate::retirement::Retirement;
        use serde_json::{Value, json};

        fn blocks(r: &Retirement, c: Option<&crate::balance_history::Charts>) -> Vec<Value> {
            json(&view(r, c, &mut Accounts::new()))["blocks"]
                .as_array()
                .unwrap()
                .clone()
        }

        fn table<'a>(blocks: &'a [Value], class: &str) -> &'a Value {
            blocks
                .iter()
                .find(|b| b["class"] == class)
                .unwrap_or_else(|| panic!("no {class} table: {blocks:?}"))
        }

        #[test]
        fn retirement_without_marked_accounts_says_where_to_mark_them() {
            let drawn = blocks(&Retirement::default(), None);
            assert_eq!(drawn[0], json!({"t": "Heading", "text": "Retirement"}));
            assert_eq!(
                drawn[1],
                json!({"t": "Note",
                       "text": "Mark investment accounts as Retirement on the Accounts screen."})
            );
            assert!(
                drawn.iter().all(|b| b["class"] != "ra"),
                "no accounts to list"
            );
        }

        #[test]
        fn the_title_names_the_age_and_the_salary() {
            let drawn = blocks(&retirement(), None);
            assert_eq!(
                drawn[0]["text"],
                "Retirement \u{b7} Age 37 \u{b7} Salary $100,000"
            );
        }

        #[test]
        fn a_short_status_is_negative_and_names_the_gap() {
            let drawn = blocks(&retirement(), None);
            let saved = &table(&drawn, "rs")["rows"][0]["c"];
            assert_eq!(saved[0], "Saved");
            assert_eq!(saved[1], "$330,000");
            assert_eq!(saved[4], json!(["Short $10,000", "neg"]));
        }

        #[test]
        fn an_on_track_status_is_mid() {
            let mut r = retirement();
            r.saved = Cents::from_dollars(350_000);
            let drawn = blocks(&r, None);
            let saved = &table(&drawn, "rs")["rows"][0]["c"];
            assert_eq!(saved[4][1], "mid", "{saved}");
        }

        #[test]
        fn the_milestones_start_at_now_with_their_dollar_bands() {
            let drawn = blocks(&retirement(), None);
            let rm = table(&drawn, "rm");
            assert_eq!(
                rm["head"],
                json!(["Milestone", "\u{d7} salary", "Saved $", "Tax-free"])
            );
            assert_eq!(rm["rows"][0]["c"][0], "Now (37)");
            assert!(rm.to_string().contains("$400K \u{2013} $450K"), "{rm}");
        }

        #[test]
        fn the_held_accounts_follow_the_milestones() {
            let drawn = blocks(&retirement(), None);
            let ra = table(&drawn, "ra");
            assert_eq!(ra["head"], json!(["Account", "Tax", "Balance", "Share"]));
            assert_eq!(ra["rows"][0]["c"][3], "100.00%");
        }

        #[test]
        fn a_charts_lines_carry_their_account_class_and_no_hex() {
            let c = charts();
            let drawn = blocks(&retirement(), Some(&c));
            let charts: Vec<&Value> = drawn.iter().filter(|b| b["t"] == "Chart").collect();
            assert_eq!(charts.len(), 2);
            for chart in &charts {
                let svg = chart["svg"].as_str().unwrap();
                let bytes = svg.as_bytes();
                for (i, _) in svg.match_indices('#') {
                    assert!(
                        !bytes[i + 1..].iter().take(6).all(u8::is_ascii_hexdigit),
                        "a hex color in {svg}"
                    );
                }
                assert!(!svg.contains("stroke=\""), "{svg}");
            }
            let investment = charts[1]["svg"].as_str().unwrap();
            assert!(investment.contains("class=\"ln a-copper\""), "{investment}");
            assert!(investment.contains("class=\"ln total\""), "{investment}");
            let legend = charts[1]["legend"].as_array().unwrap();
            assert_eq!(legend.last().unwrap(), &json!(["Total", "total"]));
            assert_eq!(legend.len(), 3);
        }

        #[test]
        fn a_chart_of_one_account_draws_no_total() {
            let c = charts();
            let drawn = blocks(&retirement(), Some(&c));
            let cash = drawn.iter().find(|b| b["t"] == "Chart").unwrap();
            assert_eq!(cash["legend"].as_array().unwrap().len(), 1);
            assert!(!cash["svg"].as_str().unwrap().contains("ln total"));
        }

        #[test]
        fn with_nothing_recorded_each_chart_says_so() {
            let drawn = blocks(&retirement(), None);
            let notes = drawn
                .iter()
                .filter(|b| b["text"] == crate::balance_history::NO_HISTORY)
                .count();
            assert_eq!(notes, 2);
            assert!(drawn.iter().all(|b| b["t"] != "Chart"));
        }
    }
}
