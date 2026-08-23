//! The Planning tab: what the payday would move, over the waterfall that
//! decided it.

use super::{account, escape, full_width_row, whole_money};
use crate::calc::planning::Lines;
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
    row("", label, whole_money(cents), String::new())
}

/// A figure the rows above it add up to.
fn total(label: &str, cents: Cents) -> String {
    row("tot", label, whole_money(cents), String::new())
}

/// A figure with the percentage that produced it beside it.
fn split(label: &str, cents: Cents, pct: Percent) -> String {
    row("", label, whole_money(cents), format!("{}%", pct.0))
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
fn transfer(t: &Transfer, shortfall: &Lines) -> String {
    let label = match &t.to {
        Some(a) => account(a),
        None => escape(&t.label),
    };
    let head = format!(
        "<tr class=\"tot\"><td>{label}</td>{}<td class=\"n\"></td></tr>",
        whole_money(t.cents)
    );
    let lines: String = t
        .lines
        .iter()
        .map(|(line, cents)| {
            // What the excess cut from this line, in the column the page
            // keeps for it and the red the screen paints it -- and nothing
            // where a withdrawal carries it, because a withdrawal is one
            // line under a head repeating its own figure and the Planning
            // screen draws it plain. What it lost is in the `Shortfall` row
            // below, stated once for the whole plan rather than per line.
            let cut = match t.to.is_some() {
                true => line.amount(shortfall),
                false => Cents::ZERO,
            };
            row(
                "sub",
                line.label(),
                whole_money(*cents),
                match cut > Cents::ZERO {
                    true => gap(cut),
                    false => String::new(),
                },
            )
        })
        .collect();
    format!("{head}{lines}")
}

/// A gap, in the red every gap on this page is drawn in.
fn gap(cents: Cents) -> String {
    format!(
        "<span style=\"color:{}\">\u{394} {}</span>",
        crate::palette::hex(crate::palette::NEGATIVE),
        escape(&cents.to_whole_dollars())
    )
}

/// A gap the block foots with rather than hangs off a line.
///
/// The column the per-line gaps are drawn in, because these are what those
/// cells could not carry: each reports a payday with no transfer row to hang
/// a cell off at all.
fn footer(label: &str, cents: Cents) -> String {
    row(
        "tot",
        label,
        "<td class=\"n\"></td>".to_string(),
        gap(cents),
    )
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
        Ok(transfers) => rows.extend(transfers.iter().map(|t| transfer(t, &p.shortfall))),
    }
    // Both footers sit outside the match on purpose, exactly as they do on
    // the screen: each reports a payday with no transfer row to hang a
    // per-line cell off -- a plug of nothing for the first, an excess the
    // fixed bills took whole for the second.
    if let Some(unmet) = crate::transfer::unmet_asks(p.lines.goals, view.spread_ask_total) {
        rows.push_str(&footer("Unmet Asks", unmet));
    }
    if p.shortfall.total() > Cents::ZERO {
        rows.push_str(&footer("Shortfall", p.shortfall.total()));
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
        whole_money(view.housing_monthly),
        p.housing_biweekly.to_whole_dollars(),
    ));
    for b in view.housing.iter().chain(&view.other_bills) {
        rows.push_str(&row(
            "sub",
            &b.label,
            whole_money(b.monthly),
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
        whole_money(s.bill_payment_cap),
        String::new(),
    ));
    rows.push_str(&figure("Mom & Dad", p.mom_and_dad));
    rows.push_str(&row(
        "sub",
        "Annual",
        whole_money(s.mom_and_dad_annual),
        String::new(),
    ));
    rows.push_str(&total("Remainder", p.remainder));
    rows.push_str(&row(
        "sub",
        "Goals Floor",
        whole_money(s.goals_floor),
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
    }

    /// The fixture's snapshot with its plan reshaped. These tests are about
    /// what the page draws *over* a plan, and computing a second waterfall
    /// to reach one figure would pin the arithmetic twice over.
    fn with_plan(edit: impl FnOnce(&mut crate::report::PlanView)) -> String {
        let mut snapshot = snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000);
        let Planning::Resolved(view) = &mut snapshot.planning else {
            panic!("the fixture's plan does not resolve");
        };
        edit(view);
        planning(&snapshot)
    }

    /// The transfers never total more than the excess, so there is nothing
    /// for a heading or a foot to report -- and a figure saying nothing is
    /// wrong on every ordinary payday is a figure nobody reads.
    #[test]
    fn a_plan_that_covers_everything_draws_no_gap_anywhere() {
        let planning = with_plan(|_| {});

        assert!(
            !planning.contains("Checksum"),
            "the foot of the tab still carries a checksum"
        );
        assert!(
            !planning.contains('\u{394}'),
            "a covered plan drew a gap: {planning}"
        );
    }

    /// A payday too small for the fixed bills cuts one of them, and the page
    /// says so where the screen does: in the cut line's own cell, in the
    /// column the page keeps for it.
    #[test]
    fn a_cut_line_carries_its_gap_beside_itself() {
        let planning = with_plan(|v| {
            let cents = crate::money::Cents::from_dollars(307);
            v.plan.shortfall.bills = crate::money::Cents::from_dollars(237);
            if let Ok(transfers) = &mut v.transfers {
                transfers[0]
                    .lines
                    .push((crate::plan_line::Line::Bills, cents));
            }
        });

        // Twice: the cut line's own cell, and the `Shortfall` row that sums
        // every such cell at the foot of the block.
        assert_eq!(
            planning.matches("\u{394} 237").count(),
            2,
            "no gap: {planning}"
        );
    }

    /// An unset `planning.goal.bill_payments_id` sends the bill line out of
    /// the tracked system, and the Planning screen draws such a line plain --
    /// it is one line under a head that repeats its own figure. So does this,
    /// rather than being the one of the two sinks that annotates it. What the
    /// excess cut from it is still said, once, in the `Shortfall` row.
    #[test]
    fn a_cut_line_leaving_as_a_withdrawal_carries_no_gap_beside_itself() {
        let planning = with_plan(|v| {
            v.plan.shortfall.bills = crate::money::Cents::from_dollars(237);
            if let Ok(transfers) = &mut v.transfers {
                transfers.retain(|t| t.to.is_none());
                transfers[0].lines = vec![(
                    crate::plan_line::Line::Bills,
                    crate::money::Cents::from_dollars(307),
                )];
            }
        });

        assert_eq!(
            planning.matches("\u{394} 237").count(),
            1,
            "the withdrawal's line drew a gap the screen does not: {planning}"
        );
        assert!(planning.contains("Shortfall"), "no shortfall: {planning}");
    }

    /// The payday where the fixed bills took everything is the one the gap
    /// exists for, and it is also the one `transfer::plan` refuses: every
    /// line is zero, so there is not a transfer row in the block for a
    /// per-line gap to hang off. A block that only ever spoke through its
    /// lines would go silent exactly there.
    #[test]
    fn a_plan_that_moves_nothing_still_reports_what_the_excess_left_unpaid() {
        let planning = with_plan(|v| {
            v.plan.shortfall.current_housing = crate::money::Cents::from_dollars(693);
            v.plan.shortfall.bills = crate::money::Cents::from_dollars(544);
            v.transfers = Err(crate::transfer::NOTHING_TO_TRANSFER.into());
        });

        assert!(
            planning.contains("\u{394} 1,237"),
            "the whole plan's gap is not on the page: {planning}"
        );
    }

    /// The plug is divided by what each goal asks of this paycheck, so the
    /// page says when it will not stretch -- the same footer the screen says
    /// it in, and the same silence when it covers them.
    #[test]
    fn a_plug_short_of_the_paycheck_asks_carries_the_gap_in_the_blocks_footer() {
        let planning = with_plan(|v| {
            v.spread_ask_total = v.plan.lines.goals + crate::money::Cents::from_dollars(220)
        });

        assert!(planning.contains("Unmet Asks"), "no footer: {planning}");
        assert!(planning.contains("\u{394} -220"), "no gap: {planning}");
    }

    #[test]
    fn a_plug_that_covers_every_ask_says_nothing_below_itself() {
        let planning = with_plan(|v| v.spread_ask_total = v.plan.lines.goals);

        assert!(
            !planning.contains('\u{394}'),
            "a covered plug drew a gap: {planning}"
        );
    }

    /// `transfer::plan` skips a line at zero, so the payday whose plug is
    /// nothing has no Goals row at all -- and it is the payday whose goals
    /// are worst served. A gap hung off that row would fade out exactly as
    /// the condition it reports got worse.
    #[test]
    fn a_plug_of_nothing_still_reports_what_its_goals_asked() {
        let planning = with_plan(|v| {
            v.plan.lines.goals = crate::money::Cents::ZERO;
            v.spread_ask_total = crate::money::Cents::from_dollars(220);
            v.transfers = Err(crate::transfer::NOTHING_TO_TRANSFER.into());
        });

        assert!(planning.contains("\u{394} -220"), "no gap: {planning}");
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
        assert!(
            planning.contains("Remaining Excess"),
            "the waterfall went with it"
        );
    }
}
