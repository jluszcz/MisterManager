//! The Planning tab: what the payday would move, over the waterfall that
//! decided it.

use super::{account, escape, full_width_row, money};
use crate::gate::Gate;
use crate::money::Cents;
use crate::plan_line::Line;
use crate::rate::Percent;
use crate::report::{PlanView, Planning, Transfer};

const COLUMNS: usize = 3;

/// Label, figure, and the extra column the screen carries: the percentage
/// that produced a split, the biweekly figure beside a monthly bill.
fn row(class: &str, label: &str, figure: String, extra: String) -> String {
    let class = match class.is_empty() {
        true => String::new(),
        false => format!(" class=\"{class}\""),
    };
    format!(
        "<tr{class}><td>{}</td>{figure}<td class=\"n\">{extra}</td></tr>",
        escape(label)
    )
}

/// A figure the waterfall computed. Whole dollars, the precision the Planning
/// screen shows -- cents on a plan is a false claim about where a percentage
/// landed.
fn figure(label: &str, cents: Cents) -> String {
    row(
        "",
        label,
        money(cents.to_whole_dollars(), cents),
        String::new(),
    )
}

/// A figure the rows above it add up to.
fn total(label: &str, cents: Cents) -> String {
    row(
        "tot",
        label,
        money(cents.to_whole_dollars(), cents),
        String::new(),
    )
}

/// A figure with the percentage that produced it beside it.
fn split(label: &str, cents: Cents, pct: Percent) -> String {
    row(
        "",
        label,
        money(cents.to_whole_dollars(), cents),
        format!("{}%", pct.0),
    )
}

/// A row whose value is not money: a pay-period count, or a gate's answer.
fn stated(label: &str, value: &str) -> String {
    row(
        "",
        label,
        format!("<td class=\"n\">{}</td>", escape(value)),
        String::new(),
    )
}

fn heading(label: &str) -> String {
    full_width_row("head", COLUMNS, escape(label))
}

/// One transfer, and the lines that make it up.
///
/// The account is named once, at the head of the group it heads -- tinted
/// there, exactly as the Planning screen tints the label column rather than
/// the figures. A withdrawal names no account and takes no color: money
/// leaving the tracked system is a destination, not a failure.
fn transfer(t: &Transfer) -> String {
    let label = match &t.to {
        Some(a) => account(a),
        None => escape(&t.label),
    };
    let head = format!(
        "<tr class=\"tot\"><td>{label}</td>{}<td class=\"n\"></td></tr>",
        money(t.cents.to_whole_dollars(), t.cents)
    );
    let lines: String = t
        .lines
        .iter()
        .map(|(line, cents)| {
            row(
                "sub",
                line,
                money(cents.to_whole_dollars(), *cents),
                String::new(),
            )
        })
        .collect();
    format!("{head}{lines}")
}

fn resolved(view: &PlanView) -> String {
    let p = &view.plan;
    let s = &view.settings;
    let mut rows = heading("Transfers");
    match &view.transfers {
        // The screen renders the reason in place of the block; so does this.
        // Every figure below is still right -- a dangling destination key
        // stops the money moving without making the waterfall wrong.
        Err(message) => rows.push_str(&full_width_row("note", COLUMNS, escape(message))),
        Ok(transfers) => rows.extend(transfers.iter().map(transfer)),
    }

    rows.push_str(&heading("Excess"));
    rows.push_str(&figure("Target", s.target));
    rows.push_str(&figure("Buffer", s.buffer));
    rows.push_str(&stated(
        "Pay Periods / Year",
        &s.periods_per_year.to_string(),
    ));
    rows.push_str(&figure("Excess (Actual)", p.excess_actual));
    rows.push_str(&total("Excess (Used)", p.excess_used));

    rows.push_str(&heading("Bills"));
    // `Planning!C6` -- the housing subtotal, and the only bill line that is
    // not a bill. Its biweekly figure is the one the waterfall spends.
    rows.push_str(&row(
        "",
        "Mortgage + HOA",
        money(
            view.housing_monthly.to_whole_dollars(),
            view.housing_monthly,
        ),
        p.housing_biweekly.to_whole_dollars(),
    ));
    for b in view.housing.iter().chain(&view.other_bills) {
        rows.push_str(&row(
            "sub",
            &b.label,
            money(b.monthly.to_whole_dollars(), b.monthly),
            b.biweekly.to_whole_dollars(),
        ));
    }
    rows.push_str(&total("Remaining Excess", p.remaining_excess));

    rows.push_str(&heading("Gates"));
    for (gate, needed) in [
        (Gate::EmergencyFund, p.need_emergency),
        (Gate::Roth, p.need_roth),
    ] {
        rows.push_str(&stated(gate.label(), if needed { "needed" } else { "met" }));
    }

    rows.push_str(&heading("Waterfall"));
    rows.push_str(&split("Bill Payments", p.bill_payments, s.bill_payment_pct));
    rows.push_str(&row(
        "sub",
        "Cap",
        money(s.bill_payment_cap.to_whole_dollars(), s.bill_payment_cap),
        String::new(),
    ));
    rows.push_str(&figure("Mom & Dad", p.mom_and_dad));
    rows.push_str(&row(
        "sub",
        "Annual",
        money(
            s.mom_and_dad_annual.to_whole_dollars(),
            s.mom_and_dad_annual,
        ),
        String::new(),
    ));
    rows.push_str(&total("Remainder", p.remainder));
    rows.push_str(&row(
        "sub",
        "Goals Floor",
        money(s.goals_floor.to_whole_dollars(), s.goals_floor),
        String::new(),
    ));

    rows.push_str(&heading("Split"));
    rows.push_str(&split(
        Line::FutureHousing.label(),
        p.future_housing,
        s.future_housing_pct,
    ));
    rows.push_str(&split(
        Line::Retirement.label(),
        p.retirement,
        s.retirement_pct,
    ));
    rows.push_str(&split(
        Line::Investment.label(),
        p.investment,
        s.investment_pct,
    ));
    rows.push_str(&split(Line::Goals.label(), p.goals, s.goals_pct()));

    rows.push_str(&total("Checksum", p.checksum));
    format!("<table>{rows}</table>")
}

pub(super) fn block(planning: &Planning) -> String {
    match planning {
        // A plan that cannot resolve is an ordinary state, and the reason is
        // the whole content of the tab -- the same way the screen renders the
        // message in place of the waterfall.
        Planning::Unresolvable(message) => {
            format!(
                "<table>{}</table>",
                full_width_row("note", COLUMNS, escape(message))
            )
        }
        Planning::Resolved(view) => resolved(view),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{panel, plan_view, row, snapshot};
    use super::super::page;
    use crate::report::Planning;

    fn planning(snapshot: &crate::report::Snapshot) -> String {
        panel(&page(snapshot), "planning").to_string()
    }

    /// The waterfall in the order the screen walks it: what moves, then the
    /// working behind it, then the check that the working adds up.
    #[test]
    fn the_waterfall_is_drawn_in_the_screens_own_order() {
        let planning = planning(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let mut at = 0;
        for heading in [
            "Transfers",
            "Excess",
            "Bills",
            "Gates",
            "Waterfall",
            "Split",
        ] {
            let found = planning[at..]
                .find(heading)
                .unwrap_or_else(|| panic!("no {heading} block, or it came out of order"));
            at += found;
        }
        assert!(planning.contains("Checksum"), "no checksum");
    }

    /// A withdrawal is a destination and not a failure, so it draws as a
    /// transfer with no account rather than as a missing row.
    #[test]
    fn a_withdrawal_draws_beside_the_transfers() {
        let planning = planning(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(planning.contains("Withdrawal"), "the withdrawal vanished");
        assert!(planning.contains("Retirement"), "its line vanished");
    }

    /// A plan that cannot resolve is an ordinary state -- a database with no
    /// account in the `Checking` band has one -- and the reason is the whole
    /// content of the tab, exactly as it is the whole content of the screen.
    #[test]
    fn an_unresolvable_plan_draws_its_reason_in_place_of_the_figures() {
        let mut snapshot = snapshot(vec![], 1_000);
        snapshot.planning = Planning::Unresolvable("no account in the Checking band".into());
        let planning = planning(&snapshot);
        assert!(
            planning.contains("no account in the Checking band"),
            "the reason is not on the page"
        );
        assert!(
            !planning.contains("Checksum"),
            "a figure survived the error"
        );
    }

    /// Transfers that cannot be resolved do not make the waterfall above them
    /// wrong: a dangling destination key stops the money moving and nothing
    /// else, so the figures stay and the block says why it is empty.
    #[test]
    fn unresolvable_transfers_leave_the_waterfall_standing() {
        let mut snapshot = snapshot(vec![], 1_000);
        let mut view = plan_view();
        view.transfers = Err("setting planning.goals = 41 is gone".into());
        snapshot.planning = Planning::Resolved(Box::new(view));
        let planning = planning(&snapshot);
        assert!(
            planning.contains("setting planning.goals = 41 is gone"),
            "the reason is not on the page"
        );
        assert!(planning.contains("Checksum"), "the waterfall went with it");
    }
}
