//! The Overview tab: the three projection columns, banded.

use super::{account, escape, money};
use crate::overview::{Balances, Overview, Section};

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

#[cfg(test)]
mod tests {
    use super::super::fixture::{panel, row, snapshot, tables};
    use super::super::page;

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
