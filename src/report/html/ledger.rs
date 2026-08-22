//! The Cash and Credit tabs: every transaction, and the month filter over it.
//!
//! The filter is a radio per month and a `:checked ~` rule per radio, wrapped
//! in a `<details>` so it reads as a dropdown. A `<select>` is what this would
//! be with a line of script, and CSS cannot see into one: no selector matches
//! "the option that is chosen", so a `<select>` here would render and do
//! nothing. Radios are the only control CSS can read.

use super::{account, escape, full_width_row, money};
use crate::db::account::Kind;
use crate::description;
use crate::report::{Ledger, LedgerMonth};

/// Date, Account, Description, Amount -- the ledger screen's own columns, in
/// its order.
const HEADER: &str = "<thead><tr><th>Date</th><th>Account</th>\
                      <th>Description</th><th class=\"n\">Amount</th></tr></thead>";

const COLUMNS: usize = 4;

/// The id every one of this ledger's controls is prefixed with.
///
/// `Kind::as_str`, so the tab, the radio names and the CSS all say `cash` and
/// `credit` because the column in the database does.
fn prefix(kind: Kind) -> &'static str {
    kind.as_str()
}

/// The id of the radio that selects one month, and of the `<tbody>` it shows.
fn month_id(kind: Kind, key: &str) -> String {
    format!("{}-{key}", prefix(kind))
}

/// One month's rows.
///
/// Oldest first inside the month, which is the ledger screen's order: a
/// running balance is read downward, and a page that reversed the rows would
/// be the only place in the application where it is not.
fn month(kind: Kind, m: &LedgerMonth) -> String {
    let rows: String = m
        .rows
        .iter()
        .map(|r| {
            // Dim, exactly as the screen draws them: a row dated past today is
            // the projection model made visible, not a payment already made.
            let class = if r.future { " class=\"future\"" } else { "" };
            format!(
                "<tr{class}><td>{}</td><td>{}</td><td>{}</td>{}</tr>",
                r.date,
                account(&r.account),
                escape(description::render(&r.description)),
                money(r.cents.to_string(), r.cents),
            )
        })
        .collect();
    format!(
        "<tbody id=\"{}-rows\">{rows}</tbody>",
        month_id(kind, &m.key)
    )
}

pub(super) fn panel(ledger: &Ledger) -> String {
    let p = prefix(ledger.kind);
    let radio = |id: &str, checked: bool| {
        let checked = if checked { " checked" } else { "" };
        format!("<input class=\"pick\" type=\"radio\" name=\"{p}-month\" id=\"{id}\"{checked}>")
    };
    // The month the page opens filtered to: the one it is being read in.
    // `All months` when that month has no rows of its own, which is what the
    // first of a month looks like -- a dropdown whose selection matched
    // nothing would draw an empty table and no reason for it.
    let opens_on = match ledger.months.iter().any(|m| m.key == ledger.current) {
        true => month_id(ledger.kind, &ledger.current),
        false => format!("{p}-all"),
    };
    // Newest first in the dropdown, oldest first in the table: the month
    // someone opens the page to check is the one they are standing in, and it
    // would otherwise be the last entry of a long list.
    let entries: Vec<(String, &str)> = std::iter::once((format!("{p}-all"), "All months"))
        .chain(
            ledger
                .months
                .iter()
                .rev()
                .map(|m| (month_id(ledger.kind, &m.key), m.label.as_str())),
        )
        .collect();
    let mut inputs = String::new();
    let mut options = String::new();
    let mut summary = String::new();
    for (id, label) in &entries {
        inputs.push_str(&radio(id, *id == opens_on));
        options.push_str(&format!("<label for=\"{id}\">{label}</label>"));
        // One of these shows at a time -- whichever radio is checked -- so the
        // closed dropdown reads as the month it is filtered to.
        summary.push_str(&format!("<span id=\"{id}-sel\">{label}</span>"));
    }
    let bodies: String = match ledger.months.is_empty() {
        // An empty ledger is a database nobody has imported into yet, and a
        // bare header row would look like a page that failed to render.
        true => full_width_row("note", COLUMNS, "no transactions".to_string()),
        false => ledger
            .months
            .iter()
            .map(|m| month(ledger.kind, m))
            .collect(),
    };
    format!(
        "{inputs}\
         <details class=\"picker\"><summary>{summary}</summary>\
         <div class=\"options\">{options}</div></details>\
         <table class=\"ledger\">{HEADER}{bodies}</table>"
    )
}

/// What each radio shows: its own month's rows, or -- for `All months` --
/// every `<tbody>` in the table.
///
/// Generated per month rather than written out, because which months exist is
/// a fact about the database and not about the page.
pub(super) fn month_rules(ledger: &Ledger) -> String {
    let p = prefix(ledger.kind);
    let show = |id: &str, rows: &str| {
        format!(
            "#{id}:checked~table.ledger {rows}{{display:table-row-group}}\
             #{id}:checked~.picker #{id}-sel{{display:inline}}"
        )
    };
    let mut css = show(&format!("{p}-all"), "tbody");
    for m in &ledger.months {
        let id = month_id(ledger.kind, &m.key);
        css.push_str(&show(&id, &format!("#{id}-rows")));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{panel, row, snapshot};
    use super::super::page;
    use crate::db::account::Kind;
    use crate::report::Ledger;

    fn cash(snapshot: &crate::report::Snapshot) -> String {
        panel(&page(snapshot), "cash").to_string()
    }

    /// The page carries every row there is, where the screen carries a
    /// window: a report that stopped at the current month would be missing
    /// exactly what someone opens it to check.
    #[test]
    fn every_month_of_the_ledger_is_on_the_page() {
        let cash = cash(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        for description in ["Groceries", "Paycheck", "Rent"] {
            assert!(cash.contains(description), "no {description} row");
        }
    }

    /// The page and the screen have to agree about what a description-less
    /// row looks like: an empty `<td>` reads as a column that failed to
    /// render, where the screen's own em dash reads as the blank the owner
    /// chose.
    #[test]
    fn a_row_with_no_description_draws_an_em_dash_here_too() {
        let mut snapshot = snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000);
        snapshot.cash.months[0].rows[0].description = String::new();
        assert!(
            cash(&snapshot).contains("<td>\u{2014}</td>"),
            "an unnamed row left an empty cell"
        );
    }

    /// The filter is the radios and the rules together: either half alone is
    /// a dropdown that changes nothing.
    #[test]
    fn each_month_has_a_radio_an_option_and_a_rule_that_shows_it() {
        let snapshot = snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000);
        let page = page(&snapshot);
        let cash = panel(&page, "cash");
        for (key, label) in [("2026-07", "Jul 2026"), ("2026-08", "Aug 2026")] {
            assert!(
                cash.contains(&format!("id=\"cash-{key}\"")),
                "no radio for {label}"
            );
            assert!(
                cash.contains(&format!("<label for=\"cash-{key}\">{label}</label>")),
                "no dropdown entry for {label}"
            );
            assert!(
                cash.contains(&format!("<tbody id=\"cash-{key}-rows\">")),
                "no rows grouped under {label}"
            );
            assert!(
                page.contains(&format!(
                    "#cash-{key}:checked~table.ledger #cash-{key}-rows{{display:table-row-group}}"
                )),
                "nothing shows {label} when it is picked"
            );
        }
    }

    /// A page read in August opens on August: every other month is one tap
    /// away, and the one being lived in is the one being checked.
    #[test]
    fn the_filter_opens_on_the_current_month() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(
            panel(&page, "cash").contains("id=\"cash-2026-08\" checked"),
            "the ledger does not open on the current month"
        );
        assert!(
            page.contains("#cash-all:checked~table.ledger tbody{display:table-row-group}"),
            "All months shows nothing"
        );
    }

    /// The first of a month, before anything has been entered in it. A
    /// selection matching no group would draw an empty table and no reason
    /// for it, so the filter falls back to the months there are.
    #[test]
    fn a_current_month_with_no_rows_falls_back_to_all_months() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.cash.current = "2026-09".into();
        let page = page(&snapshot);
        let cash = panel(&page, "cash");
        assert!(
            cash.contains("id=\"cash-all\" checked"),
            "nothing is selected: {cash}"
        );
        assert!(
            !cash.contains("id=\"cash-2026-09\""),
            "a month with no rows got an entry of its own"
        );
    }

    /// The two ledgers filter independently: one radio group across both
    /// would mean picking a month in Cash blanked Credit.
    #[test]
    fn cash_and_credit_carry_separate_filters() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(panel(&page, "cash").contains("name=\"cash-month\""));
        assert!(panel(&page, "credit").contains("name=\"credit-month\""));
    }

    /// Dim, exactly as the screen draws it: a row dated past today is the
    /// projection model made visible, not a payment already made.
    #[test]
    fn a_future_dated_row_is_marked_as_one() {
        let cash = cash(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let rent = cash.find("Rent").expect("no Rent row");
        let row_start = cash[..rent].rfind("<tr").expect("a row outside a row");
        assert!(
            cash[row_start..rent].contains("class=\"future\""),
            "a future-dated row drew as an ordinary one"
        );
    }

    /// A bare header row over nothing reads as a page that failed to render.
    #[test]
    fn an_empty_ledger_says_so() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.cash = Ledger {
            kind: Kind::Cash,
            months: Vec::new(),
            current: "2026-08".into(),
        };
        let cash = cash(&snapshot);
        assert!(
            cash.contains("no transactions"),
            "an empty ledger drew bare"
        );
        assert!(
            cash.contains("<label for=\"cash-all\">All months</label>"),
            "the dropdown lost its one entry"
        );
    }
}
