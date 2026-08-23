//! The report as one page.
//!
//! Self-contained by requirement: inline CSS, no script, no font, no image, no
//! request of any kind. A phone opening this out of a sync folder may be
//! offline, and anything fetched would render half-drawn or not at all. That
//! is also why every control here is a radio button and a sibling selector:
//! the tabs and the ledgers' month filter are CSS, because a click handler
//! would be the one thing on the page that could fail to arrive.
//!
//! One module per tab, the way `src/tui/` keeps one module per screen. What
//! they share -- escaping, a money cell, an account in its own color -- lives
//! here.

mod funds;
mod ledger;
mod overview;
mod planning;
mod savings;

#[cfg(test)]
mod fixture;

use super::Snapshot;
use crate::money::Cents;
use crate::palette;

/// Every interpolation of owner-typed text goes through here.
///
/// Correctness rather than polish: a goal named with an angle bracket would
/// otherwise truncate the page at its own row, and the figures below it would
/// simply not be there.
///
/// Escapes `&`, `<`, `>` and `"`, never `'`: the page's only double-quoted
/// attributes carrying interpolated data are `money`'s, `account`'s and
/// `savings::percent`'s `style="color:{hex}"`, and the hex always comes from
/// `palette::hex` over an enum-derived triple or the funding ramp's clamped
/// interpolation -- never owner text. A call site that puts escaped text
/// inside a single-quoted attribute reopens the injection this function
/// exists to close.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A money cell. `negative` is the one thing that colors a figure, and it is
/// the same decision `tui::style::amount_color` makes.
fn money(text: String, cents: Cents) -> String {
    let color = if cents < Cents::ZERO {
        format!(" style=\"color:{}\"", palette::hex(palette::NEGATIVE))
    } else {
        String::new()
    };
    format!("<td class=\"n\"{color}>{}</td>", escape(&text))
}

/// A money cell for an optional figure -- `per_paycheck` is `None` when a
/// goal has no runway to divide, or is already at its target.
fn optional_money(cents: Option<Cents>) -> String {
    match cents {
        Some(c) => money(c.to_whole_dollars(), c),
        None => "<td class=\"n\">--</td>".to_string(),
    }
}

/// An account, in its own color. The report's half of the rule that an
/// account never reaches a glyph without its color -- `tui::label` has the
/// other half.
fn account(account: &crate::account_label::Account) -> String {
    account.render_with(|text, color| {
        format!(
            "<span style=\"color:{}\">{}</span>",
            palette::hex(palette::account(color)),
            escape(text)
        )
    })
}

/// A row whose cells are one label spanning the table: a section heading, or
/// the sentence a table has instead of rows.
///
/// `colspan` and not a one-cell row, so the column widths a real row sets are
/// not disturbed by a line of prose sitting in the first of them.
fn full_width_row(class: &str, columns: usize, html: String) -> String {
    format!("<tr class=\"{class}\"><td colspan=\"{columns}\">{html}</td></tr>")
}

/// The tabs, in the order the screens are numbered: `1` Overview, `2` Cash,
/// `3` Credit, `4` Savings, `5` Planning, `6` Funds. An owner who reaches for
/// `4` on the keyboard should not find Planning under it here.
///
/// The id doubles as the panel's and as the CSS selector's, so a tab added
/// here is a tab wired everywhere -- including in `report`'s own check that
/// the minifier left the switch working, which reads this list rather than
/// restating it.
pub(super) const TABS: [(&str, &str); 6] = [
    ("overview", "Overview"),
    ("cash", "Cash"),
    ("credit", "Credit"),
    ("savings", "Savings"),
    ("planning", "Planning"),
    ("funds", "Funds"),
];

/// The tab bar, and the radios that drive it.
///
/// A checkbox hack rather than a click handler, because the page carries no
/// script: a phone opening this out of a sync folder still switches tabs
/// offline. Every radio sits ahead of the nav and of every panel, since
/// `:checked ~` only ever looks forward -- an input placed after the thing it
/// shows would leave the page stuck on whichever panel CSS defaulted to.
fn tab_bar() -> String {
    let inputs: String = TABS
        .iter()
        .enumerate()
        .map(|(i, (id, _))| {
            // The first tab is the one the page opens on.
            let checked = if i == 0 { " checked" } else { "" };
            format!("<input class=\"tab\" type=\"radio\" name=\"tab\" id=\"{id}\"{checked}>")
        })
        .collect();
    let labels: String = TABS
        .iter()
        .map(|(id, name)| format!("<label for=\"{id}\">{name}</label>"))
        .collect();
    format!("{inputs}<nav>{labels}</nav>")
}

/// Which panel is shown, which label is lit, and where the focus ring goes --
/// one rule set per tab, generated so that the list above stays the only
/// place a tab is named.
fn tab_rules() -> String {
    TABS.iter()
        .map(|(id, _)| {
            format!(
                "#{id}:checked~nav label[for={id}]\
                 {{color:inherit;border-bottom-color:currentColor}}\
                 #{id}:focus-visible~nav label[for={id}]\
                 {{outline:2px solid currentColor;outline-offset:-2px}}\
                 #{id}:checked~#{id}-panel{{display:block}}"
            )
        })
        .collect()
}

/// A system font stack, tabular money, a favorite's band, an expired goal's
/// marker, the tab switch, the ledgers' month dropdown, and the one media
/// query that flips background and foreground for a phone in dark mode.
/// Target a phone width; there is no `MIN_WIDTH` here.
///
/// The radios are moved off the page rather than `display:none`d, which would
/// take them out of the focus order and leave the tabs reachable by pointer
/// alone.
///
/// Three classes say what a column may do with the width it is given, and
/// between them they are what fits Savings' six columns on a phone. `n` and
/// `d` -- a figure and a date -- never wrap: both offer a break at a comma or
/// a hyphen, and a table narrow enough takes it, so `2026-08-` over `22` reads
/// as two dates rather than one and `-1,` over `000.00` as neither. Refusing
/// to wrap sets a floor under those columns, so `w` names the one that gives
/// way instead: the owner's own free text, a ledger description or a goal
/// name. It is the longest text on the page, and the only text still legible
/// broken mid-word -- every other column keeps its longest word whole, which
/// is what stops `Brokerage` becoming `Broker`/`age` beside it.
///
/// Those floors are why the panel scrolls. An account name is the owner's
/// too and takes no `w`, so a long enough one puts the table past the phone
/// whatever the description gives up; `overflow-x` on the panel is what that
/// then moves, rather than the page under every tab. It sits on the panel and
/// not on a wrapper around each table because the month filter reaches its
/// rows as `#month:checked~table.ledger`, and a `<div>` between the two would
/// leave the dropdown showing nothing.
///
/// The table face is smaller than the page's and the cells are padded by a
/// quarter of it. That is Savings' bill too: six columns, one of them a date
/// and three of them money, on the 361px a 393px phone leaves inside the
/// body's padding.
const STYLE: &str = "\
    :root{color-scheme:light dark}\
    body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;\
    margin:0 auto;max-width:32rem;padding:1rem;background:#ffffff;color:#1a1a1a}\
    h3{margin:1.2rem 0 0.4rem}\
    p.stamp{color:#666666;margin:0 0 0.6rem;font-size:0.85rem}\
    table{width:100%;border-collapse:collapse;margin-bottom:1rem;font-size:0.82rem}\
    td,th{padding:0.25rem 0.25rem;border-bottom:1px solid #dddddd;text-align:left}\
    td.n,th.n{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}\
    td.d,th.d{white-space:nowrap}\
    td.w{overflow-wrap:anywhere}\
    tr.fav{background:#fff6d8}\
    tr.expired td:first-child::after{content:' !'}\
    tr.head td{padding-top:0.9rem;font-weight:600}\
    tr.tot td{font-weight:600}\
    tr.sub td:first-child{padding-left:1.4rem;color:#666666}\
    tr.future{opacity:0.55}\
    tr.note td{color:#666666}\
    input.tab,input.pick{position:absolute;opacity:0;width:0;height:0}\
    nav{display:flex;flex-wrap:wrap;border-bottom:1px solid #dddddd;margin-bottom:0.8rem}\
    nav label{padding:0.5rem 0.7rem;margin-bottom:-1px;cursor:pointer;font-weight:600;\
    color:#666666;border-bottom:2px solid transparent}\
    section.panel{display:none;overflow-x:auto}\
    tbody+tbody tr:first-child td{padding-top:0.9rem}\
    details.picker{margin:0 0 0.8rem}\
    details.picker summary{display:inline-block;cursor:pointer;padding:0.35rem 0.7rem;\
    border:1px solid #dddddd;border-radius:0.35rem;list-style:none}\
    details.picker summary::-webkit-details-marker{display:none}\
    details.picker summary::after{content:' \\25be';color:#666666}\
    details.picker summary span{display:none}\
    details.picker div.options{border:1px solid #dddddd;border-radius:0.35rem;\
    margin-top:0.3rem;max-height:14rem;overflow-y:auto}\
    details.picker div.options label{display:block;padding:0.4rem 0.7rem;cursor:pointer}\
    table.ledger tbody{display:none}\
    footer{border-top:1px solid #dddddd;margin-top:1.5rem;padding-top:0.6rem}\
    footer p.stamp{margin:0}\
    @media (prefers-color-scheme: dark){\
    body{background:#121212;color:#eeeeee}\
    td,th{border-bottom-color:#333333}\
    nav,footer,details.picker summary,details.picker div.options{border-color:#333333}\
    tr.fav{background:#3a3315}\
    }";

/// How old the page is, in the footer: a reader arrives at the figures, and
/// checks the stamp on the way out. `%-I:%M %p` rather than a 24-hour clock,
/// because this is read on a phone alongside every other page on it.
const STAMP_FORMAT: &str = "%Y-%m-%d %-I:%M %p";

pub fn page(snapshot: &Snapshot) -> String {
    // In `TABS` order, and the zip below is what holds the two together.
    let panels = [
        overview::table(&snapshot.overview),
        ledger::panel(&snapshot.cash),
        ledger::panel(&snapshot.credit),
        savings::sections(&snapshot.containers),
        planning::block(&snapshot.planning),
        funds::table(&snapshot.funds),
    ];
    let body: String = TABS
        .iter()
        .zip(panels)
        .map(|((id, _), html)| {
            format!("<section class=\"panel\" id=\"{id}-panel\">{html}</section>")
        })
        .collect();
    // The month filters are per-ledger and per-month, so their rules cannot
    // be a constant: what months exist is a fact about the database.
    let rules = format!(
        "{}{}{}",
        tab_rules(),
        ledger::month_rules(&snapshot.cash),
        ledger::month_rules(&snapshot.credit),
    );
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Money</title><style>{STYLE}{rules}</style></head><body>\
         {}{body}\
         <footer><p class=\"stamp\">Generated {}</p></footer>\
         </body></html>",
        tab_bar(),
        snapshot.generated_at.format(STAMP_FORMAT),
    )
}

#[cfg(test)]
mod tests {
    use super::fixture::{cells, panel, row_column_counts, snapshot, tables};
    use super::*;

    /// A phone opening this out of a sync folder may be offline, and a page
    /// that fetched anything would render half-drawn or not at all. The
    /// script half is the stronger rule: every control here is CSS, so there
    /// is nothing for a blocked or broken script to take away.
    #[test]
    fn the_page_makes_no_external_request_and_carries_no_script() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        assert!(!page.contains("http"), "the page reaches out of itself");
        assert!(!page.contains("<script"), "the page carries script");
    }

    /// Six tabs, six panels, and the switch wired between them. A tab whose
    /// panel or rule went missing would render as a label that does nothing.
    #[test]
    fn every_tab_has_a_panel_and_the_page_opens_on_the_overview() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        assert!(
            page.contains("id=\"overview\" checked"),
            "the page does not open on the Overview"
        );
        for (id, name) in TABS {
            assert!(
                page.contains(&format!("<label for=\"{id}\">{name}</label>")),
                "no {id} tab"
            );
            assert!(
                page.contains(&format!("id=\"{id}-panel\"")),
                "no {id} panel"
            );
            assert!(
                page.contains(&format!("#{id}:checked~#{id}-panel")),
                "nothing switches the {id} panel on"
            );
        }
    }

    /// A hyphen is a break opportunity, so a column narrow enough draws
    /// `2026-08-` over `22` -- which reads as two dates rather than one, and
    /// is what a phone did to all three of the Overview's column headers. Every
    /// date on the page therefore sits in a cell that refuses to wrap: `n`
    /// where the date is a money column's header, `d` where it is data.
    #[test]
    fn no_date_reaches_the_page_in_a_cell_that_may_wrap() {
        /// `2026-08-21`, and nothing else on the page is shaped like it.
        fn is_a_date(text: &str) -> bool {
            let b = text.as_bytes();
            b.len() == 10
                && b[4] == b'-'
                && b[7] == b'-'
                && [0, 1, 2, 3, 5, 6, 8, 9]
                    .iter()
                    .all(|&i| b[i].is_ascii_digit())
        }

        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        let dates: Vec<_> = cells(&page)
            .into_iter()
            .filter(|(_, t)| is_a_date(t))
            .collect();
        assert!(
            dates.len() >= 5,
            "the fixture stopped carrying dates, so this test checks nothing"
        );
        for (attrs, text) in dates {
            assert!(
                attrs.contains("class=\"d\"") || attrs.contains("class=\"n\""),
                "{text} sits in a cell that may wrap: <td{attrs}>"
            );
        }
    }

    /// `<thead>` starts with `<th` as well, and reading it as a cell returns
    /// an entry running from `<tr>` to the first `</th>` -- swallowing the
    /// first header cell of every table, which is exactly the cell the check
    /// above would then never see.
    #[test]
    fn a_thead_is_not_taken_for_a_header_cell() {
        let html = "<thead><tr><th class=\"d\">2026-08-21</th><th>Date</th></tr></thead>";
        assert_eq!(
            cells(html),
            vec![(" class=\"d\"", "2026-08-21"), ("", "Date")]
        );
    }

    /// The page's job on a phone is to be honest about its own age, and the
    /// footer is where a reader looks for that.
    #[test]
    fn the_page_names_when_it_was_generated() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        assert!(
            page.contains("<footer><p class=\"stamp\">Generated 2026-08-21 2:30 PM</p></footer>"),
            "no generated-at stamp in the footer"
        );
    }

    /// A row a cell short of its neighbors either drops a column's data
    /// silently or shifts every cell after it into the wrong column -- the
    /// Unallocated row once put its figure under `%` this way. Every table on
    /// the page is checked, and a heading spanning its table counts as the
    /// columns it spans rather than as one.
    #[test]
    fn every_row_in_every_table_is_as_wide_as_its_neighbors() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        for table in tables(&page) {
            let counts = row_column_counts(table);
            let first = counts[0];
            assert!(
                counts.iter().all(|&c| c == first),
                "rows of differing width in one table: {counts:?}"
            );
        }
    }

    /// Owner-typed text reaches the page through `escape` or it does not
    /// reach it at all: a goal named with an angle bracket would otherwise
    /// truncate the page at its own row, and every figure below it would
    /// simply not be there.
    #[test]
    fn owner_typed_text_is_escaped_wherever_it_lands() {
        let page = page(&snapshot(
            vec![fixture::row("Roof <b> & \"gutters\"", 500, 1_000)],
            1_000,
        ));
        assert!(
            page.contains("Roof &lt;b&gt; &amp; &quot;gutters&quot;"),
            "the name was not escaped"
        );
        assert!(!page.contains("<b>"), "the name reached the page raw");
    }

    #[test]
    fn an_account_is_drawn_in_its_own_color() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        let teal = palette::hex(palette::account(crate::db::account::AccountColor::Teal));
        assert!(page.contains(&teal), "the account lost its color");
    }

    /// Each tab draws its own screen and no other's: a panel built from the
    /// wrong field would show the right table under the wrong label.
    #[test]
    fn each_panel_holds_its_own_screen() {
        let page = page(&snapshot(
            vec![fixture::row("Rainy Day", 500, 1_000)],
            1_000,
        ));
        assert!(panel(&page, "cash").contains("Groceries"), "no cash rows");
        assert!(
            panel(&page, "savings").contains("Unallocated"),
            "no savings rows"
        );
        assert!(
            panel(&page, "planning").contains("Remaining Excess"),
            "no planning rows"
        );
        assert!(panel(&page, "funds").contains("Stocks"), "no fund rows");
    }
}
