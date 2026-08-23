//! The Planning tab: what the payday would move, over the waterfall that
//! decided it.
//!
//! The rows themselves come from [`crate::plan_rows`], which the Planning
//! screen reads too. What is here is only this medium's spelling of them: a
//! `<tr>` per row, a class per [`plan_rows::Kind`], and a `sub` class per
//! level of the indent the screen spends in spaces.

use super::{account, escape, full_width_row, whole_money};
use crate::money::Cents;
use crate::plan_rows::{self, Extra, Kind, RowLabel, Value};
use crate::report::{PlanView, Planning};

const COLUMNS: usize = 3;

/// A gap, in the red every gap on this page is drawn in.
fn gap(cents: Cents) -> String {
    format!(
        "<span style=\"color:{}\">\u{394} {}</span>",
        crate::palette::hex(crate::palette::NEGATIVE),
        escape(&cents.to_whole_dollars())
    )
}

/// How far one level of depth indents a label.
const INDENT: &str = "1.4rem";

/// The class a row's depth spells, or none where it takes no indent.
///
/// A class per level rather than one class for "indented": the screen spends
/// two spaces a level and goes on spending them, so a page that collapsed
/// every level past the first onto one class would draw flat a row the screen
/// draws nested -- the drift one shared list of rows exists to prevent. Depth
/// `1` still takes none, because the heading above a block's own lines has
/// already set them apart.
fn sub_class(depth: u8) -> Option<String> {
    (depth >= 2).then(|| format!("sub{depth}"))
}

/// The indent rules the depths on this page need, one per level.
///
/// Generated rather than carried in [`super::STYLE`], for the reason
/// [`super::ledger::month_rules`] is: how deep this waterfall goes is a fact
/// about the plan rather than about the stylesheet, and a constant list would
/// leave a row deeper than the deepest anybody had written classed but
/// unstyled -- flat again, and silently.
pub(super) fn depth_rules(planning: &Planning) -> String {
    let Planning::Resolved(view) = planning else {
        return String::new();
    };
    indent_rules(waterfall(view).iter().map(|row| row.depth))
}

/// One rule per level present, deepest last, and none for the levels that
/// take no indent.
fn indent_rules(depths: impl Iterator<Item = u8>) -> String {
    let mut depths: Vec<u8> = depths.filter(|depth| *depth >= 2).collect();
    depths.sort_unstable();
    depths.dedup();
    depths
        .iter()
        .map(|depth| {
            format!(
                "tr.sub{depth} td:first-child\
                 {{padding-left:calc({} * {INDENT});color:#666666}}",
                depth - 1
            )
        })
        .collect()
}

/// One row of the waterfall as a `<tr>`.
///
/// Two classes rather than one: what a row *is* -- a total, a heading -- and
/// how deep it sits are separate facts, and `tr.tot td` and
/// `tr.sub2 td:first-child` are separate rules.
fn render(row: &plan_rows::Row) -> String {
    let label = match &row.label {
        RowLabel::Text(text) => escape(text),
        RowLabel::Account(a) => account(a),
    };
    match row.kind {
        // The screen draws an empty line between blocks; the page takes its
        // spacing from `tr.head td`'s padding and needs no row for it.
        Kind::Blank => return String::new(),
        Kind::Heading => return full_width_row("head", COLUMNS, label),
        // A block that cannot resolve says why, across the whole table. Every
        // figure below is still right -- a dangling destination key stops the
        // money moving without making the waterfall wrong.
        Kind::Note => return full_width_row("note", COLUMNS, label),
        Kind::Figure | Kind::Total => {}
    }
    let mut classes: Vec<String> = Vec::new();
    if row.kind == Kind::Total {
        classes.push("tot".to_string());
    }
    classes.extend(sub_class(row.depth));
    let class = match classes.is_empty() {
        true => String::new(),
        false => format!(" class=\"{}\"", classes.join(" ")),
    };
    // Whole dollars, the precision the Planning screen shows -- cents on a
    // plan is a false claim about where a percentage landed.
    let figure = match row.value {
        Value::Money(cents) => whole_money(cents),
        Value::Count(count) => format!("<td class=\"n\">{count}</td>"),
        Value::Stated(text) => format!("<td class=\"n\">{}</td>", escape(text)),
        Value::None => "<td class=\"n\"></td>".to_string(),
    };
    let extra = match row.extra {
        Extra::Percent(pct) => format!("{}%", pct.0),
        Extra::Biweekly(cents) => escape(&cents.to_whole_dollars()),
        Extra::Gap(cents) => gap(cents),
        Extra::Date(date) => escape(&format!("{date}*")),
        Extra::None => String::new(),
    };
    format!("<tr{class}><td>{label}</td>{figure}<td class=\"n\">{extra}</td></tr>")
}

/// The rows this tab is, before they are anything HTML.
fn waterfall(view: &PlanView) -> Vec<plan_rows::Row> {
    plan_rows::rows(&plan_rows::Input {
        plan: &view.plan,
        settings: &view.settings,
        housing: &view.housing,
        other_bills: &view.other_bills,
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

fn resolved(view: &PlanView) -> String {
    let rows: String = waterfall(view).iter().map(render).collect();
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

    /// The page draws the list [`crate::plan_rows`] hands it, whole and in
    /// order -- the blanks excepted, which are the screen's separator and
    /// this medium's `tr.head` padding.
    ///
    /// That is the whole of what this module does, and it is worth pinning:
    /// the page and the Planning screen were hand-transcriptions of each
    /// other until they read one list, and they had already come to disagree
    /// about which blocks are headed at all.
    #[test]
    fn the_page_draws_the_whole_waterfall_in_the_order_it_is_given() {
        let snapshot = snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000);
        let crate::report::Planning::Resolved(view) = &snapshot.planning else {
            panic!("the fixture's plan does not resolve");
        };
        let drawn = planning(&snapshot);

        let mut at = 0;
        for expected in super::waterfall(view) {
            if expected.kind == super::Kind::Blank {
                continue;
            }
            let label = match &expected.label {
                super::RowLabel::Text(text) => super::escape(text),
                super::RowLabel::Account(a) => a.render_with(|text, _| super::escape(text)),
            };
            let found = drawn[at..]
                .find(&label)
                .unwrap_or_else(|| panic!("no {label} row, or it came out of order: {drawn}"));
            at += found;
        }
    }

    /// One row of the waterfall at a chosen depth. These two tests are about
    /// what a depth spells, and the waterfall itself carries only the two
    /// levels it happens to use today.
    fn at_depth(depth: u8) -> super::plan_rows::Row {
        super::plan_rows::Row {
            kind: super::Kind::Figure,
            label: super::RowLabel::Text(format!("Level {depth}")),
            value: super::Value::Money(crate::money::Cents::from_dollars(1)),
            extra: super::Extra::None,
            depth,
            target: None,
            edit: String::new(),
        }
    }

    /// The screen spends two spaces a level and goes on spending them, so the
    /// page spells a class a level. Collapsing every level past the first
    /// onto one class would draw flat a row the screen draws nested, which is
    /// the drift one shared list of rows exists to prevent -- and the depth
    /// is carried as a number precisely so each medium may spend it in its
    /// own units.
    #[test]
    fn each_level_of_depth_below_a_block_takes_a_class_of_its_own() {
        let drawn: Vec<String> = (0..4).map(|d| super::render(&at_depth(d))).collect();

        for shallow in &drawn[..2] {
            assert!(
                !shallow.contains("sub"),
                "a block's own line took an indent: {shallow}"
            );
        }
        assert!(drawn[2].contains("class=\"sub2\""), "{}", drawn[2]);
        assert!(drawn[3].contains("class=\"sub3\""), "{}", drawn[3]);

        let rules = super::indent_rules(0..4u8);
        assert!(
            !rules.contains("tr.sub1"),
            "an unindented level took a rule"
        );
        assert!(rules.contains("calc(1 * 1.4rem)"), "{rules}");
        assert!(rules.contains("calc(2 * 1.4rem)"), "{rules}");
    }

    /// Every indent the page spells has a rule behind it, which is what makes
    /// the class mean anything: a `sub` class the stylesheet never names
    /// renders flat, the same drift as no class at all.
    #[test]
    fn every_indent_class_the_page_draws_has_a_rule_behind_it() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let drawn = panel(&page, "planning").to_string();

        let mut indented = 0;
        for depth in 2..=9u8 {
            if drawn.contains(&format!("sub{depth}")) {
                indented += 1;
                assert!(
                    page.contains(&format!("tr.sub{depth} td:first-child")),
                    "nothing indents a depth-{depth} row"
                );
            }
        }
        assert!(indented > 0, "no indented row on the page at all: {drawn}");
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
