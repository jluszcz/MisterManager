//! The Planning waterfall as an ordered list of rows, in neither medium.
//!
//! A peer of `overview` and `savings`, and it exists for the same reason: two
//! sinks draw this screen -- `tui::planning` and `report::html::planning` --
//! and the sequence they draw is one fact about the app rather than one fact
//! per medium. Transcribing it twice is how the two came to disagree about
//! which blocks have headings at all.
//!
//! What is *here* is the order, the labels, the grouping, the two footers
//! that sit outside the transfers block, and the Biweekly Expenses line under
//! them. What is not is anything either
//! medium decides for itself: a terminal's tint and cursor, a page's classes,
//! and the Destinations block, which only the screen draws because only the
//! screen can edit a destination.
//!
//! [`Row::depth`] is the one place the two mediums genuinely differ and it is
//! carried as a number for that reason: the screen spends two spaces a level
//! and the page spends a `sub{n}` class a level, and neither spelling belongs
//! in a list both of them read.

use crate::account_label::Account;
use crate::calc::planning::{Plan, PlanSettings, per_period};
use crate::db::BillId;
use crate::db::setting::{Key, key};
use crate::gate::Gate;
use crate::money::Cents;
use crate::plan_line::Line;
use crate::rate::Percent;
use crate::transfer;
use chrono::NaiveDate;

/// One editable constant of the waterfall.
///
/// An enum rather than the `Key<T>` being edited, because the constants have
/// different `T` -- `Cents`, `Percent`, `i64` -- and one field cannot hold
/// them all. It sits here rather than beside the screen that edits it for the
/// reason the rows do: which row *is* which constant is a fact about the
/// waterfall, and a sink that paired them itself would be pairing them again.
/// `tui::planning` holds the half that is the screen's -- how each arm's text
/// parses, and where it is written.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Target,
    Buffer,
    PeriodsPerYear,
    BillPaymentCap,
    BillPaymentPct,
    MomAndDadAnnual,
    GoalsFloor,
    FutureHousingPct,
    RetirementPct,
    InvestmentPct,
    PinnedExcess,
    Bill(BillId),
}

impl Target {
    /// The three waterfall constants that may be counted into the Biweekly
    /// Expenses figure, in the order the reader returns them.
    ///
    /// Written out by hand, so a constant added to
    /// [`Target::counts_as_expense_key`] and forgotten here would be
    /// markable by nothing and counted by nothing --
    /// `every_countable_constant_is_in_the_countable_list` is what catches
    /// that.
    pub const COUNTABLE_AS_EXPENSE: [Target; 3] = [
        Target::BillPaymentCap,
        Target::MomAndDadAnnual,
        Target::GoalsFloor,
    ];

    /// The key this constant's biweekly-expense mark is stored under, for the
    /// three constants that may carry one.
    ///
    /// A bill carries its mark as a column, since it has a row of its own to
    /// carry it; a constant does not, so it is a key -- and the pairing lives
    /// on the enum, the way `gate::Gate`'s does, so no call site ever writes
    /// one. `None` is most of the enum: a percentage is a shape rather than a
    /// sum, and Target, Buffer and the pinned excess are cash to hold rather
    /// than money that leaves.
    pub fn counts_as_expense_key(self) -> Option<Key<bool>> {
        match self {
            Target::BillPaymentCap => Some(key::BILL_PAYMENT_CAP_COUNTS_AS_EXPENSE),
            Target::MomAndDadAnnual => Some(key::MOM_AND_DAD_ANNUAL_COUNTS_AS_EXPENSE),
            Target::GoalsFloor => Some(key::GOALS_FLOOR_COUNTS_AS_EXPENSE),
            _ => None,
        }
    }

    /// What this constant contributes to the Biweekly Expenses figure, per
    /// pay period.
    ///
    /// Mom & Dad is the one that is not already a per-paycheck figure: it is
    /// stored as a year's commitment and spent by the waterfall a paycheck at
    /// a time, so counting what is stored would put a year of it into every
    /// pay period. `periods` is clamped by the caller, which is what makes
    /// [`per_period`]'s divide safe.
    fn expense_biweekly(self, settings: &PlanSettings, periods: i64) -> Option<Cents> {
        match self {
            Target::BillPaymentCap => Some(settings.bill_payment_cap),
            Target::GoalsFloor => Some(settings.goals_floor),
            Target::MomAndDadAnnual => Some(per_period(settings.mom_and_dad_annual, periods)),
            _ => None,
        }
    }
}

/// One monthly bill, with the biweekly figure the waterfall actually spends.
pub struct Bill {
    pub id: BillId,
    pub label: String,
    pub monthly: Cents,
    pub biweekly: Cents,
    /// Whether the Biweekly Expenses figure counts this bill --
    /// `db::bill::Bill`'s own column, carried across unchanged.
    pub counts_as_expense: bool,
}

/// One transfer the plan would write: where it lands, and the lines that put
/// it there.
pub struct Transfer {
    /// The account it lands in. `None` is a withdrawal -- money leaving the
    /// tracked system, which is a supported destination and not a failure.
    pub to: Option<Account>,
    /// What the row is headed by when it names no account.
    pub label: String,
    pub cents: Cents,
    /// The lines it carries, as [`Line`] rather than as labels: the gap a
    /// cut line draws is reached through the line's own identity, and
    /// matching a label back to the enum that produced it would be a name
    /// lookup where an identity is already in hand.
    pub lines: Vec<(Line, Cents)>,
}

/// [`transfer::Row`] as a sink reads it: the account resolved, and a
/// withdrawal turned into the destination it is rather than a separate shape.
///
/// One conversion rather than one per sink. `transfer::plan` has the account
/// row in hand when it builds its rows, so the name and color come across
/// with the id and neither sink looks the account up again.
pub fn transfers(rows: &[transfer::Row]) -> Vec<Transfer> {
    rows.iter()
        .map(|row| match row {
            transfer::Row::Transfer {
                to,
                name,
                color,
                cents,
                lines,
            } => Transfer {
                to: Some(Account::held(*to, name.clone(), *color)),
                label: name.clone(),
                cents: *cents,
                lines: lines.clone(),
            },
            transfer::Row::Withdrawal { line, cents } => Transfer {
                to: None,
                label: "Withdrawal".to_string(),
                cents: *cents,
                lines: vec![(*line, *cents)],
            },
        })
        .collect()
}

/// What a row is, as far as either medium's emphasis goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A block title.
    Heading,
    /// A separator between blocks. The screen draws an empty line; the page
    /// draws no row at all and spends the spacing on the row *below* it
    /// instead -- a heading's own padding where one follows, and a rule of
    /// its own where the row is an ordinary one.
    Blank,
    /// An ordinary line.
    Figure,
    /// A figure the rows above it add up to, or a block's own footer.
    Total,
    /// The sentence a block has instead of rows.
    Note,
}

/// A row's label: text, or the account whose name *is* the label.
///
/// An enum rather than a `String` beside an `Option<Account>`, so there is no
/// route to a transfer head's text that skips the color --
/// [`Account::render_with`] is the only reader of it, in either medium.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowLabel {
    Text(String),
    /// A transfer's head. The account its money lands in is the whole of what
    /// the row is called, and each medium tints it its own way.
    Account(Account),
}

/// The middle column: a figure, or one of the two things that are not one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Money(Cents),
    /// A count of pay periods. A number rather than an amount, which is what
    /// keeps it from ever drawing as a negative figure would.
    Count(i64),
    /// A gate's verdict.
    Stated(&'static str),
    None,
}

/// The third column: what the screen keeps beside a figure, and the page
/// keeps a `<td>` for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Extra {
    /// The percentage that produced the figure beside it.
    Percent(Percent),
    /// A monthly bill's biweekly figure -- what the waterfall spends.
    Biweekly(Cents),
    /// What the excess cut, drawn as a `\u{394}` in the red both sinks keep
    /// for a gap.
    Gap(Cents),
    /// The ad-hoc date a scrub moved `Excess (Actual)` to. Only the screen
    /// ever carries one: the report quotes the canonical dates, so a
    /// hypothetical balance is a state it does not have.
    Date(NaiveDate),
    /// The same money over a year of paychecks, beside the Biweekly Expenses
    /// figure it is derived from.
    Annual(Cents),
    None,
}

/// One row of the waterfall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub label: RowLabel,
    pub value: Value,
    pub extra: Extra,
    /// How far under its block's heading this row sits. A heading is `0`, a
    /// block's own lines are `1`, and a line belonging to the one above it is
    /// `2`.
    pub depth: u8,
    /// The constant this row *is*, for the medium that lets the owner edit
    /// one. At most one per row, so a key that edits is never ambiguous about
    /// what it edits.
    pub target: Option<Target>,
    /// What an editor prefills its field with: the stored value, unformatted
    /// for a percentage and with its cents for an amount.
    ///
    /// Deliberately not what the row *displays*. Both sinks floor every
    /// figure to a whole dollar, and prefilling the floored text would make
    /// opening an editor on a setting and pressing Enter silently drop its
    /// cents. Paired with `target` here, where the stored figure is in hand,
    /// rather than reconstructed from the two columns by whichever sink
    /// edits.
    pub edit: String,
    /// Whether the Biweekly Expenses figure counts this row.
    ///
    /// A flag rather than a glyph, for the reason [`Row::depth`] is a number:
    /// the screen spends a character in its label column and the page spends
    /// a class, and neither spelling belongs in a list both of them read.
    pub counts_as_expense: bool,
}

impl Row {
    fn new(kind: Kind, label: &str, value: Value, depth: u8) -> Row {
        Row {
            kind,
            label: RowLabel::Text(label.to_string()),
            value,
            extra: Extra::None,
            depth,
            target: None,
            edit: String::new(),
            counts_as_expense: false,
        }
    }

    /// The same row, saying whether the Expenses figure counts it.
    fn counted(self, counts_as_expense: bool) -> Row {
        Row {
            counts_as_expense,
            ..self
        }
    }

    fn blank() -> Row {
        Row::new(Kind::Blank, "", Value::None, 0)
    }

    fn heading(label: &str) -> Row {
        Row::new(Kind::Heading, label, Value::None, 0)
    }

    fn note(message: &str) -> Row {
        Row::new(Kind::Note, message, Value::None, 1)
    }

    fn figure(label: &str, value: Cents, depth: u8) -> Row {
        Row::new(Kind::Figure, label, Value::Money(value), depth)
    }

    /// One bill of the block, at the depth its category puts it.
    fn bill(b: &Bill, depth: u8) -> Row {
        Row {
            extra: Extra::Biweekly(b.biweekly),
            target: Some(Target::Bill(b.id)),
            edit: b.monthly.to_string(),
            counts_as_expense: b.counts_as_expense,
            ..Row::figure(&b.label, b.monthly, depth)
        }
    }

    fn total(label: &str, value: Cents, depth: u8) -> Row {
        Row::new(Kind::Total, label, Value::Money(value), depth)
    }

    /// A figure the owner may retype. The prefill is the stored figure, not
    /// the floored one the row shows.
    fn money(label: &str, value: Cents, target: Target, depth: u8) -> Row {
        Row {
            target: Some(target),
            edit: value.to_string(),
            ..Row::figure(label, value, depth)
        }
    }

    fn count(label: &str, value: i64, target: Target, depth: u8) -> Row {
        Row {
            target: Some(target),
            edit: value.to_string(),
            ..Row::new(Kind::Figure, label, Value::Count(value), depth)
        }
    }

    /// A figure the waterfall computed, with the percentage that produced it
    /// beside it. Editing the row edits the percentage, which is why the
    /// prefill is the share rather than the amount.
    fn split(label: &str, value: Cents, pct: Percent, target: Option<Target>, depth: u8) -> Row {
        Row {
            extra: Extra::Percent(pct),
            target,
            edit: pct.0.to_string(),
            ..Row::figure(label, value, depth)
        }
    }

    /// A gap the block foots with rather than hangs off a line: each reports
    /// a payday with no transfer row to hang a cell off at all.
    fn footer(label: &str, gap: Cents) -> Row {
        Row {
            extra: Extra::Gap(gap),
            ..Row::new(Kind::Total, label, Value::None, 1)
        }
    }
}

/// Everything the list is built from, in the shape both sinks can fill.
pub struct Input<'a> {
    pub plan: &'a Plan,
    pub settings: &'a PlanSettings,
    pub housing: &'a [Bill],
    pub other_bills: &'a [Bill],
    /// The transfers the payday would write, or why they cannot be resolved.
    /// A misconfigured destination stops the money moving without making one
    /// figure below it wrong, so the failure is a block's content rather than
    /// the list's.
    pub transfers: Result<&'a [Transfer], &'a str>,
    /// What the goals the plug spreads over ask of this paycheck, summed --
    /// `transfer::spread_asks`, read on its own by each sink and never
    /// chained to the transfers above.
    pub spread_ask_total: Cents,
    /// The ad-hoc date the plan was computed at, when a scrub has moved it
    /// off the derived one.
    pub scrubbed_adhoc: Option<NaiveDate>,
    /// The waterfall constants the owner has marked as biweekly expenses.
    ///
    /// The bills carry their own mark on [`Bill`]; a constant has no row in
    /// any table, so its mark is a setting and arrives here as a list of the
    /// ones that are set. A `Target` in it that cannot be counted --
    /// [`Target::counts_as_expense_key`] says which can -- is neither marked
    /// nor counted, so the two halves cannot come apart.
    pub expense_constants: &'a [Target],
}

/// The transfers a payday would write, then `Planning!C1:G41` top to bottom
/// under them.
///
/// The transfers come first because they are the answer -- the money that
/// actually moves -- and every block below is the working behind it: an owner
/// who trusts the plan reads the top and presses `t`, and one who doubts a
/// figure reads down to the line that made it.
pub fn rows(input: &Input) -> Vec<Row> {
    let p = input.plan;
    let s = input.settings;

    // Never divide by a user-supplied zero, the same clamp `compute` makes
    // over the same setting.
    let periods = s.periods_per_year.max(1);
    let marked = |t: Target| input.expense_constants.contains(&t);

    let mut rows = vec![Row::heading("Transfers")];
    match input.transfers {
        Err(message) => rows.push(Row::note(message)),
        Ok(transfers) => {
            for t in transfers {
                rows.push(Row {
                    label: match &t.to {
                        Some(a) => RowLabel::Account(a.clone()),
                        None => RowLabel::Text(t.label.clone()),
                    },
                    ..Row::total(&t.label, t.cents, 1)
                });
                for (line, cents) in &t.lines {
                    // What the excess cut from this line, in the one cell
                    // such a line has spare -- and nothing where the line
                    // leaves as a withdrawal, because a withdrawal is one
                    // line under a head repeating its own figure. What it
                    // lost is in the `Shortfall` row below either way,
                    // stated once for the whole plan.
                    let cut = match t.to.is_some() {
                        true => line.amount(&p.shortfall),
                        false => Cents::ZERO,
                    };
                    rows.push(Row {
                        extra: match cut > Cents::ZERO {
                            true => Extra::Gap(cut),
                            false => Extra::None,
                        },
                        ..Row::figure(line.label(), *cents, 2)
                    });
                }
            }
        }
    }
    // Both footers sit outside the match on purpose: each reports a payday
    // with no transfer row to hang a per-line cell off -- a plug of nothing
    // for the first, an excess the fixed bills took whole for the second --
    // and a gap drawn only inside the block those states empty would go
    // silent exactly where it is worth most.
    if let Some(gap) = transfer::unmet_asks(p.lines.goals, input.spread_ask_total) {
        rows.push(Row::footer("Unmet Asks", gap));
    }
    if p.shortfall.total() > Cents::ZERO {
        rows.push(Row::footer("Shortfall", p.shortfall.total()));
    }

    // What a pay period costs, by the owner's own reckoning. It sits here
    // for the reason the transfers sit above it: both are answers rather
    // than working, and the blocks below are what produced them -- an owner
    // who wants the figure should not have to read the whole waterfall to
    // reach it, even though every input it counts is down there.
    //
    // One line rather than a block, standing at the depth a heading would
    // because there is nothing beneath it to head. Drawn only when something
    // is marked, the way the two footers above it are: a figure nobody has
    // chosen the inputs to means nothing, and a row reading zero on every
    // unconfigured database is a row nobody reads.
    if let Some(counted) = counted_biweekly(input, periods) {
        // The year is the *rounded* figure over a year of paychecks rather
        // than the raw one rounded again. The two sit on one line, so a
        // reader who multiplies the first by the cadence has to land on the
        // second.
        //
        // Saturating, for the reason the divisor two lines up is clamped:
        // the cadence is user-editable and `parse_periods` bounds it only
        // below, so a fat-fingered count reaches this multiply with nothing
        // in between. Wrapped, it would draw a garbage year beside a correct
        // paycheck figure.
        let biweekly = counted.ceil_to_hundred_dollars();
        rows.push(Row::blank());
        rows.push(Row {
            extra: Extra::Annual(Cents(biweekly.0.saturating_mul(periods))),
            ..Row::total("Expenses", biweekly, 0)
        });
    }

    rows.push(Row::blank());
    rows.push(Row::heading("Excess"));
    rows.push(Row::money("Target", s.target, Target::Target, 1));
    rows.push(Row::money("Buffer", s.buffer, Target::Buffer, 1));
    rows.push(Row::count(
        "Pay Periods / Year",
        s.periods_per_year,
        Target::PeriodsPerYear,
        1,
    ));
    rows.push(Row::blank());
    // The one row a scrub moves, and it names the date rather than the
    // drift: a screen quoting a hypothetical balance must say which day it
    // is quoting, and this block has no column header to hang one off.
    rows.push(Row {
        extra: match input.scrubbed_adhoc {
            Some(date) => Extra::Date(date),
            None => Extra::None,
        },
        ..Row::figure("Excess (Actual)", p.excess_actual, 1)
    });
    // Editable, and typing a figure here pins it: this is the sheet's
    // hand-typed `Excess (Fixed)` cell, which the pin key only ever fills
    // with the floored actual. The prefill is `excess_used` in both states,
    // because that is the pin when there is one and the figure a first pin
    // would freeze when there is not.
    rows.push(Row {
        target: Some(Target::PinnedExcess),
        edit: p.excess_used.to_string(),
        ..Row::total("Excess (Used)", p.excess_used, 1)
    });

    rows.push(Row::blank());
    rows.push(Row::heading("Bills"));
    // `Planning!C6` -- the housing subtotal, and the only bill line that is
    // not a bill. Its biweekly figure is `E6`, the one `lines.current_housing`
    // spends, so it is the figure the waterfall answers for rather than a sum
    // of the rows beneath it.
    rows.push(Row {
        extra: Extra::Biweekly(p.housing_biweekly),
        ..Row::figure(
            "Mortgage + HOA",
            input.housing.iter().map(|b| b.monthly).sum(),
            1,
        )
    });
    for b in input.housing {
        rows.push(Row::bill(b, 2));
    }
    // A bill outside housing is the block's own line and a peer of the
    // subtotal, not one of the rows it sums: `Mortgage + HOA` adds up
    // `input.housing` alone, and a bill drawn one level under a figure that
    // does not count it reads as an omission from that figure.
    for b in input.other_bills {
        rows.push(Row::bill(b, 1));
    }
    rows.push(Row::total("Remaining Excess", p.remaining_excess, 1));

    rows.push(Row::blank());
    rows.push(Row::heading("Gates"));
    for (gate, needed) in [
        (Gate::EmergencyFund, p.need_emergency),
        (Gate::Roth, p.need_roth),
    ] {
        rows.push(Row::new(
            Kind::Figure,
            gate.label(),
            Value::Stated(if needed { "needed" } else { "met" }),
            1,
        ));
    }

    rows.push(Row::blank());
    rows.push(Row::heading("Waterfall"));
    rows.push(Row::split(
        "Bill Payments",
        p.bill_payments,
        s.bill_payment_pct,
        Some(Target::BillPaymentPct),
        1,
    ));
    rows.push(
        Row::money("Cap", s.bill_payment_cap, Target::BillPaymentCap, 2)
            .counted(marked(Target::BillPaymentCap)),
    );
    rows.push(Row::figure("Mom & Dad", p.mom_and_dad, 1));
    rows.push(
        Row::money("Annual", s.mom_and_dad_annual, Target::MomAndDadAnnual, 2)
            .counted(marked(Target::MomAndDadAnnual)),
    );
    rows.push(Row::total("Remainder", p.remainder, 1));
    rows.push(
        Row::money("Goals Floor", s.goals_floor, Target::GoalsFloor, 2)
            .counted(marked(Target::GoalsFloor)),
    );

    rows.push(Row::blank());
    rows.push(Row::heading("Split"));
    for (line, cents, pct, target) in [
        (
            Line::FutureHousing,
            p.future_housing,
            s.future_housing_pct,
            Some(Target::FutureHousingPct),
        ),
        (
            Line::Retirement,
            p.retirement,
            s.retirement_pct,
            Some(Target::RetirementPct),
        ),
        (
            Line::Investment,
            p.investment,
            s.investment_pct,
            Some(Target::InvestmentPct),
        ),
        // The plug takes what the other three leave, so there is no share of
        // its own to type.
        (Line::Goals, p.goals, s.goals_pct(), None),
    ] {
        rows.push(Row::split(line.label(), cents, pct, target, 1));
    }

    rows
}

/// What a pay period has to carry, by the owner's own reckoning, before the
/// line rounds it -- or `None` when nothing is marked at all.
///
/// Each side reaches its own per-period figure before being added, since a
/// bill is stored by the month and two of the three constants are already per
/// paycheck. `periods` is clamped by the caller.
fn counted_biweekly(input: &Input, periods: i64) -> Option<Cents> {
    let counted: Vec<Cents> = input
        .housing
        .iter()
        .chain(input.other_bills)
        .filter(|b| b.counts_as_expense)
        .map(|b| b.biweekly)
        .chain(
            input
                .expense_constants
                .iter()
                .filter_map(|t| t.expense_biweekly(input.settings, periods)),
        )
        .collect();
    match counted.is_empty() {
        true => None,
        false => Some(counted.into_iter().sum()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::planning::{PlanInputs, compute};
    use crate::db::AccountId;

    fn settings() -> PlanSettings {
        let d = Cents::from_dollars;
        PlanSettings {
            target: d(10_000),
            buffer: d(5_000),
            periods_per_year: 26,
            bill_payment_cap: d(2_000),
            bill_payment_pct: Percent(50),
            mom_and_dad_annual: d(12_000),
            goals_floor: d(500),
            future_housing_pct: Percent(35),
            retirement_pct: Percent(15),
            investment_pct: Percent(15),
        }
    }

    fn bill(id: i64, label: &str, dollars: i64) -> Bill {
        Bill {
            id: BillId(id),
            label: label.to_string(),
            monthly: Cents::from_dollars(dollars),
            biweekly: Cents::from_dollars(dollars * 12 / 26),
            counts_as_expense: false,
        }
    }

    /// The fixture with a set of bills and a set of constants marked as
    /// biweekly expenses, read however the caller wants it -- as the rows it
    /// produces, or as the sum behind them before the row rounds it.
    fn with_marks<T>(bills: &[i64], constants: &[Target], read: impl FnOnce(&Input) -> T) -> T {
        let plan = plan();
        let settings = settings();
        let mark = |mut b: Bill| {
            b.counts_as_expense = bills.contains(&b.id.0);
            b
        };
        let housing: Vec<Bill> = [bill(1, "Mortgage", 1_200), bill(2, "HOA", 300)]
            .into_iter()
            .map(mark)
            .collect();
        let other: Vec<Bill> = [bill(3, "Plumber", 90)].into_iter().map(mark).collect();
        read(&Input {
            plan: &plan,
            settings: &settings,
            housing: &housing,
            other_bills: &other,
            transfers: Ok(&[]),
            spread_ask_total: Cents::ZERO,
            scrubbed_adhoc: None,
            expense_constants: constants,
        })
    }

    fn plan() -> Plan {
        compute(
            &settings(),
            &PlanInputs {
                checking_at_adhoc: Cents(3_250_075),
                pinned_excess: None,
                housing_monthly: vec![Cents::from_dollars(1_200), Cents::from_dollars(300)],
                other_bills_monthly: vec![Cents::from_dollars(90)],
                remaining_emergency: Cents::ZERO,
                remaining_roth: Cents::ZERO,
            },
        )
        .unwrap()
    }

    /// The waterfall over the fixture above, with one of its inputs moved.
    fn built(transfers: &[Transfer], edit: impl FnOnce(&mut Input)) -> Vec<Row> {
        let plan = plan();
        let settings = settings();
        let housing = vec![bill(1, "Mortgage", 1_200), bill(2, "HOA", 300)];
        let other = vec![bill(3, "Plumber", 90)];
        let mut input = Input {
            plan: &plan,
            settings: &settings,
            housing: &housing,
            other_bills: &other,
            transfers: Ok(transfers),
            spread_ask_total: Cents::ZERO,
            scrubbed_adhoc: None,
            expense_constants: &[],
        };
        edit(&mut input);
        rows(&input)
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match &r.label {
                RowLabel::Text(t) => t.clone(),
                RowLabel::Account(a) => a.text().to_string(),
            })
            .collect()
    }

    fn at(rows: &[Row], label: &str) -> Row {
        rows.iter()
            .find(|r| matches!(&r.label, RowLabel::Text(t) if t == label))
            .unwrap_or_else(|| panic!("no {label} row"))
            .clone()
    }

    /// Every block has a heading, and they come in the order the waterfall
    /// runs: what moves, then the excess behind it, then what is taken out of
    /// that excess in the order it is taken.
    #[test]
    fn every_block_of_the_waterfall_is_headed_in_the_order_it_runs() {
        let rows = built(&[], |_| {});
        let headings: Vec<String> = rows
            .iter()
            .filter(|r| r.kind == Kind::Heading)
            .map(|r| match &r.label {
                RowLabel::Text(t) => t.clone(),
                RowLabel::Account(_) => unreachable!("a heading names no account"),
            })
            .collect();

        assert_eq!(
            headings,
            [
                "Transfers",
                "Excess",
                "Bills",
                "Gates",
                "Waterfall",
                "Split"
            ]
        );
    }

    /// A block's own lines sit one level under its heading, and a line that
    /// belongs to the line above it sits one further. The two mediums spell
    /// that differently and neither may decide it.
    #[test]
    fn a_line_belonging_to_the_line_above_it_sits_one_level_deeper() {
        let rows = built(&[], |_| {});

        assert_eq!(at(&rows, "Bill Payments").depth, 1);
        assert_eq!(at(&rows, "Cap").depth, 2);
        assert_eq!(at(&rows, "Mortgage + HOA").depth, 1);
        assert_eq!(at(&rows, "Mortgage").depth, 2);
        assert_eq!(at(&rows, "Remaining Excess").depth, 1);
    }

    /// `Mortgage + HOA` sums the housing bills and no others, so only those
    /// sit under it. A bill outside housing indented there would invite the
    /// reader to add it into a subtotal that does not carry it -- and both
    /// mediums spend the depth literally, with nothing between the two halves
    /// of the block to say where one ends.
    #[test]
    fn a_bill_outside_housing_is_not_drawn_under_the_housing_subtotal() {
        let rows = built(&[], |_| {});

        assert_eq!(at(&rows, "HOA").depth, 2);
        assert_eq!(at(&rows, "Plumber").depth, 1);
    }

    /// `Planning!C6` is the housing subtotal, and it is a sum of the rows
    /// beneath it rather than a bill of its own -- which is why it carries no
    /// `Target` and cannot be typed into.
    #[test]
    fn the_housing_subtotal_is_the_sum_of_its_bills_and_edits_none_of_them() {
        let rows = built(&[], |_| {});
        let housing = at(&rows, "Mortgage + HOA");

        assert_eq!(housing.value, Value::Money(Cents::from_dollars(1_500)));
        assert_eq!(housing.target, None);
        assert_eq!(at(&rows, "Mortgage").target, Some(Target::Bill(BillId(1))));
    }

    /// The prefill is the stored figure, never the floored one a sink shows:
    /// opening an editor and pressing Enter must not drop a setting's cents.
    #[test]
    fn an_editable_row_prefills_the_stored_figure_rather_than_the_drawn_one() {
        let rows = built(&[], |_| {});

        assert_eq!(
            at(&rows, "Target").edit,
            Cents::from_dollars(10_000).to_string()
        );
        // A split is typed as its share, not as the amount it produced.
        assert_eq!(at(&rows, "Bill Payments").edit, "50");
        assert_eq!(
            at(&rows, "Bill Payments").extra,
            Extra::Percent(Percent(50))
        );
    }

    /// The plug takes what the other three shares leave, so there is nothing
    /// to type on its row -- and it still carries the share it came to, which
    /// is what makes the block foot to 100.
    #[test]
    fn the_goals_plug_shows_its_share_and_offers_nothing_to_edit() {
        let rows = built(&[], |_| {});
        let goals = at(&rows, Line::Goals.label());

        assert_eq!(goals.target, None);
        assert_eq!(goals.extra, Extra::Percent(Percent(35)));
    }

    /// A transfer's head names the account through [`Account`] rather than as
    /// text, so neither sink can draw the name without the color.
    #[test]
    fn a_transfer_head_names_its_account_and_a_withdrawal_names_none() {
        let transfers = vec![
            Transfer {
                to: Some(Account::held(AccountId(1), "Rainy Day", None)),
                label: "Rainy Day".to_string(),
                cents: Cents::from_dollars(400),
                lines: vec![(Line::Goals, Cents::from_dollars(400))],
            },
            Transfer {
                to: None,
                label: "Withdrawal".to_string(),
                cents: Cents::from_dollars(100),
                lines: vec![(Line::Retirement, Cents::from_dollars(100))],
            },
        ];
        let rows = built(&transfers, |_| {});

        assert!(matches!(rows[1].label, RowLabel::Account(_)));
        assert!(matches!(&rows[3].label, RowLabel::Text(t) if t == "Withdrawal"));
        assert_eq!(
            labels(&rows)[..5],
            [
                "Transfers",
                "Rainy Day",
                Line::Goals.label(),
                "Withdrawal",
                Line::Retirement.label()
            ]
        );
    }

    /// A line that leaves as a withdrawal draws no gap beside itself: it is
    /// one line under a head repeating its own figure, and what the excess
    /// cut from it is in the `Shortfall` footer either way.
    #[test]
    fn only_a_line_landing_in_an_account_carries_its_own_gap() {
        let mut plan = plan();
        plan.shortfall.bills = Cents::from_dollars(237);
        plan.shortfall.retirement = Cents::from_dollars(11);
        let settings = settings();
        let transfers = vec![
            Transfer {
                to: Some(Account::held(AccountId(1), "Rainy Day", None)),
                label: "Rainy Day".to_string(),
                cents: Cents::from_dollars(400),
                lines: vec![(Line::Bills, Cents::from_dollars(400))],
            },
            Transfer {
                to: None,
                label: "Withdrawal".to_string(),
                cents: Cents::from_dollars(100),
                lines: vec![(Line::Retirement, Cents::from_dollars(100))],
            },
        ];
        let rows = rows(&Input {
            plan: &plan,
            settings: &settings,
            housing: &[],
            other_bills: &[],
            transfers: Ok(&transfers),
            spread_ask_total: Cents::ZERO,
            scrubbed_adhoc: None,
            expense_constants: &[],
        });

        assert_eq!(rows[2].extra, Extra::Gap(Cents::from_dollars(237)));
        assert_eq!(rows[4].extra, Extra::None);
        assert_eq!(
            at(&rows, "Shortfall").extra,
            Extra::Gap(Cents::from_dollars(248))
        );
    }

    /// Both footers sit outside the transfers block, so the payday with no
    /// transfer row at all is still the one that reports them.
    #[test]
    fn the_two_footers_are_drawn_over_a_block_that_could_not_resolve() {
        let mut plan = plan();
        plan.shortfall.bills = Cents::from_dollars(237);
        plan.lines.goals = Cents::ZERO;
        let settings = settings();
        let rows = rows(&Input {
            plan: &plan,
            settings: &settings,
            housing: &[],
            other_bills: &[],
            transfers: Err(crate::transfer::NOTHING_TO_TRANSFER),
            spread_ask_total: Cents::from_dollars(220),
            scrubbed_adhoc: None,
            expense_constants: &[],
        });

        assert_eq!(rows[1].kind, Kind::Note);
        assert_eq!(
            at(&rows, "Unmet Asks").extra,
            Extra::Gap(Cents::from_dollars(-220))
        );
        assert_eq!(
            at(&rows, "Shortfall").extra,
            Extra::Gap(Cents::from_dollars(237))
        );
    }

    /// A plan that covers everything says nothing below itself: a footer that
    /// reads "nothing is wrong" on every ordinary payday is a footer nobody
    /// reads.
    #[test]
    fn a_plan_that_covers_everything_foots_with_neither_gap() {
        let rows = built(&[], |_| {});
        assert!(!rows.iter().any(|r| matches!(r.extra, Extra::Gap(_))));
    }

    /// Only `Excess (Actual)` moves with a scrub, and it names the date it
    /// was quoted at.
    #[test]
    fn a_scrubbed_plan_names_its_date_on_the_one_row_a_scrub_moves() {
        let date = crate::test_support::day(2026, 8, 29);
        let rows = built(&[], |input| input.scrubbed_adhoc = Some(date));

        assert_eq!(at(&rows, "Excess (Actual)").extra, Extra::Date(date));
        assert_eq!(at(&rows, "Excess (Used)").extra, Extra::None);
    }

    /// A figure nobody has chosen the inputs to is a figure that means
    /// nothing, and a row reading `0` on every database that has not been
    /// configured is a row nobody reads -- the same reason the two transfer
    /// footers are drawn only when they have something to report.
    #[test]
    fn a_database_with_nothing_marked_draws_no_expenses_line() {
        assert_eq!(with_marks(&[], &[], |i| counted_biweekly(i, 26)), None);
        let rows = with_marks(&[], &[], rows);
        assert!(
            !rows
                .iter()
                .any(|r| matches!(&r.label, RowLabel::Text(t) if t == "Expenses"))
        );
    }

    /// The figure spans two blocks of the waterfall -- a monthly bill and a
    /// per-paycheck constant -- so it can only sit under both, and each side
    /// arrives at its own per-period figure before being added.
    #[test]
    fn the_expenses_line_adds_up_every_marked_bill_and_constant() {
        // 1,200/mo and 90/mo at 26 periods, plus the two constants as they
        // stand: 553 + 41 + 2,000 + 500.
        let counted = with_marks(
            &[1, 3],
            &[Target::BillPaymentCap, Target::GoalsFloor],
            |i| counted_biweekly(i, 26),
        );

        assert_eq!(counted, Some(Cents::from_dollars(3_094)));
    }

    /// Mom & Dad is stored as a year's commitment and spent by the waterfall
    /// a paycheck at a time. Counting the stored figure would put a year of
    /// it into every pay period.
    #[test]
    fn a_marked_mom_and_dad_counts_its_share_of_a_paycheck_not_the_year() {
        let counted = with_marks(&[], &[Target::MomAndDadAnnual], |i| counted_biweekly(i, 26));

        assert_eq!(counted, Some(Cents(46_153)), "12,000 a year, 26 paychecks");
    }

    /// A plan rather than a measurement: the figure rounds up to a hundred,
    /// which is the unit somebody actually budgets in, and a pay period
    /// budgeted short is the failure worth avoiding.
    ///
    /// The year is that *rounded* figure over a year of paychecks, not the
    /// raw one rounded a second time. The two sit on one line, so a reader
    /// who multiplies the first by the cadence has to land on the second --
    /// rounding each from the raw sum would print 4,300 beside a year that
    /// 4,300 does not produce.
    #[test]
    fn the_expenses_line_rounds_up_to_a_hundred_and_the_year_follows_it() {
        let raw = with_marks(&[1], &[], |i| counted_biweekly(i, 26)).unwrap();
        let rows = with_marks(&[1], &[], rows);
        let line = at(&rows, "Expenses");

        assert_eq!(raw, Cents::from_dollars(553));
        assert_eq!(line.value, Value::Money(Cents::from_dollars(600)));
        assert_eq!(line.extra, Extra::Annual(Cents::from_dollars(600 * 26)));
    }

    /// A reader has to be able to see which rows made the figure, or the
    /// total is a number with no working behind it. Both mediums spell the
    /// mark their own way, so the row carries it as a flag.
    #[test]
    fn only_the_rows_the_figure_counts_are_marked() {
        let rows = with_marks(&[1, 3], &[Target::GoalsFloor], rows);

        assert!(at(&rows, "Mortgage").counts_as_expense);
        assert!(at(&rows, "Plumber").counts_as_expense);
        assert!(at(&rows, "Goals Floor").counts_as_expense);
        assert!(!at(&rows, "HOA").counts_as_expense);
        assert!(!at(&rows, "Cap").counts_as_expense);
        assert!(!at(&rows, "Target").counts_as_expense);
    }

    /// It reads as an answer rather than as working, so it sits under the
    /// transfers rather than under the blocks whose figures it counts: an
    /// owner who wants to know what a pay period costs reads it without
    /// scrolling the whole waterfall first.
    ///
    /// One line rather than a block, standing at the depth a heading would
    /// because there is nothing under it to head -- and a blank is all that
    /// separates it from the Excess block, so it belongs to neither.
    #[test]
    fn the_expenses_line_sits_between_the_transfers_and_the_excess() {
        let rows = with_marks(&[1], &[], rows);
        let labels = labels(&rows);
        let expenses = labels.iter().position(|l| l == "Expenses").unwrap();
        let excess = labels.iter().position(|l| l == "Excess").unwrap();

        assert_eq!(rows[expenses].kind, Kind::Total);
        assert_eq!(rows[expenses].depth, 0);
        assert_eq!(rows[expenses - 1].kind, Kind::Blank);
        assert_eq!(rows[expenses + 1].kind, Kind::Blank);
        assert_eq!(excess, expenses + 2, "{labels:?}");
        assert!(
            labels[..expenses].contains(&"Transfers".to_string()),
            "{labels:?}"
        );
        assert!(
            !rows.iter().any(|r| r.kind == Kind::Heading
                && matches!(&r.label, RowLabel::Text(t) if t == "Expenses"))
        );
    }

    /// `COUNTABLE_AS_EXPENSE` is written out by hand and is what the reader
    /// walks, so a constant missing from it is one the owner can never mark
    /// however many keys it owns.
    #[test]
    fn every_countable_constant_is_in_the_countable_list() {
        for target in [
            Target::Target,
            Target::Buffer,
            Target::PeriodsPerYear,
            Target::BillPaymentCap,
            Target::BillPaymentPct,
            Target::MomAndDadAnnual,
            Target::GoalsFloor,
            Target::FutureHousingPct,
            Target::RetirementPct,
            Target::InvestmentPct,
            Target::PinnedExcess,
            Target::Bill(BillId(1)),
        ] {
            assert_eq!(
                Target::COUNTABLE_AS_EXPENSE.contains(&target),
                target.counts_as_expense_key().is_some(),
                "{target:?}"
            );
        }
    }

    /// Two matches on `self`, so a constant could take one key and another's
    /// figure -- a constant marked under a key that adds up a different
    /// constant, which is the mispairing `counts_as_expense_key` living on
    /// the enum exists to prevent. The set is the pairing.
    #[test]
    fn a_constant_is_countable_under_a_key_exactly_when_it_has_a_figure() {
        let settings = settings();
        for target in [
            Target::Target,
            Target::Buffer,
            Target::PeriodsPerYear,
            Target::BillPaymentCap,
            Target::BillPaymentPct,
            Target::MomAndDadAnnual,
            Target::GoalsFloor,
            Target::FutureHousingPct,
            Target::RetirementPct,
            Target::InvestmentPct,
            Target::PinnedExcess,
            Target::Bill(BillId(1)),
        ] {
            assert_eq!(
                target.counts_as_expense_key().is_some(),
                target.expense_biweekly(&settings, 26).is_some(),
                "{target:?}"
            );
        }
    }

    /// The pay cadence is user-editable and `parse_periods` bounds it only
    /// below, so a fat-fingered count reaches this multiply with nothing in
    /// between to stop it. Wrapped, the `/yr` cell draws a garbage figure --
    /// possibly a negative one -- beside a biweekly total that is correct; in
    /// a debug build it panics mid-frame and takes the screen down.
    ///
    /// The divisor on this same line is already clamped, which is the crate's
    /// policy for a user-editable setting that reaches arithmetic. This is
    /// the other half of it.
    #[test]
    fn a_nonsense_pay_cadence_saturates_the_year_rather_than_overflowing() {
        let plan = plan();
        let settings = PlanSettings {
            periods_per_year: i64::MAX,
            ..settings()
        };
        let housing = vec![Bill {
            counts_as_expense: true,
            ..bill(1, "Mortgage", 1_200)
        }];
        let rows = rows(&Input {
            plan: &plan,
            settings: &settings,
            housing: &housing,
            other_bills: &[],
            transfers: Ok(&[]),
            spread_ask_total: Cents::ZERO,
            scrubbed_adhoc: None,
            expense_constants: &[],
        });

        assert_eq!(at(&rows, "Expenses").extra, Extra::Annual(Cents(i64::MAX)));
    }

    /// A count is a number rather than an amount, so no sink can decide it is
    /// a figure below zero.
    #[test]
    fn the_pay_period_count_is_never_money() {
        let rows = built(&[], |_| {});
        assert_eq!(at(&rows, "Pay Periods / Year").value, Value::Count(26));
    }
}
