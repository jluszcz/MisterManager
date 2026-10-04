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

/// One line, as a polyline -- and, where it is a single month, a dot, since
/// a polyline of one point draws nothing.
fn line(charts: &Charts, points: &[(Month, Cents)], stroke: &str, class: &str) -> String {
    let coords: Vec<(f64, f64)> = points
        .iter()
        .map(|(m, c)| (x(charts.months, *m), y(charts.cents, *c)))
        .collect();
    match coords.as_slice() {
        [(cx, cy)] => format!(
            "<circle class=\"{class}\" cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"3\" fill=\"{stroke}\"/>"
        ),
        _ => {
            let points: Vec<String> = coords
                .iter()
                .map(|(px, py)| format!("{px:.1},{py:.1}"))
                .collect();
            format!(
                "<polyline class=\"{class}\" points=\"{}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"2\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
                points.join(" ")
            )
        }
    }
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
    let mut legend = String::new();
    for series in &chart.series {
        let color = series
            .account
            .render_with(|_, color| palette::hex(palette::account(color)));
        svg.push_str(&line(charts, &series.points, &color, "series"));
        legend.push_str(&format!(
            "<span class=\"item\"><span class=\"key\" style=\"background:{color}\"></span>{}</span>",
            account(&series.account)
        ));
    }
    if !chart.total.is_empty() {
        let total = palette::hex(palette::TOTAL);
        svg.push_str(&line(charts, &chart.total, &total, "total"));
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
    let mut title = String::from("Retirement");
    if let Some(age) = r.age {
        title.push_str(&format!(" · Age {age}"));
    }
    if let Some(salary) = r.salary {
        title.push_str(&format!(" · Salary {}", dollars(salary)));
    }
    // The milestones stand on their own, as on the screen; only the account
    // list needs something marked.
    let held = match r.held.is_empty() {
        true => String::new(),
        false => accounts(r),
    };
    format!(
        "<h3>{}</h3>{}{}{held}{}",
        escape(&title),
        standing(r),
        milestones(r),
        charts(history)
    )
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
}
