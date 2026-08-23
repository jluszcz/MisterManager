//! The Savings tab: every container's goals, and what it has left over.

use super::{account, escape, optional_money, whole_money};
use crate::palette;
use crate::rate::Percent;
use crate::report::Container;
use crate::savings::{Row, unallocated};

/// The row's `<tr>` classes: `fav` for the owner's highlight, `expired` for a
/// goal past its date and still short. Either, both, or neither.
fn row_classes(row: &Row) -> String {
    let mut classes = Vec::new();
    if row.favorite {
        classes.push("fav");
    }
    if row.expired {
        classes.push("expired");
    }
    if classes.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", classes.join(" "))
    }
}

/// How funded a goal is, on [`palette::percent`]'s ramp: red at nothing,
/// yellow at halfway, green at funded. The Savings screen's `%` column reads
/// the same three stops, so a goal is the same shade in either sink.
///
/// A goal with no positive target has no percentage to place on the ramp, so
/// its `--` stays plain rather than being colored as if it were at zero -- the
/// same split the screen makes over its em dash.
fn percent(percent: Option<Percent>) -> String {
    match percent {
        Some(p) => format!(
            "<td class=\"n\" style=\"color:{}\">{}%</td>",
            palette::hex(palette::percent(p)),
            p.0
        ),
        None => "<td class=\"n\">--</td>".to_string(),
    }
}

/// The Savings table's own header, in `src/tui/savings.rs`'s column order and
/// wording minus the leading `Account` column -- the report groups a
/// container's rows under its own heading instead of repeating the container
/// on every row, and `src/report/` cannot name `tui` to read the labels off
/// it directly, so the two sinks are kept in agreement by eye instead.
const HEADER: [&str; 6] = ["Goal", "Current", "Goal", "%", "Goal Date", "$/Pay"];

/// One container: its goals, and the Unallocated remainder below them.
///
/// The widest table on the page -- six columns, and a phone -- so the goal
/// name is the column that gives way (`w`) while the date and the three
/// figures hold their line. `Unallocated` is ours rather than the owner's and
/// takes no `w`: it is a word the column is sized to fit, not one it may break
/// to make room for something else.
fn section(container: &Container) -> String {
    let header: String = HEADER
        .iter()
        .enumerate()
        .map(|(i, h)| {
            // The name and the goal date read as prose; the rest are money or
            // a percentage, right-aligned the same way their column's cells
            // are.
            let class = if i == 0 || i == 4 { "" } else { " class=\"n\"" };
            format!("<th{class}>{h}</th>")
        })
        .collect();
    let mut rows = format!("<thead><tr>{header}</tr></thead>");
    for r in &container.rows {
        let goal_date = r
            .goal_date
            .map(|d| escape(&d.to_string()))
            .unwrap_or_default();
        rows.push_str(&format!(
            "<tr{}><td class=\"w\">{}</td>{}{}{}<td class=\"d\">{goal_date}</td>{}</tr>",
            row_classes(r),
            escape(&r.name),
            whole_money(r.current),
            whole_money(r.goal),
            percent(r.percent),
            optional_money(r.per_paycheck),
        ));
    }
    // Named rather than left to `whole_money`, which would do the same
    // arithmetic: this is the rule the screen's own footer reads, and one
    // function is what stops the two sinks quoting different remainders.
    let excess = unallocated(container.excess);
    // Six cells, matching every goal row above: the excess is a money figure
    // and Current is where a reader looks for "how much is sitting here".
    rows.push_str(&format!(
        "<tr><td>Unallocated</td>{}<td class=\"n\"></td><td class=\"n\"></td><td class=\"d\"></td><td class=\"n\"></td></tr>",
        whole_money(excess)
    ));
    format!(
        "<h3>{}</h3><table>{rows}</table>",
        account(&container.account)
    )
}

pub(super) fn sections(containers: &[Container]) -> String {
    containers.iter().map(section).collect()
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{panel, row, snapshot};
    use super::super::page;
    use crate::palette;
    use crate::rate::Percent;

    /// One row at a given percentage, over the fixture's own figures: what is
    /// under test is the color the column carries, not the division that
    /// produced it.
    fn at(percent: Option<Percent>) -> crate::savings::Row {
        let mut row = row("Rainy Day", 500, 1_000);
        row.percent = percent;
        row
    }

    /// The screen's ramp and the page's are the same three stops, spelled
    /// `#rrggbb` here and handed to ratatui there -- a goal drawn green in
    /// the terminal and amber on the phone would be two answers to one
    /// question.
    #[test]
    fn the_percent_column_is_colored_by_the_funding_ramp() {
        let stops = [Percent::ZERO, Percent(50), Percent::ONE_HUNDRED];
        let page = page(&snapshot(stops.map(|p| at(Some(p))).to_vec(), 1_000));
        let panel = panel(&page, "savings");
        for p in stops {
            let cell = format!(
                "<td class=\"n\" style=\"color:{}\">{}%</td>",
                palette::hex(palette::percent(p)),
                p.0
            );
            assert!(panel.contains(&cell), "{p:?} is not on the ramp: {panel}");
        }
    }

    /// A goal with no positive target has no percentage to place on the ramp,
    /// so its `--` stays plain: an unfunded red there would report a goal with
    /// no target as a goal with no money.
    #[test]
    fn a_goal_with_no_percentage_is_left_uncolored() {
        let page = page(&snapshot(vec![at(None)], 1_000));
        let panel = panel(&page, "savings");
        assert!(
            panel.contains("<td class=\"n\">--</td>"),
            "the plain `--` vanished: {panel}"
        );
        for p in [Percent::ZERO, Percent(50), Percent::ONE_HUNDRED] {
            let hex = palette::hex(palette::percent(p));
            assert!(
                !panel.contains(&hex),
                "{hex} colored a row with no percentage: {panel}"
            );
        }
    }

    /// The screens' own precision, the Unallocated line included: the $0.23 a
    /// container sits at for months is noise beside figures in the
    /// thousands, and truncating it is what leaves the line saying nothing
    /// when there is nothing to say.
    #[test]
    fn every_figure_drops_its_cents_the_unallocated_line_included() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(page.contains(">500<"), "a savings figure grew cents");
        assert!(!page.contains("0.23"), "the remainder kept its cents");
    }

    /// The container the `Unallocated` row belongs to is named in its own
    /// color, in the heading above the table -- the screen's footer names the
    /// same container the same way, on the same line as the figure because a
    /// terminal has no heading to hang it off.
    #[test]
    fn the_container_heading_names_its_account_in_color() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        let teal = palette::hex(palette::account(crate::db::account::AccountColor::Teal));
        assert!(
            panel.contains(&format!(
                "<h3><span style=\"color:{teal}\">Rainy Day</span></h3>"
            )),
            "{panel}"
        );
    }

    /// The marker the row used to carry is gone: it said "this container is
    /// within a dollar", which is now what the truncated figure itself says.
    #[test]
    fn the_unallocated_row_carries_no_marker() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        for marker in ["✓", "!"] {
            assert!(!panel.contains(marker), "{marker} survived: {panel}");
        }
    }

    /// A container allocated past what it holds reads red, the color every
    /// other negative figure on the page takes -- and a sub-dollar negative
    /// does not, because the truncation takes its sign along with its cents.
    #[test]
    fn a_negative_remainder_is_red_and_a_sub_dollar_one_is_not() {
        let red = palette::hex(palette::NEGATIVE);
        let mut over = snapshot(vec![], 1_000);
        over.containers[0].excess = crate::money::Cents(-250_000);
        let over = page(&over);
        let over = panel(&over, "savings");
        assert!(over.contains(&red), "an overdrawn container was not red");
        assert!(over.contains("-2,500"), "{over}");

        let mut drift = snapshot(vec![], 1_000);
        drift.containers[0].excess = crate::money::Cents(-23);
        let drift = page(&drift);
        let drift = panel(&drift, "savings");
        assert!(!drift.contains(&red), "sub-dollar drift drew red: {drift}");
        assert!(!drift.contains("-0"), "{drift}");
    }

    /// A goal's own balance drops its cents before its color is chosen, the
    /// same as the `Unallocated` row beneath it. The cents a container
    /// collects land on its goals too -- an interest share, an imported
    /// correction -- and a figure the page shows as `0` must not be marked as
    /// a debt.
    #[test]
    fn a_sub_dollar_negative_balance_is_neither_signed_nor_red() {
        let red = palette::hex(palette::NEGATIVE);
        let mut snapshot = snapshot(vec![row("Rainy Day", 0, 1_000)], 0);
        snapshot.containers[0].rows[0].current = crate::money::Cents(-23);
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        assert!(!panel.contains(&red), "sub-dollar drift drew red: {panel}");
        // `>-0<` rather than `-0`, which every goal date carries.
        assert!(!panel.contains(">-0<"), "{panel}");
        assert!(panel.contains("<td class=\"n\">0</td>"), "{panel}");
    }

    /// Unlike an empty Overview band, which is a group the owner does not
    /// have, a container holding an unallocated remainder is a fact worth
    /// seeing.
    #[test]
    fn a_container_with_no_open_goals_still_reports_its_unallocated_remainder() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        assert!(panel.contains("Rainy Day"), "the container vanished");
        assert!(panel.contains("Unallocated"), "the remainder vanished");
    }

    /// A database with no containers holding goals is not a database with
    /// nothing to import -- the tab must still be there, over nothing.
    #[test]
    fn a_snapshot_with_no_containers_still_renders() {
        let mut without_containers = snapshot(vec![], 1_000);
        without_containers.containers.clear();
        let page = page(&without_containers);
        assert!(
            page.contains("<label for=\"savings\">Savings</label>"),
            "the Savings tab vanished"
        );
        assert!(
            panel(&page, "savings").is_empty(),
            "a container survived being cleared"
        );
    }
}
