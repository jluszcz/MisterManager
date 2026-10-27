//! The Overview tab: the three projection columns, banded.

use super::{account, escape, money};
use crate::overview::{Balances, Overview, Section};
use crate::palette::Sense;
use crate::report::view::{Accounts, Block, Cell, Row, TabView, money as money_cell};

/// The three `Balances` columns, in Overview order: To-Date, Paycheck-Eve,
/// Month-End. Keeps its cents, the same precision the Overview screen shows.
fn balance_cells(b: Balances) -> String {
    format!(
        "{}{}{}",
        money(b.to_date.to_string(), b.to_date),
        money(b.adhoc.to_string(), b.adhoc),
        money(b.month_end.to_string(), b.month_end),
    )
}

/// A subtotal row: an unaccounted label, bold, rather than a tinted account.
fn subtotal_row(label: &str, balances: Balances) -> String {
    format!(
        "<tr><td><strong>{}</strong></td>{}</tr>",
        escape(label),
        balance_cells(balances)
    )
}

/// One kind's bands as a `<tbody>`: an account row per `Line`, a band subtotal
/// only when the kind splits into more than one band, and the kind's own total
/// -- the same three levels the Overview screen draws, in the same order.
fn section(section: &Section) -> String {
    let mut rows = String::new();
    for band in &section.bands {
        for line in &band.lines {
            let label = match &line.account {
                Some(a) => account(a),
                None => escape(&line.label),
            };
            rows.push_str(&format!(
                "<tr><td>{label}</td>{}</tr>",
                balance_cells(line.balances)
            ));
        }
        if section.breaks_down() {
            rows.push_str(&subtotal_row(band.group.label(), band.total));
        }
    }
    rows.push_str(&subtotal_row(section.kind.label(), section.total));
    format!("<tbody>{rows}</tbody>")
}

/// Net, in a group of its own: it belongs to neither section, and summing it
/// inside one would put a total under a heading that does not own it.
fn net_group(net: Balances) -> String {
    format!("<tbody>{}</tbody>", subtotal_row("Net", net))
}

/// The Overview as one table, the three dates its header row.
///
/// One table and not three, because a header only labels the table it sits in
/// and a column only lines up with the column above it. Split into a table
/// per section, each would size its columns to its own longest figure, so
/// Cash would not line up with Credit and only the first would be dated. The
/// sections keep their separation as `<tbody>` groups instead.
///
/// The dates alone, no `To-Date`/`Paycheck-Eve`/`Month-End` above them: the
/// Overview screen's own header is the bare dates, and this page has the
/// narrower width of the two.
pub(super) fn table(overview: &Overview) -> String {
    let dates = overview.dates;
    let header = [dates.to_date, dates.adhoc, dates.month_end]
        .map(|d| format!("<th class=\"n\">{d}</th>"))
        .join("");
    format!(
        "<table><thead><tr><th></th>{header}</tr></thead>{}{}{}</table>",
        section(&overview.cash),
        section(&overview.credit),
        net_group(overview.net),
    )
}

/// A balance row's three money cells, to the cent.
fn money_cells(b: Balances) -> Vec<Cell> {
    [b.to_date, b.adhoc, b.month_end]
        .into_iter()
        .map(|c| money_cell(Sense::Natural, c.to_string(), c))
        .collect()
}

fn view_row(first: Cell, balances: Balances, total: bool) -> Row {
    let mut c = vec![first];
    c.extend(money_cells(balances));
    Row {
        c,
        k: total.then(|| "tot".to_string()),
        h: None,
        d: None,
        m: None,
    }
}

/// The Overview as data: one table of the same rows [`table`] draws. Each
/// section's first row carries `sep`, which is where the script rules a line
/// between the sections the markup keeps as separate `<tbody>`s.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn view(o: &Overview, acc: &mut Accounts) -> TabView {
    let d = o.dates;
    let head = std::iter::once(String::new())
        .chain([d.to_date, d.adhoc, d.month_end].map(|d| d.format("%Y-%m-%d").to_string()))
        .collect();
    let mut rows = Vec::new();
    let mut push_section = |section: &Section, rows: &mut Vec<Row>| {
        let start = rows.len();
        for band in &section.bands {
            for line in &band.lines {
                let label = match &line.account {
                    Some(a) => acc.cell(a),
                    None => Cell::Text(line.label.clone()),
                };
                rows.push(view_row(label, line.balances, false));
            }
            if section.breaks_down() {
                let label = Cell::Text(band.group.label().to_string());
                rows.push(view_row(label, band.total, true));
            }
        }
        let label = Cell::Text(section.kind.label().to_string());
        rows.push(view_row(label, section.total, true));
        mark_separated(&mut rows[start]);
    };
    push_section(&o.cash, &mut rows);
    push_section(&o.credit, &mut rows);
    let start = rows.len();
    rows.push(view_row(Cell::Text("Net".to_string()), o.net, true));
    mark_separated(&mut rows[start]);
    TabView {
        id: "overview",
        blocks: vec![Block::Table {
            class: "ov",
            head,
            rows,
        }],
    }
}

fn mark_separated(row: &mut Row) {
    row.k = Some(match row.k.take() {
        Some(k) => format!("sep {k}"),
        None => "sep".to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, panel, row, snapshot, tables};
    use super::super::page;
    use super::view;
    use crate::report::view::Accounts;
    use serde_json::json;

    fn table(snapshot: &crate::report::Snapshot) -> serde_json::Value {
        json(&view(&snapshot.overview, &mut Accounts::new()))["blocks"][0].clone()
    }

    #[test]
    fn the_three_projection_dates_head_the_overview() {
        let t = table(&snapshot(vec![], 1_000));
        assert_eq!(t["t"], "Table");
        assert_eq!(t["class"], "ov");
        assert_eq!(
            t["head"],
            json!(["", "2026-08-21", "2026-08-27", "2026-09-01"])
        );
    }

    #[test]
    fn credit_totals_are_bold_and_net_closes_the_table() {
        let t = table(&snapshot(vec![], 1_000));
        let rows = t["rows"].as_array().unwrap();
        let label = |r: &serde_json::Value| r["c"][0].clone();
        // Cash: one account line, then its total; Credit likewise; then Net.
        assert_eq!(rows.len(), 5);
        assert_eq!(label(&rows[1]), "Cash");
        assert_eq!(rows[1]["k"], "tot");
        assert_eq!(label(&rows[3]), "Credit");
        assert_eq!(rows[3]["k"], "tot");
        let net = rows.last().unwrap();
        assert_eq!(label(net), "Net");
        assert_eq!(net["k"], "sep tot");
        assert_eq!(net["c"][1], "1,000.00");
    }

    #[test]
    fn each_sections_first_row_is_set_apart_and_a_single_band_has_no_subtotal() {
        let t = table(&snapshot(vec![], 1_000));
        let rows = t["rows"].as_array().unwrap();
        assert_eq!(rows[0]["k"], "sep");
        assert_eq!(rows[2]["k"], "sep");
        assert!(rows[0]["c"][0].is_number(), "an account is an index");
    }

    #[test]
    fn a_section_in_two_bands_subtotals_each_and_only_its_first_row_is_set_apart() {
        use crate::db::account::{Group, Kind};
        use crate::overview::{Balances, Band, Line};
        let b = |d| Balances {
            to_date: crate::money::Cents::from_dollars(d),
            adhoc: crate::money::Cents::from_dollars(d),
            month_end: crate::money::Cents::from_dollars(d),
        };
        let line = |label: &str, d| Line {
            account: None,
            label: label.to_string(),
            balances: b(d),
        };
        let mut s = snapshot(vec![], 0);
        s.overview.cash.bands = vec![
            Band {
                group: Group::Checking,
                lines: vec![line("Everyday", 100), line("Brokerage", 50)],
                total: b(150),
            },
            Band {
                group: Group::Savings,
                lines: vec![line("Rainy Day", 400)],
                total: b(400),
            },
        ];
        s.overview.cash.total = b(550);
        assert_eq!(s.overview.cash.kind, Kind::Cash);
        let t = table(&s);
        let rows = t["rows"].as_array().unwrap();
        let labels: Vec<_> = rows.iter().map(|r| r["c"][0].clone()).collect();
        assert_eq!(
            labels[..6],
            [
                json!("Everyday"),
                json!("Brokerage"),
                json!(Group::Checking.label()),
                json!("Rainy Day"),
                json!(Group::Savings.label()),
                json!("Cash"),
            ]
        );
        for i in [2, 4, 5] {
            assert_eq!(rows[i]["k"], "tot", "row {i}");
        }
        assert_eq!(rows[2]["c"][1], "150.00");
        let separated: Vec<_> = (0..rows.len())
            .filter(|&i| rows[i]["k"].as_str().is_some_and(|k| k.contains("sep")))
            .collect();
        // Cash's first row, Credit's first row, and Net.
        assert_eq!(separated, [0, 6, rows.len() - 1]);
    }

    #[test]
    fn a_negative_cash_balance_is_toned() {
        let t = table(&snapshot(vec![], -1_000));
        assert_eq!(t["rows"][0]["c"][1], json!(["-1,000.00", "neg"]));
    }

    /// A date labels the column beneath it, so it has to be that column's
    /// header: set above the table as a line of prose it lined up with
    /// nothing. And one table over all three sections, since separate tables
    /// size their columns independently -- Cash would not line up with
    /// Credit, and only the first would carry the dates at all.
    #[test]
    fn the_dates_head_the_columns_they_quote() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let panel = panel(&page, "overview");
        assert!(
            panel.contains(
                "<thead><tr><th></th>\
                 <th class=\"n\">2026-08-21</th>\
                 <th class=\"n\">2026-08-27</th>\
                 <th class=\"n\">2026-09-01</th></tr></thead>"
            ),
            "the dates do not head the columns: {panel}"
        );
        assert_eq!(
            tables(panel).len(),
            1,
            "the sections are in tables that size themselves apart"
        );
    }

    #[test]
    fn a_negative_net_is_drawn_in_the_negative_color() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], -1_000));
        let negative = crate::palette::hex(crate::palette::NEGATIVE);
        assert!(
            page.contains(&negative),
            "a negative figure took no warning color"
        );
    }
}
