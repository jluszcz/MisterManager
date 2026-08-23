//! The Funds tab: the asset allocation, target against actual.

use super::{escape, whole_money};
use crate::fund::Allocation;
use crate::palette;
use crate::rate::BasisPoints;

/// A percentage cell. `—` for an age row with no birth date on record, which
/// is a question rather than a zero.
fn percent(bp: Option<BasisPoints>, low: bool) -> String {
    let text = bp.map_or_else(|| "—".to_string(), |bp| bp.to_string());
    // The furthest-down fund carries the warning color on its delta, the one
    // figure on this screen that says "put the next dollar here".
    let color = match low {
        true => format!(" style=\"color:{}\"", palette::hex(palette::NEGATIVE)),
        false => String::new(),
    };
    format!("<td class=\"n\"{color}>{text}</td>")
}

const HEADER: &str = "<thead><tr><th>Fund</th><th class=\"n\">Target %</th>\
                      <th class=\"n\">Actual %</th><th class=\"n\">Delta</th>\
                      <th class=\"n\">Actual Value</th></tr></thead>";

pub(super) fn table(funds: &Allocation) -> String {
    let mut rows = String::new();
    for (i, r) in funds.rows.iter().enumerate() {
        rows.push_str(&format!(
            "<tr><td>{}</td>{}{}{}{}</tr>",
            escape(&r.name),
            percent(r.target, false),
            percent(Some(r.actual_share), false),
            percent(r.delta, funds.furthest_down == Some(i)),
            whole_money(r.actual),
        ));
    }
    // Actual and Delta are left empty on purpose: every actual share sums to
    // 100%, and a total delta is not a number that means anything.
    rows.push_str(&format!(
        "<tr class=\"tot\"><td>Total</td>{}<td class=\"n\"></td><td class=\"n\"></td>{}</tr>",
        percent(Some(funds.target_total), false),
        whole_money(funds.total),
    ));
    format!("<table>{HEADER}{rows}</table>")
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{funds, panel, row, snapshot};
    use super::super::page;
    use crate::rate::BasisPoints;

    fn drawn(snapshot: &crate::report::Snapshot) -> String {
        panel(&page(snapshot), "funds").to_string()
    }

    #[test]
    fn every_fund_is_drawn_with_its_target_actual_and_delta() {
        let funds = drawn(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(
            funds.contains("Stocks") && funds.contains("Bonds"),
            "a fund vanished"
        );
        assert!(funds.contains("70.00"), "no target percentage");
        assert!(funds.contains("60.00"), "no actual percentage");
        assert!(funds.contains("10.00"), "no delta");
    }

    /// The one figure on this screen that says "put the next dollar here".
    #[test]
    fn the_furthest_down_fund_carries_the_warning_color_on_its_delta() {
        let funds = drawn(&snapshot(vec![], 1_000));
        let negative = crate::palette::hex(crate::palette::NEGATIVE);
        assert!(
            funds.contains(&negative),
            "the furthest-down delta is untinted"
        );
    }

    /// An age row with no birth date on record claims nothing, and `—` is
    /// that question rather than a zero that reads as an answer.
    #[test]
    fn a_target_with_no_birth_date_behind_it_draws_as_a_dash() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.funds.rows[1].target = None;
        snapshot.funds.rows[1].delta = None;
        assert!(
            drawn(&snapshot).contains("—"),
            "no dash for an absent target"
        );
    }

    /// Actual and Delta are left empty on purpose: every actual share sums to
    /// 100%, and a total delta is not a number that means anything.
    #[test]
    fn the_total_row_carries_the_target_total_and_the_value() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.funds = funds();
        let funds = drawn(&snapshot);
        assert!(
            funds.contains("<tr class=\"tot\"><td>Total</td>"),
            "no total row"
        );
        assert!(
            funds.contains(&BasisPoints(10_000).to_string()),
            "the target total vanished"
        );
    }
}
