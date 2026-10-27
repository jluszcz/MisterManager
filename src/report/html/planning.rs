//! The Planning tab: what the payday would move, over the waterfall that
//! decided it.
//!
//! The rows themselves come from [`crate::plan_rows`], which the Planning
//! screen reads too. What is here is only this medium's spelling of them: a
//! row per row, a class per [`plan_rows::Kind`], and the depth the screen
//! spends in spaces handed on as a number.

use crate::plan_rows::{self, Extra, Kind, RowLabel, Value};
use crate::report::view::{Accounts, Block, Cell, Row, TabView, whole};
use crate::report::{PlanView, Planning};

/// The rows this tab is, before they are anything this medium draws.
fn waterfall(view: &PlanView) -> Vec<plan_rows::Row> {
    plan_rows::rows(&plan_rows::Input {
        plan: &view.plan,
        settings: &view.settings,
        housing: &view.housing,
        other_bills: &view.other_bills,
        expense_constants: &view.expense_constants,
        transfers: match &view.transfers {
            Ok(transfers) => Ok(transfers.as_slice()),
            Err(message) => Err(message.as_str()),
        },
        spread_ask_total: view.spread_ask_total,
        // The report quotes the canonical dates and never `App::adhoc`, so
        // there is no hypothetical balance for a row to name.
        scrubbed_adhoc: None,
    })
}

/// A row's label as a cell: text, or the account in its own color.
fn label_cell(label: &RowLabel, acc: &mut Accounts) -> Cell {
    match label {
        RowLabel::Text(text) => Cell::Text(text.clone()),
        RowLabel::Account(a) => acc.cell(a),
    }
}

/// One waterfall row, or `None` for a blank: the screen's separator is an
/// empty line, and this medium's is the `gap` class on the row *after* it --
/// read from the list rather than from the row, because a row does not know
/// what sits above it.
///
/// Classes rather than one class, because what a row *is* -- a total, a
/// heading -- and whether the Biweekly Expenses figure counts it are separate
/// facts. The depth goes on as a number for the script to spend as an
/// indent, a level at a time: a page that collapsed every level past the
/// first onto one indent would draw flat a row the screen draws nested.
/// Depth `1` takes none, the heading above a block's own lines having already
/// set them apart. A gap is drawn in `neg`, the red every gap is.
fn view_row(row: &plan_rows::Row, after_blank: bool, acc: &mut Accounts) -> Option<Row> {
    let label = label_cell(&row.label, acc);
    let class = match row.kind {
        Kind::Blank => return None,
        Kind::Heading => Some("head"),
        Kind::Note => Some("note"),
        Kind::Figure | Kind::Total => None,
    };
    if let Some(class) = class {
        return Some(Row {
            c: vec![label],
            k: Some(class.to_string()),
            h: None,
            d: None,
            m: None,
        });
    }
    let mut classes: Vec<&str> = Vec::new();
    if row.kind == Kind::Total {
        classes.push("tot");
    }
    if row.counts_as_expense {
        classes.push("counted");
    }
    if after_blank {
        classes.push("gap");
    }
    let figure = match row.value {
        Value::Money(cents) => whole(cents),
        Value::Count(count) => Cell::Text(count.to_string()),
        Value::Stated(text) => Cell::Text(text.to_string()),
        Value::None => Cell::Text(String::new()),
    };
    let extra = match row.extra {
        Extra::Percent(pct) => Cell::Text(format!("{}%", pct.exact_percent())),
        Extra::Biweekly(cents) => Cell::Text(cents.to_whole_dollars()),
        Extra::Gap(cents) => Cell::Toned(
            format!("\u{394} {}", cents.to_whole_dollars()),
            "neg".to_string(),
        ),
        Extra::Date(date) => Cell::Text(format!("{date}*")),
        Extra::Annual(cents) => Cell::Text(format!("{}/yr", cents.to_whole_dollars())),
        Extra::None => Cell::Text(String::new()),
    };
    Some(Row {
        c: vec![label, figure, extra],
        k: (!classes.is_empty()).then(|| classes.join(" ")),
        h: None,
        d: (row.depth >= 2).then_some(row.depth),
        m: None,
    })
}

/// The Planning tab: one [`Row`] per waterfall row, or the reason a plan
/// cannot resolve in its place -- an ordinary state, and then the whole
/// content of the tab, the same way the screen renders the message in place
/// of the waterfall.
pub(super) fn view(planning: &Planning, acc: &mut Accounts) -> TabView {
    let blocks = match planning {
        Planning::Unresolvable(message) => vec![Block::Note {
            text: message.clone(),
        }],
        Planning::Resolved(view) => {
            let all = waterfall(view);
            let rows = all
                .iter()
                .enumerate()
                .filter_map(|(i, row)| view_row(row, i > 0 && all[i - 1].kind == Kind::Blank, acc))
                .collect();
            vec![Block::Table {
                class: "pl",
                head: vec![],
                rows,
            }]
        }
    };
    TabView {
        id: "planning",
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, plan_view, row, snapshot};
    use super::{Kind, view, waterfall};
    use crate::report::Planning;
    use crate::report::view::Accounts;
    use serde_json::json;

    fn resolved(edit: impl FnOnce(&mut crate::report::PlanView)) -> Planning {
        let mut view = plan_view();
        edit(&mut view);
        Planning::Resolved(Box::new(view))
    }

    fn rows(planning: &Planning) -> Vec<serde_json::Value> {
        let v = json(&view(planning, &mut Accounts::new()));
        v["blocks"][0]["rows"].as_array().unwrap().clone()
    }

    #[test]
    fn an_unresolvable_plan_is_its_message() {
        let v = json(&view(
            &Planning::Unresolvable("no account in the Checking band".into()),
            &mut Accounts::new(),
        ));
        assert_eq!(v["id"], "planning");
        assert_eq!(
            v["blocks"],
            json!([{"t": "Note", "text": "no account in the Checking band"}])
        );
    }

    #[test]
    fn blank_rows_draw_nothing_and_the_row_after_takes_gap() {
        let planning =
            resolved(|v| v.expense_constants = vec![crate::plan_rows::Target::GoalsFloor]);
        let Planning::Resolved(plan) = &planning else {
            unreachable!()
        };
        let all = waterfall(plan);
        let blanks = all.iter().filter(|r| r.kind == Kind::Blank).count();
        let drawn = rows(&planning);
        assert!(blanks > 0);
        assert_eq!(drawn.len(), all.len() - blanks);
        let expenses = drawn
            .iter()
            .find(|r| r["c"][0] == "Expenses")
            .unwrap_or_else(|| panic!("no Expenses row: {drawn:?}"));
        assert!(
            expenses["k"].as_str().unwrap().contains("gap"),
            "{expenses}"
        );
    }

    #[test]
    fn depth_two_and_deeper_carry_their_depth() {
        let planning = resolved(|_| {});
        let Planning::Resolved(plan) = &planning else {
            unreachable!()
        };
        let deep = waterfall(plan)
            .iter()
            .filter(|r| r.depth >= 2 && !matches!(r.kind, Kind::Blank | Kind::Heading | Kind::Note))
            .count();
        let carried = rows(&planning)
            .iter()
            .filter(|r| r.get("d").is_some())
            .count();
        assert!(deep > 0, "the fixture has nothing indented");
        assert_eq!(carried, deep);
        assert!(
            rows(&planning)
                .iter()
                .all(|r| r["d"].as_u64().is_none_or(|d| d >= 2))
        );
    }

    #[test]
    fn a_shortfall_is_a_negative_delta_in_the_extra_cell() {
        let planning = resolved(|v| {
            let cents = crate::money::Cents::from_dollars(307);
            v.plan.shortfall.bills = crate::money::Cents::from_dollars(237);
            if let Ok(transfers) = &mut v.transfers {
                transfers[0]
                    .lines
                    .push((crate::plan_line::Line::Bills, cents));
            }
        });
        let drawn = rows(&planning);
        let gaps = drawn
            .iter()
            .filter(|r| r["c"][2] == json!(["\u{394} 237", "neg"]))
            .count();
        assert!(gaps >= 1, "no toned gap: {drawn:?}");
    }

    #[test]
    fn a_counted_row_is_marked() {
        let planning =
            resolved(|v| v.expense_constants = vec![crate::plan_rows::Target::GoalsFloor]);
        assert!(
            rows(&planning)
                .iter()
                .any(|r| r["k"].as_str().is_some_and(|k| k.contains("counted"))),
        );
        assert!(
            !rows(&resolved(|_| {}))
                .iter()
                .any(|r| r["k"].as_str().is_some_and(|k| k.contains("counted")))
        );
    }

    #[test]
    fn headings_and_notes_are_one_cell_rows_and_totals_are_marked() {
        let drawn = rows(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000).planning);
        assert!(
            drawn
                .iter()
                .any(|r| r["k"] == "head" && r["c"].as_array().unwrap().len() == 1)
        );
        assert!(
            drawn
                .iter()
                .any(|r| r["k"].as_str().is_some_and(|k| k.contains("tot")))
        );
        assert!(drawn.iter().any(|r| r["c"].as_array().unwrap().len() == 3));
    }
}
