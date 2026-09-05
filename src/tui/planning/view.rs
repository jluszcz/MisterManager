//! The view model the waterfall is mapped into: one [`Row`] per line of the
//! screen, and the [`View`] `App` hands in to build them from.
//!
//! `plan_rows::rows` is the list; this is that list in a terminal's units.
//! What it may add is only what this medium has -- a cursor, a tint, an
//! editor -- and it may not add, drop, rename or reorder a row.

use super::Target;
use crate::calc;
use crate::calc::planning::{Plan, PlanSettings};
use crate::db::AccountId;
use crate::db::account::AccountColor;
use crate::db::bill::Bill;
use crate::money::Cents;
use crate::plan_line::{Destination, Line};
use crate::plan_rows;
use crate::transfer::{self, Container, Landing, Wiring};
use crate::tui::style::{self, Tone};
use anyhow::Result;
use chrono::NaiveDate;

/// What `e` acts on, for the rows it acts on at all.
///
/// The two are edited in different ways -- a constant is typed into a
/// one-field form, a destination is chosen from the goals that exist -- and
/// both are reached by the same key, because both are "change the thing the
/// cursor is on".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Editable {
    Constant(Target),
    Destination(Line),
}

/// One line of the screen.
///
/// Three columns: the label, the figure, and an "extra" that carries a bill's
/// biweekly amount, a split's percentage, or a destination's container. At
/// most one editable thing per row, so `e` is never ambiguous about what it
/// is editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub extra: String,
    /// What `e` would act on here, if anything. The cursor skips rows
    /// without one, so `↑`/`↓` move between the things `e` acts on.
    pub editable: Option<Editable>,
    /// What `e` prefills its field with: the stored value, unformatted for a
    /// percentage and with its cents for an amount.
    ///
    /// Deliberately not what the row *displays*. The screen floors every
    /// figure to a whole dollar (see [`Cents::to_whole_dollars`]), and
    /// prefilling the floored text would make opening `e` on a setting and
    /// pressing Enter silently drop its cents.
    pub edit: String,
    pub bold: bool,
    /// What `value` means, as far as color goes: a figure below zero, a
    /// destination that stops the plan resolving, or a gap with something on
    /// offer to fill it.
    ///
    /// A tone rather than the `Cents` themselves because this column is
    /// heterogeneous -- a figure, a count, a gate's verdict, a destination --
    /// so there is no amount to hand [`crate::tui::amount`]. Only
    /// [`plan_rows::Value::Money`] is read as an amount, which is why a count
    /// can never render red.
    pub tone: Tone,
    /// What `extra` means, as far as color goes. Only ever [`Tone::Negative`]:
    /// the one thing that cell reports rather than states is a gap the money
    /// will not cover.
    ///
    /// A second field rather than a second reader of `tone`, because the two
    /// cells say different things about the same row -- the Goals line's
    /// figure is right while the gap beside it is the problem, and one tone
    /// over both would paint the amount red for the plan's sake.
    pub extra_tone: Tone,
    /// The one account this row names, if it names one, and which of the
    /// three cells holds it.
    ///
    /// One field rather than one per column, because a row naming two
    /// accounts is not a state this screen has -- a transfer heads its own
    /// account, a destination names a goal's container or the account the
    /// line points at, and nothing names both. Making that unrepresentable
    /// is cheaper than checking it.
    pub account: Option<Tint>,
}

/// Which cell of a [`Row`] a tint applies to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Column {
    /// The transfers block: each row is headed by the account its money
    /// lands in, and the name is the row's label.
    Label,
    /// The two account-backed destination lines, which name an account
    /// where the others name a goal.
    Value,
    /// A goal's container, or the one container the plug spreads over.
    Extra,
}

/// An account a row names, the cell it names it in, and the color it draws
/// in.
///
/// The same tint the Account column carries on every other screen, resolved
/// through [`style::account_color`] like every other one: an account
/// named in one shade on Savings and another here would be two screens
/// disagreeing about the same account.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Tint {
    pub column: Column,
    pub id: AccountId,
    pub color: Option<AccountColor>,
}

impl Tint {
    fn of(container: &Container, column: Column) -> Tint {
        Tint {
            column,
            id: container.id,
            color: container.color,
        }
    }

    /// This tint's color, if it applies to `column`.
    pub(super) fn color_in(tint: Option<Tint>, column: Column) -> Option<style::Color> {
        tint.filter(|t| t.column == column)
            .map(|t| style::account_color(t.id, t.color))
    }
}

impl Row {
    fn blank() -> Row {
        Row {
            label: String::new(),
            value: String::new(),
            extra: String::new(),
            editable: None,
            edit: String::new(),
            bold: false,
            tone: Tone::Plain,
            extra_tone: Tone::Plain,
            account: None,
        }
    }

    /// One [`crate::plan_rows::Row`] in this medium: the depth spent as two
    /// spaces a level, the tones read off what each cell holds, and the
    /// editor attached to the constant the row already says it is.
    ///
    /// Every row above the Destinations block comes through here. What the
    /// screen adds is what only a terminal has -- an indent, a tint, a
    /// cursor, a bold -- and nothing it adds may change which rows there are
    /// or what they are called.
    fn of(row: &plan_rows::Row) -> Row {
        let indent = "  ".repeat(usize::from(row.depth));
        // The account a transfer lands in is named once, at the head of the
        // group it heads, so it is tinted in the label column rather than in
        // the two a destination row uses. `render_with` is the only reader of
        // its text, here as everywhere: the name and the color arrive
        // together or not at all.
        let (label, account) = match &row.label {
            plan_rows::RowLabel::Text(text) => (format!("{indent}{text}"), None),
            plan_rows::RowLabel::Account(a) => a.render_with(|text, color| {
                (
                    format!("{indent}{text}"),
                    Some(Tint {
                        column: Column::Label,
                        id: a.id(),
                        color: Some(color),
                    }),
                )
            }),
        };
        // Only money can be negative. The value column carries counts and
        // gate verdicts too, and a count is a number rather than an amount --
        // which is what keeps `26` from ever rendering red.
        let (value, tone) = match row.value {
            plan_rows::Value::Money(cents) => (
                crate::demo::whole_figure(cents),
                match cents < Cents::ZERO {
                    true => Tone::Negative,
                    false => Tone::Plain,
                },
            ),
            plan_rows::Value::Count(count) => (count.to_string(), Tone::Plain),
            plan_rows::Value::Stated(text) => (text.to_string(), Tone::Plain),
            plan_rows::Value::None => (String::new(), Tone::Plain),
        };
        // The one thing this cell reports rather than states is a gap the
        // money will not cover, which is why it is the only one that colors.
        let (extra, extra_tone) = match row.extra {
            plan_rows::Extra::Percent(pct) => (format!("{}%", pct.0), Tone::Plain),
            plan_rows::Extra::Biweekly(cents) => (crate::demo::whole_figure(cents), Tone::Plain),
            plan_rows::Extra::Gap(cents) => (
                format!("\u{394} {}", crate::demo::whole_figure(cents)),
                Tone::Negative,
            ),
            // Marked `*` like the Overview column it follows.
            plan_rows::Extra::Date(date) => (format!("{date}*"), Tone::Plain),
            plan_rows::Extra::None => (String::new(), Tone::Plain),
        };
        Row {
            label,
            value,
            extra,
            editable: row.target.map(Editable::Constant),
            edit: row.edit.clone(),
            bold: matches!(row.kind, plan_rows::Kind::Heading | plan_rows::Kind::Total),
            tone,
            extra_tone,
            account,
        }
    }

    /// The transfers block when there is no block: the reason, in the shape
    /// this screen states a failure in.
    ///
    /// Nothing in any line is not a plan that failed to resolve: there is
    /// nothing to resolve, no goal in the wrong container, and nothing
    /// `Enter` could explain. So it says so in its own words rather than
    /// under a label that reads as a failure.
    fn note(message: &str) -> Row {
        match message == transfer::NOTHING_TO_TRANSFER {
            true => Row::figure_text(&format!("  {message}"), ""),
            false => Row::figure_text("  unresolved", message),
        }
    }

    fn heading(label: &str) -> Row {
        Row {
            label: label.to_string(),
            bold: true,
            ..Row::blank()
        }
    }

    /// Where one line's money lands: the goal or account in the value
    /// column, and its container beside it.
    ///
    /// A suggestion displaces the container, which is empty for every state
    /// that can carry one -- an unset key and a dangling one both name
    /// nothing to put there. The trailing `?` is the whole of what marks it
    /// as a question rather than a setting: nothing is stored until the
    /// owner answers it.
    fn destination(w: &Wiring) -> Row {
        // Which of the two right-hand cells names an account, if either.
        // The account-backed lines name theirs in `value`; every other
        // landing that names one at all puts its container in `extra`.
        let mut tint = None;
        let (value, mut extra) = match &w.landing {
            Landing::Goal { goal, container } => {
                tint = Some(Tint::of(container, Column::Extra));
                (
                    crate::demo::text(goal).into_owned(),
                    crate::demo::text(&container.name).into_owned(),
                )
            }
            Landing::Account { account } => {
                tint = Some(Tint::of(account, Column::Value));
                (crate::demo::text(&account.name).into_owned(), String::new())
            }
            Landing::Spread { container } => {
                tint = Some(Tint::of(container, Column::Extra));
                (
                    "spread".to_string(),
                    crate::demo::text(&container.name).into_owned(),
                )
            }
            // Named while they fit and counted past that: the cell is
            // right-aligned, so an overflowing list would lose its *leading*
            // characters and read as a shorter list of the wrong containers.
            // `Enter` has room for all of them.
            Landing::Ambiguous { containers } => (
                "ambiguous".to_string(),
                match containers.len() {
                    0..=2 => containers
                        .iter()
                        .map(|c| crate::demo::text(c).into_owned())
                        .collect::<Vec<_>>()
                        .join(", "),
                    n => format!("{n} containers"),
                },
            ),
            Landing::Nowhere => ("nowhere to spread".to_string(), String::new()),
            Landing::Withdrawal => ("withdrawal".to_string(), String::new()),
            Landing::Dangling { .. } => match w.line.destination() {
                Destination::Account(_) => ("no such account".to_string(), String::new()),
                _ => ("no such goal".to_string(), String::new()),
            },
        };
        if let Some(goal) = &w.suggestion {
            extra = format!("{}?", crate::demo::text(&goal.name));
            // The cell is a goal's name now, not a container's.
            if matches!(
                tint,
                Some(Tint {
                    column: Column::Extra,
                    ..
                })
            ) {
                tint = None;
            }
        }
        Row {
            label: format!("  {}", w.line.label()),
            value,
            extra,
            // Red outranks amber: a line that stops the plan is not also a
            // suggestion worth browsing.
            tone: if w.landing.breaks_the_plan() {
                Tone::Negative
            } else if w.suggestion.is_some() {
                Tone::Warning
            } else {
                Tone::Plain
            },
            // Three kinds of row are read-only here. The plug has no key to
            // point anywhere. The two account lines hold an account id
            // rather than a goal's, and unset there means "leaves the
            // tracked system", which is how they are meant to stand.
            //
            // And the two gate-backed lines borrow `gate::Gate`'s key --
            // deliberately, so a line and its gate cannot name two different
            // goals. Repointing one from here would not be choosing a
            // destination at all: `plan::compute_from_db` reads that same key
            // as the gate's remaining shortfall, so the pick would silently
            // decide whether the gate fires and re-route four other lines'
            // amounts on the next reload. The Gates block above is where that
            // belongs; a destination row must not be a second, quieter door
            // to it.
            editable: (matches!(w.line.destination(), Destination::Goal(_))
                && w.line.gate().is_none())
            .then_some(Editable::Destination(w.line)),
            account: tint,
            ..Row::blank()
        }
    }

    /// A label and a plain string, for the one row that reports a failure
    /// rather than a figure.
    fn figure_text(label: &str, value: &str) -> Row {
        Row {
            label: label.to_string(),
            value: value.to_string(),
            ..Row::blank()
        }
    }
}

/// Everything the screen renders, as `App` gathers it.
pub struct View {
    pub plan: Plan,
    pub settings: PlanSettings,
    pub housing: Vec<Bill>,
    pub other_bills: Vec<Bill>,
    pub pinned: Option<Cents>,
    pub pinned_at: Option<NaiveDate>,
    /// The ad-hoc date the plan was computed at, when the Overview's scrub
    /// has moved it off the derived one. `None` means the screen is quoting
    /// the date the paycheck recurring transaction derives, which the columns
    /// on the Overview already name.
    pub scrubbed_adhoc: Option<NaiveDate>,
    /// Where each line's money lands, whether or not the transfers below
    /// could be resolved -- the block is most worth reading when they could
    /// not.
    pub wiring: Vec<Wiring>,
    /// The rows `t` would write, already grouped and summed.
    pub transfers: Vec<transfer::Row>,
    /// What the goals the plug spreads over ask of this paycheck, summed --
    /// `transfer::spread_asks`, which is the same set and the same pricing
    /// `t`'s own prefill divides the Goals line by.
    pub spread_ask_total: Cents,
    /// Why they could not be resolved, when they could not. A misconfigured
    /// destination must not take the whole screen down: every other figure on
    /// it is still correct and still worth reading.
    pub transfer_error: Option<String>,
    /// The same failure at the length a panel can hold. Empty exactly when
    /// the plan resolves, which is what tells `Enter` there is nothing to
    /// open.
    pub transfer_detail: Vec<String>,
}

/// The transfers `t` would write, then `Planning!C1:G41` top to bottom under
/// them.
pub(super) fn build(view: &View) -> Result<Vec<Row>> {
    let bills = |list: &[Bill]| -> Result<Vec<plan_rows::Bill>> {
        list.iter()
            .map(|b| {
                Ok(plan_rows::Bill {
                    id: b.id,
                    label: crate::demo::text(&b.label).into_owned(),
                    monthly: b.cents,
                    // Unclamped: `calc::biweekly` clamps its own denominator,
                    // so a nonsense count is already prevented from taking the
                    // screen down. A second clamp here would be a protection
                    // that cannot fire, and the report's copy of this mapping
                    // does not make it either.
                    biweekly: calc::biweekly(b.cents, view.settings.periods_per_year)?,
                })
            })
            .collect()
    };
    let housing = bills(&view.housing)?;
    let other_bills = bills(&view.other_bills)?;
    let transfers = plan_rows::transfers(&view.transfers);
    let waterfall = plan_rows::rows(&plan_rows::Input {
        plan: &view.plan,
        settings: &view.settings,
        housing: &housing,
        other_bills: &other_bills,
        // A misconfigured destination is a block's content, not the screen's:
        // every figure below it is still right.
        transfers: match &view.transfer_error {
            Some(message) => Err(message.as_str()),
            None => Ok(&transfers),
        },
        spread_ask_total: view.spread_ask_total,
        scrubbed_adhoc: view.scrubbed_adhoc,
    });
    let mut rows: Vec<Row> = waterfall
        .iter()
        .map(|row| match row.kind {
            plan_rows::Kind::Note => Row::note(match &row.label {
                plan_rows::RowLabel::Text(message) => message,
                plan_rows::RowLabel::Account(_) => "",
            }),
            _ => Row::of(row),
        })
        .collect();

    // Where the money the split just divided lands. Below the figures rather
    // than beside the transfers at the top: this block is read when one of
    // them is missing or wrong, which is a question about the line above it.
    //
    // Screen-only, and the one block `plan_rows` does not carry: a
    // destination is a thing the owner *changes*, and the report has no way
    // to offer that.
    rows.push(Row::blank());
    rows.push(Row::heading("Destinations"));
    rows.extend(view.wiring.iter().map(Row::destination));

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::{AccountId, BillId};
    use crate::money::Cents;
    use crate::plan_line::{Destination, Line};
    use crate::plan_rows;

    use crate::test_support::day;
    use crate::transfer::{self, Landing};

    use crate::tui::style::Tone;

    use crate::tui::planning::Planning;
    use crate::tui::planning::test_support::*;

    /// One waterfall row in this medium. These tests are about the mapper
    /// rather than the list it maps, so the row is built by hand: what
    /// `plan_rows` puts in each cell is pinned by its own tests.
    fn mapped(kind: plan_rows::Kind, value: plan_rows::Value) -> Row {
        Row::of(&plan_rows::Row {
            kind,
            label: plan_rows::RowLabel::Text("Remaining Excess".to_string()),
            value,
            extra: plan_rows::Extra::None,
            depth: 1,
            target: None,
            edit: String::new(),
        })
    }

    /// The value column carries figures, counts and gate verdicts alike, so
    /// only a cell holding money gets to call a row negative. A count is a
    /// number, not an amount, and must never render red.
    #[test]
    fn only_a_money_row_below_zero_is_toned_negative() {
        use plan_rows::{Kind, Value};

        assert_eq!(
            mapped(Kind::Figure, Value::Money(Cents(-1))).tone,
            Tone::Negative
        );
        assert_eq!(
            mapped(Kind::Total, Value::Money(Cents(-100))).tone,
            Tone::Negative
        );
        assert_eq!(
            mapped(Kind::Figure, Value::Money(Cents::ZERO)).tone,
            Tone::Plain
        );
        assert_eq!(
            mapped(Kind::Total, Value::Money(Cents(1))).tone,
            Tone::Plain
        );
        assert_eq!(mapped(Kind::Figure, Value::Count(26)).tone, Tone::Plain);
        assert_eq!(
            mapped(Kind::Figure, Value::Stated("needed")).tone,
            Tone::Plain
        );
        assert_eq!(Row::heading("Gates").tone, Tone::Plain);
    }

    /// The pay-period count is user-editable, so a zero reaches this screen's
    /// bill mapping. `build` divides by the setting as stored -- the only
    /// clamp is `calc::biweekly`'s own, the twin of
    /// `a_zero_pay_period_count_still_produces_a_plan` a layer down -- so this
    /// is what says the screen still draws rather than surfacing `div_ceil`'s
    /// error. The figures are not meaningful here; building the rows is.
    #[test]
    fn a_zero_pay_period_count_still_builds_the_screen() {
        let mut v = view(None, None);
        v.settings.periods_per_year = 0;
        let mut planning = Planning::new();

        planning.set_view(v).unwrap();

        assert!(!planning.rows().is_empty());
    }

    /// The Overview marks its scrubbed column and this screen has no column
    /// header to mark, so the date goes in the extra column of the one row a
    /// scrub moves. Silence would leave a screen quoting a hypothetical
    /// balance with nothing on it saying so.
    #[test]
    fn a_scrubbed_plan_names_its_date_beside_excess_actual() {
        let mut v = view(None, None);
        v.scrubbed_adhoc = Some(day(2026, 8, 29));
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        assert_eq!(row(&planning, "Excess (Actual)").extra, "2026-08-29*");
    }

    #[test]
    fn an_unscrubbed_plan_leaves_the_excess_actual_extra_column_empty() {
        let planning = screen();

        assert_eq!(row(&planning, "Excess (Actual)").extra, "");
    }

    /// The plug covering every ask is the ordinary payday, and a figure that
    /// says "nothing is wrong" on every ordinary payday is a figure nobody
    /// reads.
    #[test]
    fn a_plug_that_covers_every_ask_draws_no_unmet_asks_row() {
        let mut v = view(None, None);
        v.spread_ask_total = v.plan.lines.goals;
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        assert!(
            planning
                .rows()
                .iter()
                .all(|r| r.label.trim() != "Unmet Asks"),
            "a covered plug drew a gap"
        );
        assert_eq!(transfer_line(&planning, Line::Goals).extra, "");
    }

    /// The payday where the fixed bills took everything is the one the gap
    /// exists for, and it is also the one `transfer::plan` finds nothing to
    /// move on: every line is zero, so there is not a transfer row in the
    /// block for a per-line gap to hang off. A block that only ever spoke
    /// through its lines would go silent exactly there.
    #[test]
    fn a_plan_that_moves_nothing_still_reports_what_the_excess_left_unpaid() {
        let mut v = view(Some(Cents::ZERO), None);
        let total = v.plan.shortfall.total();
        assert!(total > Cents::ZERO, "the fixed bills were paid in full");
        v.transfers = Vec::new();
        v.transfer_error = Some(transfer::NOTHING_TO_TRANSFER.to_string());
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let row = row(&planning, "Shortfall");
        assert_eq!(row.extra, format!("\u{394} {}", total.to_whole_dollars()));
        assert_eq!(row.extra_tone, Tone::Negative);
    }

    /// A figure saying "nothing is wrong" on every ordinary payday is a
    /// figure nobody reads, so the row is absent rather than zero.
    #[test]
    fn a_plan_that_pays_every_bill_in_full_draws_no_shortfall_row() {
        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        assert!(
            planning
                .rows()
                .iter()
                .all(|r| r.label.trim() != "Shortfall"),
            "a covered plan drew a shortfall row"
        );
    }

    /// What the plug moves is divided by `calc::fit`, which scales every goal
    /// to the same fraction of what it asked when the money will not stretch.
    /// Nothing else on the screen says that is about to happen: the line's own
    /// figure is correct, and each goal's `$/Pay` is on another screen
    /// entirely.
    #[test]
    fn a_plug_short_of_the_paycheck_asks_carries_the_gap_in_the_blocks_footer() {
        let mut v = view(None, None);
        let moves = v.plan.lines.goals;
        v.spread_ask_total = moves + Cents::from_dollars(220);
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let footer = row(&planning, "Unmet Asks");
        assert_eq!(footer.extra, "\u{394} -220");
        assert_eq!(footer.extra_tone, Tone::Negative);
        // The line's own amount is untouched -- it is what the payday moves,
        // and it is not the thing that is wrong.
        let line = transfer_line(&planning, Line::Goals);
        assert_eq!(line.value, moves.to_whole_dollars());
        assert_eq!(line.tone, Tone::Plain);
        assert_eq!(line.extra, "");
    }

    /// `transfer::plan` skips a line at zero, so the payday whose plug is
    /// nothing has no Goals row at all -- and it is the payday whose goals
    /// are worst served. A gap hung off that row would fade out exactly as
    /// the condition it reports got worse.
    #[test]
    fn a_plug_of_nothing_still_reports_what_its_goals_asked() {
        let mut v = view(Some(Cents::ZERO), None);
        assert_eq!(v.plan.lines.goals, Cents::ZERO, "the plug moved something");
        v.spread_ask_total = Cents::from_dollars(220);
        v.transfers = Vec::new();
        v.transfer_error = Some(transfer::NOTHING_TO_TRANSFER.to_string());
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        assert_eq!(row(&planning, "Unmet Asks").extra, "\u{394} -220");
    }

    /// The transfers never total more than the excess, so the heading has
    /// nothing left to report and carries nothing in either column -- the
    /// shortfall it used to hold now sits on the line that took it.
    #[test]
    fn the_transfers_heading_is_a_heading_and_nothing_else() {
        let planning = screen();

        assert_eq!(row(&planning, "Transfers").value, "");
        assert_eq!(row(&planning, "Transfers").extra, "");
        assert!(
            planning.rows().iter().all(|r| r.label.trim() != "Checksum"),
            "the foot of the screen still carries a Checksum row"
        );
    }

    /// A plan with nothing in any line is not a plan that failed to resolve:
    /// there is nothing to resolve. It says so in its own words rather than
    /// under a label that reads as a failure and offers an explanation there
    /// is none of.
    #[test]
    fn a_plan_with_nothing_to_transfer_does_not_call_itself_unresolved() {
        let mut v = view(Some(Cents::ZERO), None);
        v.transfers = Vec::new();
        v.transfer_error = Some(transfer::NOTHING_TO_TRANSFER.to_string());
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        assert!(
            planning
                .rows()
                .iter()
                .all(|r| r.label.trim() != "unresolved"),
            "an empty plan called itself unresolved"
        );
        assert_eq!(row(&planning, transfer::NOTHING_TO_TRANSFER).value, "");
    }

    /// A plan that genuinely cannot be resolved keeps the label: the message
    /// says what went wrong, and `Enter` has the rest of it.
    #[test]
    fn a_plan_that_cannot_resolve_still_labels_itself_unresolved() {
        let mut v = view(None, None);
        v.transfers = Vec::new();
        v.transfer_error = Some("the Bills goal is gone".to_string());
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        assert_eq!(row(&planning, "unresolved").value, "the Bills goal is gone");
    }

    /// A payday too small for the fixed bills cuts one of them, and the line
    /// that was cut is where that is said: its own figure is right, and
    /// nothing else on the screen says it was meant to be larger.
    #[test]
    fn a_cut_bills_line_carries_the_gap_in_its_extra_cell() {
        // 693 of housing and 544 of other bills, against 1,000: housing is
        // paid in full and Bills takes the 237.
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(1_000)), None))
            .unwrap();

        let bills = transfer_line(&planning, Line::Bills);
        assert_eq!(bills.extra, "\u{394} 237");
        assert_eq!(bills.extra_tone, Tone::Negative);
    }

    /// Housing is paid first, so it is whole on the very payday that cuts
    /// Bills -- and a line that got what it asked for says nothing.
    #[test]
    fn the_line_that_was_paid_in_full_carries_no_gap() {
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(1_000)), None))
            .unwrap();

        assert_eq!(transfer_line(&planning, Line::CurrentHousing).extra, "");
    }

    #[test]
    fn a_configured_destination_names_its_goal_and_its_container() {
        let planning = screen();
        let row = destination(&planning, Line::EmergencyFund);
        assert_eq!(row.value, "Emergency Savings");
        assert_eq!(row.extra, "Brokerage");
    }

    /// The same row, under a demo: `wiring` hands `Landing::Goal` a real
    /// goal name, and it reached this cell unmasked while the container
    /// beside it did not.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_goal_a_configured_destination_names() {
        crate::demo::install_with_salt(7);
        let planning = screen();
        let row = destination(&planning, Line::EmergencyFund);
        assert_ne!(row.value, "Emergency Savings");
        assert_eq!(row.value, crate::demo::text("Emergency Savings"));
        assert_ne!(row.extra, "Brokerage");
        assert_eq!(row.extra, crate::demo::text("Brokerage"));
    }

    /// Unset means the money leaves the tracked system, which is what two of
    /// the nine lines are supposed to do -- so the state is named for what it
    /// does rather than for the key being empty.
    #[test]
    fn an_unset_destination_reads_as_the_withdrawal_it_is() {
        let planning = screen();
        assert_eq!(destination(&planning, Line::Retirement).value, "withdrawal");
        assert_eq!(destination(&planning, Line::Retirement).extra, "");
        assert_eq!(destination(&planning, Line::Retirement).tone, Tone::Plain);
    }

    /// The question mark is the whole of what says this is a question:
    /// nothing is stored until it is answered.
    #[test]
    fn an_unset_line_with_a_match_shows_it_as_a_question() {
        let planning = screen();
        let row = destination(&planning, Line::FutureHousing);
        assert_eq!(row.value, "withdrawal");
        assert_eq!(row.extra, "Home Down Payment?");
    }

    /// Amber, not red. The plan below still resolves -- the money goes out
    /// rather than nowhere -- so a suggestion must not wear the color of a
    /// plan that cannot run.
    #[test]
    fn a_suggestion_is_toned_as_a_prompt_rather_than_a_failure() {
        let planning = screen();
        assert_eq!(
            destination(&planning, Line::FutureHousing).tone,
            Tone::Warning
        );
    }

    #[test]
    fn the_plug_names_the_container_it_spreads_over() {
        let planning = screen();
        let row = destination(&planning, Line::Goals);
        assert_eq!(row.value, "spread");
        assert_eq!(row.extra, "Rainy Day");
    }

    /// The state the block exists to make visible, and the one that stops
    /// `t` writing anything.
    #[test]
    fn a_plug_spanning_two_containers_is_toned_like_the_failure_it_is() {
        let planning = screen_with(
            Line::Goals,
            Landing::Ambiguous {
                containers: vec!["Rainy Day".to_string(), "Brokerage".to_string()],
            },
        );
        let row = destination(&planning, Line::Goals);
        assert_eq!(row.value, "ambiguous");
        assert_eq!(row.extra, "Rainy Day, Brokerage");
        assert_eq!(row.tone, Tone::Negative);
    }

    /// Every sibling landing that names an account here -- `Goal`, `Account`,
    /// `Spread` -- masks it, and an ambiguous plug is not an exception:
    /// `diagnose`'s panel shows the same two containers as pseudonyms, and a
    /// screen showing the real names beside it would hand a viewer the
    /// mapping.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_containers_an_ambiguous_plug_names() {
        crate::demo::install_with_salt(7);
        let planning = screen_with(
            Line::Goals,
            Landing::Ambiguous {
                containers: vec!["Rainy Day".to_string(), "Brokerage".to_string()],
            },
        );
        let row = destination(&planning, Line::Goals);
        assert_ne!(row.extra, "Rainy Day, Brokerage");
        assert_eq!(
            row.extra,
            format!(
                "{}, {}",
                crate::demo::text("Rainy Day"),
                crate::demo::text("Brokerage")
            )
        );
    }

    /// Two container names fill the cell exactly; a third would overflow it,
    /// and a right-aligned overflow loses its front rather than its end --
    /// so the row would name the wrong containers rather than too many.
    #[test]
    fn more_containers_than_the_cell_holds_are_counted_rather_than_named() {
        let planning = screen_with(
            Line::Goals,
            Landing::Ambiguous {
                containers: vec![
                    "Rainy Day".to_string(),
                    "Brokerage".to_string(),
                    "Nest Egg".to_string(),
                ],
            },
        );
        assert_eq!(destination(&planning, Line::Goals).extra, "3 containers");
    }

    /// A key naming a goal that is gone is corruption, not a gap, and reading
    /// it as "unset" would say the money leaves the tracked system on purpose.
    #[test]
    fn a_dangling_key_says_the_goal_is_gone_rather_than_reading_as_unset() {
        let planning = screen_with(
            Line::Bills,
            Landing::Dangling {
                key: "planning.goal.bill_payments_id".to_string(),
            },
        );
        let row = destination(&planning, Line::Bills);
        assert_eq!(row.value, "no such goal");
        assert_eq!(row.tone, Tone::Negative);
    }

    /// The transfers block heads each row with the account the money lands
    /// in, so that name is tinted like the same account everywhere else --
    /// in the *label* column, which is where a transfer names its account
    /// and where no other row on the screen names one.
    ///
    /// The shade arrives already resolved: the name reaches this row through
    /// `account_label::Account::render_with`, which hands the derived color
    /// with it, so a container the owner has never colored still comes out in
    /// the one shade it wears on every other screen.
    #[test]
    fn a_transfer_row_is_tinted_by_the_account_it_lands_in() {
        let planning = screen();
        let row = planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "Rainy Day")
            .expect("no Rainy Day transfer row");

        assert_eq!(
            row.account.map(|t| (t.column, t.id)),
            Some((Column::Label, AccountId(1)))
        );
        assert_eq!(
            Tint::color_in(row.account, Column::Label),
            Some(crate::tui::style::account_color(AccountId(1), None))
        );
    }

    /// The account is said once, at the head of the group it heads. The
    /// lines beneath it are the plan's own labels and name no account, so a
    /// tint on them would claim they did.
    #[test]
    fn the_lines_under_a_transfer_carry_no_tint_of_their_own() {
        let planning = screen();
        let rows = planning.rows();
        let head = rows
            .iter()
            .position(|r| r.label.trim() == "Rainy Day")
            .expect("no Rainy Day transfer row");
        // Every row until the next one carrying a tint or heading a block.
        let children = rows[head + 1..]
            .iter()
            .take_while(|r| r.label.starts_with("    "));
        let mut counted = 0;
        for child in children {
            assert_eq!(child.account, None, "{:?} is tinted", child.label);
            counted += 1;
        }
        assert!(counted > 0, "the transfer had no lines under it");
    }

    /// Money leaving the tracked system lands in no account, so the
    /// Withdrawal row has nothing to be tinted by -- the same way the
    /// Destinations block draws a withdrawal plain.
    #[test]
    fn a_withdrawal_row_carries_no_tint() {
        let mut v = view(None, None);
        v.transfers = vec![transfer::Row::Withdrawal {
            line: Line::Retirement,
            cents: Cents::from_dollars(2_070),
        }];
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let row = planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "Withdrawal")
            .expect("no Withdrawal row");
        assert_eq!(row.account, None);
    }

    /// A container named here is the same account named on Savings and on the
    /// ledgers, so it takes the same color -- which is the whole request.
    /// It sits in the `extra` column, where the container name goes.
    #[test]
    fn a_destination_carries_its_containers_color_in_the_extra_column() {
        let planning = screen();
        let row = destination(&planning, Line::MomAndDad);
        assert_eq!(row.extra, "Brokerage");
        // In the extra column, where the container name goes -- the value
        // column names a *goal*, which belongs to no account.
        assert_eq!(
            row.account,
            Some(Tint {
                column: Column::Extra,
                id: AccountId(2),
                color: None
            })
        );
    }

    /// The two account-backed lines name an account in the value column
    /// rather than a container in the extra one, so that is where their tint
    /// goes.
    #[test]
    fn an_account_backed_destination_carries_its_color_in_the_value_column() {
        let planning = screen_with(
            Line::Retirement,
            Landing::Account {
                account: container("Brokerage"),
            },
        );
        let row = destination(&planning, Line::Retirement);
        assert_eq!(row.value, "Brokerage");
        assert_eq!(
            row.account,
            Some(Tint {
                column: Column::Value,
                id: AccountId(2),
                color: None
            })
        );
    }

    /// The plug spreads into one container, and that container is an account
    /// like any other.
    #[test]
    fn the_plugs_container_is_tinted_like_every_other_container() {
        let planning = screen();
        let row = destination(&planning, Line::Goals);
        assert_eq!(row.value, "spread");
        assert_eq!(
            row.account,
            Some(Tint {
                column: Column::Extra,
                id: AccountId(1),
                color: None
            })
        );
    }

    /// A suggestion *displaces* the container, so what is in that cell is a
    /// goal's name. Leaving the tint behind would paint a goal in an
    /// account's color and claim a relationship that is not there.
    #[test]
    fn a_suggestion_leaves_no_container_tint_behind_it() {
        let planning = screen();
        let row = destination(&planning, Line::FutureHousing);
        assert_eq!(row.extra, "Home Down Payment?");
        assert_eq!(row.account, None);
    }

    /// The suggestion keeps its `?`, and the goal name ahead of it is
    /// masked -- the same rule the configured destinations follow, so a
    /// prompt does not publish the one goal name a resolved line would
    /// have hidden.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_goal_a_suggestion_names_but_keeps_its_question_mark() {
        crate::demo::install_with_salt(7);
        let planning = screen();
        let row = destination(&planning, Line::FutureHousing);
        assert_ne!(row.extra, "Home Down Payment?");
        assert_eq!(
            row.extra,
            format!("{}?", crate::demo::text("Home Down Payment"))
        );
    }

    /// Nothing single is named, so there is nothing to tint: an ambiguous
    /// plug spans several containers and a withdrawal leaves the system.
    #[test]
    fn a_landing_naming_no_single_account_carries_no_tint() {
        let ambiguous = screen_with(
            Line::Goals,
            Landing::Ambiguous {
                containers: vec!["Rainy Day".to_string(), "Brokerage".to_string()],
            },
        );
        let row = destination(&ambiguous, Line::Goals);
        assert_eq!(row.account, None);

        let plain = screen();
        let withdrawal = destination(&plain, Line::Retirement);
        assert_eq!(withdrawal.value, "withdrawal");
        assert_eq!(withdrawal.account, None);
    }

    /// The Roth and Emergency Fund lines borrow the *gate's* key -- one id,
    /// deliberately, so the line and the gate cannot point at two different
    /// goals. Which means `e` on those rows would not be repointing a
    /// transfer at all: it would be choosing what the waterfall gates on,
    /// silently re-routing four other lines' amounts on the next reload.
    /// Reading them here is right; editing them from here is not.
    #[test]
    fn a_gate_backed_destination_is_not_editable_from_the_block() {
        let planning = screen();
        for line in [Line::Roth, Line::EmergencyFund] {
            assert_eq!(
                destination(&planning, line).editable,
                None,
                "{line:?} would rewrite {:?}",
                line.import_substring()
            );
        }
    }

    /// The two account lines hold an account id, and unset there is how they
    /// are meant to stand. The plug has no key at all. And the two
    /// gate-backed lines share a key with something bigger than a
    /// destination -- see the test above.
    #[test]
    fn only_the_lines_that_own_their_key_are_editable_from_the_block() {
        let planning = screen();
        for line in Line::ALL {
            let expected =
                matches!(line.destination(), Destination::Goal(_)) && line.gate().is_none();
            assert_eq!(
                destination(&planning, line).editable == Some(Editable::Destination(line)),
                expected,
                "{line:?}"
            );
        }
    }

    /// Every constant `Target` names has to be reachable, exactly once. A
    /// variant with no row is a setting the screen claims to edit and does
    /// not; a duplicated one is two rows writing the same key.
    #[test]
    fn every_constant_target_appears_on_exactly_one_row() {
        let planning = screen();
        let mut targets: Vec<Target> = planning
            .rows()
            .iter()
            .filter_map(|r| match r.editable {
                Some(Editable::Constant(target)) => Some(target),
                _ => None,
            })
            .filter(|t| !matches!(t, Target::Bill(_)))
            .collect();
        let before = targets.len();
        targets.sort_by_key(|t| format!("{t:?}"));
        targets.dedup();
        assert_eq!(targets.len(), before, "two rows write the same setting");
        assert_eq!(
            before, 11,
            "every Target variant but Bill must have a row: {targets:?}"
        );
    }

    #[test]
    fn every_bill_gets_its_own_row_carrying_its_own_id() {
        let planning = screen();
        let bills: Vec<Target> = planning
            .rows()
            .iter()
            .filter_map(|r| match r.editable {
                Some(Editable::Constant(target)) => Some(target),
                _ => None,
            })
            .filter(|t| matches!(t, Target::Bill(_)))
            .collect();
        assert_eq!(
            bills,
            (1..=6).map(|i| Target::Bill(BillId(i))).collect::<Vec<_>>()
        );
    }

    /// `Planning!C6` is a subtotal, not a bill: it has no row in the table and
    /// nothing may edit it. Its biweekly figure is `E6`, the one that feeds
    /// `lines.current_housing`.
    #[test]
    fn the_bill_block_shows_the_housing_subtotal_and_a_biweekly_column() {
        let planning = screen();
        let subtotal = row(&planning, "Mortgage + HOA");
        assert_eq!(
            subtotal.value,
            Cents::from_dollars(1_500).to_whole_dollars()
        );
        assert_eq!(subtotal.extra, Cents::from_dollars(693).to_whole_dollars());
        assert_eq!(subtotal.editable, None, "a subtotal is not editable");

        // Each bill's own biweekly figure, rounded up per bill -- 1,200 * 12
        // / 26 = 553.85, which the sheet's E7 carries as 554.
        assert_eq!(
            row(&planning, "Mortgage").extra,
            Cents::from_dollars(554).to_whole_dollars()
        );
        assert_eq!(
            row(&planning, "Coworking").extra,
            Cents::from_dollars(462).to_whole_dollars()
        );
    }

    /// The Goals share is whatever the other three leave. Giving it a target
    /// would let the four sum to something other than 100 with no way to say
    /// which one is wrong.
    #[test]
    fn the_goals_split_is_computed_and_not_editable() {
        let planning = screen();
        let goals = planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "Goals" && r.extra == "35%")
            .expect("no Goals split row");
        assert_eq!(goals.editable, None);
    }

    #[test]
    fn a_percentage_row_prefills_the_bare_number_and_shows_the_sign() {
        let planning = screen();
        let split = planning
            .rows()
            .iter()
            .find(|r| r.editable == Some(Editable::Constant(Target::FutureHousingPct)))
            .unwrap();
        assert_eq!(split.extra, "35%");
        assert_eq!(split.edit, "35");
    }

    #[test]
    fn a_money_row_prefills_the_cents_its_figure_drops() {
        let planning = screen();
        let target = row(&planning, "Target");
        assert_eq!(target.value, "10,000");
        assert_eq!(target.edit, "10,000.00");
    }

    /// Every figure on the screen is a whole dollar, so no row carries a
    /// decimal point. The percentages in the extra column are not money and
    /// keep their own format, and neither is `Pay Periods / Year`.
    #[test]
    fn no_figure_on_the_screen_shows_cents() {
        let planning = screen();
        for r in planning.rows() {
            assert!(
                !r.value.contains('.'),
                "{:?} shows cents in {:?}",
                r.label,
                r.value
            );
            assert!(
                !r.extra.contains('.'),
                "{:?} shows cents in the extra column: {:?}",
                r.label,
                r.extra
            );
        }
    }

    /// A negative drops its cents the same way a positive does, so the two
    /// signs of one figure read as the same number. `Checksum` is the row
    /// that goes below zero.
    #[test]
    fn a_negative_figure_drops_its_cents_like_a_positive_one() {
        use plan_rows::{Kind, Value};

        assert_eq!(
            mapped(Kind::Figure, Value::Money(Cents(-20_099))).value,
            "-200"
        );
        assert_eq!(
            mapped(Kind::Figure, Value::Money(Cents(20_099))).value,
            "200"
        );
    }

    /// Dropping the digits leaves a sub-dollar negative as `-0` -- worth
    /// knowing before reading one as a bug in the waterfall rather than in
    /// the format. This screen renders its figures itself, through
    /// `demo::whole_figure`; Savings goes through `whole_amount`, which
    /// truncates the `Cents` and so draws the same remainder as a plain `0`.
    #[test]
    fn a_negative_under_a_dollar_keeps_its_sign_over_a_zero() {
        assert_eq!(
            mapped(plan_rows::Kind::Figure, plan_rows::Value::Money(Cents(-1))).value,
            "-0"
        );
    }

    #[test]
    fn the_pin_line_names_the_date_and_the_drift() {
        let mut planning = Planning::new();
        planning
            .set_view(view(
                Some(Cents::from_dollars(17_500)),
                Some(day(2026, 8, 14)),
            ))
            .unwrap();
        // Excess actual is 17,500.75 against a 17,500.00 pin.
        assert_eq!(
            planning.pin_line().unwrap(),
            "pinned 17,500.00 on 2026-08-14 · excess has since moved 0.75"
        );
    }

    /// The pin line is two figures in prose rather than a column, and both
    /// of them are money. The date it was pinned on is not.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_pin_and_the_drift_it_has_moved_by() {
        crate::demo::install_with_salt(7);
        let mut planning = Planning::new();
        planning
            .set_view(view(
                Some(Cents::from_dollars(17_500)),
                Some(day(2026, 8, 14)),
            ))
            .unwrap();
        let line = planning.pin_line().unwrap();
        assert!(line.starts_with("pinned "));
        assert!(line.contains(" on 2026-08-14 · excess has since moved "));
        assert!(!line.contains("17,500"), "the pin survived: {line}");
        assert!(!line.contains("0.75"), "the drift survived: {line}");
        assert!(
            line.contains(&crate::demo::figure(Cents::from_dollars(17_500))),
            "no scrambled pin found: {line}"
        );
        assert!(
            line.contains(&crate::demo::figure(Cents(75))),
            "no scrambled drift found: {line}"
        );
    }

    /// Import transcribes `Planning!D3` and has no date to transcribe with it,
    /// so an imported pin is dateless and must render rather than fail.
    #[test]
    fn an_imported_pin_with_no_date_still_renders() {
        let planning = screen();
        assert_eq!(
            planning.pin_line().unwrap(),
            "pinned 17,500.00 · excess has since moved 0.75"
        );
    }

    /// The pin is still visible when it has not drifted: `p` is a toggle, and
    /// its state has to be on screen either way.
    #[test]
    fn a_pinned_plan_that_has_not_drifted_still_says_it_is_pinned() {
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents(1_750_075)), Some(day(2026, 8, 14))))
            .unwrap();
        assert_eq!(
            planning.pin_line().unwrap(),
            "pinned 17,500.75 on 2026-08-14"
        );
    }

    #[test]
    fn there_is_no_pin_line_when_the_plan_is_not_pinned() {
        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();
        assert_eq!(planning.pin_line(), None);
        assert!(!planning.is_pinned());
    }
}
