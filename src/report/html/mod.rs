//! The report as one page: `finance-utils`' shell, one JSON payload, and the
//! script that draws every tab from it.
//!
//! Self-contained by requirement: inline CSS and script, no font, no image
//! beyond the favicon inlined as a `data:` URI, no request of any kind. A
//! phone opening this out of a sync folder may be offline, and anything
//! fetched would render half-drawn or not at all.
//!
//! One module per tab, the way `src/tui/` keeps one module per screen. Each
//! builds its tab as a [`TabView`](crate::report::view::TabView) with every
//! decision already made -- a figure's sign, an account's color, a bar's
//! widths -- and `report.js` only lays them out. What the tabs share is
//! `crate::report::view`.

mod funds;
mod ledger;
mod overview;
mod planning;
mod retirement;
mod savings;

#[cfg(test)]
mod fixture;

use super::Snapshot;
use super::view::{Accounts, Page, tokens_css};
use jluszcz_finance_utils::report::html::{App, NavTab, app_page, script_json};

// Owner text reaches the page two ways, and neither is markup: the payload,
// where `script_json` turns every `<` into `<` so no string can end the
// block, and the script, which inserts every string through `textContent`.
// The one exception is a chart's SVG, built here, whose only text is a
// compact figure and a month label -- `retirement` still escapes them, so the
// rule has no case to argue.
use jluszcz_finance_utils::report::escape;

/// One tab of the bar: its id, which names its panel; the label the wide bar
/// shows and the shorter one under its icon; and the icon, the inside of a
/// 24x24 `<svg>` the shell strokes in the text color.
pub(super) struct Tab {
    pub id: &'static str,
    pub label: &'static str,
    pub short: &'static str,
    pub icon: &'static str,
}

/// The tabs, in the order the screens are numbered: `1` Overview, `2` Cash,
/// `3` Credit, `4` Savings, `5` Planning, `7` Funds, `8` Retirement. An owner
/// who reaches for `4` on the keyboard should not find Planning under it here.
///
/// [`page_view`] builds its tabs in this order, and a test holds the two
/// level: the script fills `#{id}-panel` by the id each view carries.
pub(super) const TABS: [Tab; 7] = [
    Tab {
        id: "overview",
        label: "Overview",
        short: "Overview",
        icon: "<path d=\"M4 19V9M10 19V5M16 19v-7M22 19H2\"/>",
    },
    Tab {
        id: "cash",
        label: "Cash",
        short: "Cash",
        icon: "<rect x=\"2\" y=\"6\" width=\"20\" height=\"12\" rx=\"2\"/>\
               <circle cx=\"12\" cy=\"12\" r=\"2.5\"/>",
    },
    Tab {
        id: "credit",
        label: "Credit",
        short: "Credit",
        icon: "<rect x=\"2\" y=\"5\" width=\"20\" height=\"14\" rx=\"2\"/>\
               <path d=\"M2 10h20M6 15h4\"/>",
    },
    Tab {
        id: "savings",
        label: "Savings",
        short: "Savings",
        icon: "<circle cx=\"12\" cy=\"12\" r=\"9\"/><circle cx=\"12\" cy=\"12\" r=\"5\"/>\
               <circle cx=\"12\" cy=\"12\" r=\"1\"/>",
    },
    Tab {
        id: "planning",
        label: "Planning",
        short: "Plan",
        icon: "<path d=\"M4 5h16M7 10h13M10 15h10M13 20h7\"/>",
    },
    Tab {
        id: "funds",
        label: "Funds",
        short: "Funds",
        icon: "<path d=\"M12 3v9l7 4\"/><circle cx=\"12\" cy=\"12\" r=\"9\"/>",
    },
    Tab {
        id: "retirement",
        label: "Retirement",
        short: "Retire",
        icon: "<path d=\"M3 20h18M6 20V10l6-5 6 5v10M10 20v-5h4v5\"/>",
    },
];

/// The page's own layout, after the palette tokens [`tokens_css`] generates.
const STYLE: &str = include_str!("../report.css");

/// What draws the payload into the panels.
const SCRIPT: &str = include_str!("../report.js");

/// How old the page is, in the footer: a reader arrives at the figures, and
/// checks the stamp on the way out. `%-I:%M %p` rather than a 24-hour clock,
/// because this is read on a phone alongside every other page on it.
const STAMP_FORMAT: &str = "%Y-%m-%d %-I:%M %p";

/// Every tab as data, in `TABS` order, over one account table and one list
/// of histories.
pub(super) fn page_view(snapshot: &Snapshot) -> Page {
    let mut acc = Accounts::new();
    let mut histories = Vec::new();
    let tabs = vec![
        overview::view(&snapshot.overview, &mut acc),
        ledger::view(&snapshot.cash, &mut acc),
        ledger::view(&snapshot.credit, &mut acc),
        savings::view(&snapshot.containers, &mut acc, &mut histories),
        planning::view(&snapshot.planning, &mut acc),
        funds::view(&snapshot.allocation, &mut acc),
        retirement::view(&snapshot.retirement, snapshot.charts.as_ref(), &mut acc),
    ];
    Page {
        accounts: acc.into_vec(),
        tabs,
        histories,
    }
}

pub fn page(snapshot: &Snapshot) -> String {
    let json = serde_json::to_string(&page_view(snapshot))
        .expect("the view holds only strings, numbers and lists");
    let tabs: Vec<NavTab> = TABS
        .iter()
        .map(|t| NavTab::new(t.id, t.label).short(t.short).icon(t.icon))
        .collect();
    let panels = vec![String::new(); TABS.len()];
    app_page(&App {
        title: "Money",
        favicon: Some("\u{1F4B0}"),
        style: &format!("{}{STYLE}", tokens_css()),
        tabs: &tabs,
        open: 0,
        panels: &panels,
        data: Some(&script_json(&json)),
        script: Some(SCRIPT),
        stamp: &format!("Generated {}", snapshot.generated_at.format(STAMP_FORMAT)),
    })
}

/// The JSON block a page carries, parsed: `<` arrives as `<`, which
/// JSON reads back as the character it stands for. Tolerates the
/// minifier's unquoted attributes.
#[cfg(test)]
pub(super) fn payload(page: &str) -> serde_json::Value {
    let open = page.find("application/json").expect("no data block");
    let start = open + page[open..].find('>').unwrap() + 1;
    let end = start
        + page[start..]
            .find("</script>")
            .expect("an unclosed data block");
    serde_json::from_str(&page[start..end]).expect("the data block is not JSON")
}

#[cfg(test)]
mod tests {
    use super::fixture::{row, snapshot};
    use super::*;

    fn fixture() -> Snapshot {
        snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000)
    }

    /// A phone opening this out of a sync folder may be offline, and a page
    /// that fetched anything would render half-drawn or not at all.
    #[test]
    fn the_page_makes_no_external_request() {
        let page = page(&fixture::long_names_snapshot());
        assert!(!page.contains("http"), "the page reaches out of itself");
    }

    /// The script fills each panel by the id its view carries, so a view
    /// built out of `TABS` order would draw under the wrong tab.
    #[test]
    fn every_tab_has_a_panel_in_order() {
        let snapshot = fixture();
        let page = page(&snapshot);
        let mut last = 0;
        for tab in TABS {
            let at = page
                .find(&format!("id=\"{}-panel\"", tab.id))
                .unwrap_or_else(|| panic!("no {} panel", tab.id));
            assert!(at > last, "the {} panel is out of order", tab.id);
            assert!(
                page.contains(&format!("data-t=\"{}\"", tab.id)),
                "no {} tab",
                tab.id
            );
            last = at;
        }
        let ids: Vec<_> = page_view(&snapshot).tabs.iter().map(|t| t.id).collect();
        assert_eq!(ids, TABS.map(|t| t.id));
    }

    #[test]
    fn the_payload_parses_back_to_the_view() {
        let snapshot = fixture::long_names_snapshot();
        assert_eq!(
            payload(&page(&snapshot)),
            serde_json::to_value(page_view(&snapshot)).unwrap()
        );
    }

    /// Owner text that spells the element's own closer has to arrive as
    /// text: the first `</script>` after the data block opens is the block's.
    #[test]
    fn owner_text_cannot_end_the_payload() {
        let name = "a</script><b>&\"";
        let page = page(&snapshot(vec![row(name, 500, 1_000)], 1_000));
        let open = page.find("application/json").unwrap();
        let block = &page[open..];
        let close = block.find("</script>").unwrap();
        assert!(!block[..close].contains('<'), "{}", &block[..close]);
        assert_eq!(payload(&page)["histories"][0]["name"], name);
    }

    /// The page's job on a phone is to be honest about its own age, and the
    /// footer is where a reader looks for that.
    #[test]
    fn the_page_names_when_it_was_generated() {
        assert!(
            page(&fixture()).contains("<p class=\"stamp\">Generated 2026-08-21 2:30 PM</p>"),
            "no generated-at stamp in the footer"
        );
    }

    /// Every tab carries an icon, so the shell draws the bottom bar on a
    /// phone; one missing would drop the whole bar to the wrapping row.
    #[test]
    fn the_bar_is_the_icon_form() {
        assert!(page(&fixture()).contains("class=\"tabs icons\""));
    }

    /// The chart lines and the legend's Total swatch draw in `currentColor`,
    /// so the stylesheet is the only thing that gives them ink.
    #[test]
    fn the_stylesheet_inks_the_chart_lines_and_the_total() {
        let page = page(&fixture());
        for rule in [
            ".ln{stroke:currentColor}",
            "circle.ln{fill:currentColor}",
            ".total{color:var(--total)}",
            ".key{",
        ] {
            assert!(STYLE.contains(rule), "no {rule}");
        }
        assert!(
            page.contains(STYLE.trim()),
            "the stylesheet is not on the page"
        );
        assert!(page.contains("--total:"), "no Total token");
    }
}
