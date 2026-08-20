//! The Planning screen: `Planning!C1:G41` as a flat list of rows.
//!
//! View state only -- no ratatui above the render functions at the bottom, and
//! no `Db` on the type. `App` runs the queries and hands a [`View`] in.

use super::Label;
use super::cursor::{Cursor, Scroll};
use super::form::{Field, FormFields, next_in, parse_amount, parse_date, step_index};
use super::style::Tone;
use crate::calc;
use crate::calc::planning::{Plan, PlanSettings};
use crate::db::account::AccountColor;
use crate::db::bill::{self, Bill};
use crate::db::setting::{self, key};
use crate::db::{AccountId, BillId, Db};
use crate::gate::Gate;
use crate::money::Cents;
use crate::plan_line::{Destination, Line};
use crate::rate::Percent;
use crate::transfer::{self, Container, Landing, Wiring};
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;

/// One editable constant on the screen.
///
/// An enum rather than a field holding the `Key<T>` being edited, because the
/// constants have different `T` -- `Cents`, `Percent`, `i64` -- and a struct
/// field cannot. Each arm owns both its key and how its text parses, which is
/// the same construction `gate::Gate` uses for its key and its goal-name
/// substring, and it earns its keep for the same reason: a key written at a
/// call site reads as "not configured" when mistyped, and every reader here
/// has a fallback that would hide it.
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
    Bill(BillId),
}

/// A whole-percent share, with a trailing sign tolerated.
///
/// `Percent` is whole percent, so a fraction is refused rather than rounded:
/// `0.35` accepted as `Percent(0)` would silently reroute every discretionary
/// dollar, and accepted as `Percent(35)` would make `35` and `0.35` mean the
/// same thing.
///
/// Bounded to `0..=100`: `Percent::of` does not clamp, so an unbounded value
/// would write a negative or over-100 allocation straight into the waterfall
/// with no error at any layer. The bound is per-field, not a sum check --
/// `compute` already saturates the Goals plug at zero when the other three
/// shares total over 100.
fn parse_percent(raw: &str) -> Result<Percent> {
    let text = raw.trim().trim_end_matches('%').trim();
    let value: i64 = text
        .parse()
        .with_context(|| format!("not a whole percentage: {text:?}"))?;
    ensure!(
        (0..=100).contains(&value),
        "percentage must be between 0 and 100, got {text}"
    );
    Ok(Percent(value))
}

/// A count of pay periods.
///
/// Refused at zero or below, with the text in the message. The `.max(1)`
/// clamps in `calc` stay as the backstop for a database that already holds a
/// nonsense value; this is the half that tells the user.
fn parse_periods(raw: &str) -> Result<i64> {
    let text = raw.trim();
    let value: i64 = text
        .parse()
        .with_context(|| format!("not a whole number of pay periods: {text:?}"))?;
    ensure!(
        value > 0,
        "pay periods per year must be positive, got {text}"
    );
    Ok(value)
}

impl Target {
    /// Parse `raw` as this constant expects it and store it.
    ///
    /// The only place a Planning constant is written.
    pub fn write(self, db: &Db, raw: &str) -> Result<()> {
        match self {
            Target::Target => setting::set(db, key::PLANNING_TARGET, parse_amount(raw)?),
            Target::Buffer => setting::set(db, key::PLANNING_BUFFER, parse_amount(raw)?),
            Target::PeriodsPerYear => {
                setting::set(db, key::PAY_PERIODS_PER_YEAR, parse_periods(raw)?)
            }
            Target::BillPaymentCap => setting::set(db, key::BILL_PAYMENT_CAP, parse_amount(raw)?),
            Target::BillPaymentPct => setting::set(db, key::BILL_PAYMENT_PCT, parse_percent(raw)?),
            Target::MomAndDadAnnual => {
                setting::set(db, key::MOM_AND_DAD_ANNUAL, parse_amount(raw)?)
            }
            Target::GoalsFloor => setting::set(db, key::GOALS_FLOOR, parse_amount(raw)?),
            Target::FutureHousingPct => {
                setting::set(db, key::SPLIT_FUTURE_HOUSING_PCT, parse_percent(raw)?)
            }
            Target::RetirementPct => {
                setting::set(db, key::SPLIT_RETIREMENT_PCT, parse_percent(raw)?)
            }
            Target::InvestmentPct => {
                setting::set(db, key::SPLIT_INVESTMENT_PCT, parse_percent(raw)?)
            }
            Target::Bill(id) => bill::set_amount(db, id, parse_amount(raw)?),
        }
    }
}

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
    /// so there is no amount to hand [`super::amount`]. Only [`Row::figure`]
    /// reads it off money, which is why a count can never render red.
    pub tone: Tone,
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
/// through [`super::style::account_color`] like every other one: an account
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
    fn color_in(tint: Option<Tint>, column: Column) -> Option<super::style::Color> {
        tint.filter(|t| t.column == column)
            .map(|t| super::style::account_color(t.id, t.color))
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
            account: None,
        }
    }

    fn heading(label: &str) -> Row {
        Row {
            label: label.to_string(),
            bold: true,
            ..Row::blank()
        }
    }

    fn figure(label: &str, value: Cents) -> Row {
        Row {
            label: label.to_string(),
            value: value.to_whole_dollars(),
            tone: if value < Cents::ZERO {
                Tone::Negative
            } else {
                Tone::Plain
            },
            ..Row::blank()
        }
    }

    fn total(label: &str, value: Cents) -> Row {
        Row {
            bold: true,
            ..Row::figure(label, value)
        }
    }

    fn money(label: &str, value: Cents, target: Target) -> Row {
        Row {
            editable: Some(Editable::Constant(target)),
            edit: value.to_string(),
            ..Row::figure(label, value)
        }
    }

    fn count(label: &str, value: i64, target: Target) -> Row {
        Row {
            label: label.to_string(),
            value: value.to_string(),
            editable: Some(Editable::Constant(target)),
            edit: value.to_string(),
            ..Row::blank()
        }
    }

    /// A figure the waterfall computed, with the percentage that produced it
    /// in the extra column. Editing the row edits the percentage.
    fn split(label: &str, value: Cents, pct: Percent, target: Option<Target>) -> Row {
        Row {
            extra: format!("{}%", pct.0),
            editable: target.map(Editable::Constant),
            edit: pct.0.to_string(),
            ..Row::figure(label, value)
        }
    }

    fn bill(bill: &Bill, biweekly: Cents) -> Row {
        Row {
            extra: biweekly.to_whole_dollars(),
            editable: Some(Editable::Constant(Target::Bill(bill.id))),
            edit: bill.cents.to_string(),
            ..Row::figure(&format!("  {}", bill.label), bill.cents)
        }
    }

    fn gate(label: &str, needed: bool) -> Row {
        Row {
            label: format!("  {label}"),
            value: if needed { "needed" } else { "met" }.to_string(),
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
                (goal.clone(), container.name.clone())
            }
            Landing::Account { account } => {
                tint = Some(Tint::of(account, Column::Value));
                (account.name.clone(), String::new())
            }
            Landing::Spread { container } => {
                tint = Some(Tint::of(container, Column::Extra));
                ("spread".to_string(), container.name.clone())
            }
            // Named while they fit and counted past that: the cell is
            // right-aligned, so an overflowing list would lose its *leading*
            // characters and read as a shorter list of the wrong containers.
            // `Enter` has room for all of them.
            Landing::Ambiguous { containers } => (
                "ambiguous".to_string(),
                match containers.len() {
                    0..=2 => containers.join(", "),
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
            extra = format!("{}?", goal.name);
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
fn build(view: &View) -> Result<Vec<Row>> {
    let p = &view.plan;
    let s = &view.settings;
    // The same clamp `compute` makes, for the same reason: a nonsense count
    // must not take the screen down.
    let periods = s.periods_per_year.max(1);
    let bw = |monthly: Cents| calc::biweekly(monthly, periods);

    // What `t` would write, above the waterfall that produced it. These rows
    // are the screen's answer -- the money that actually moves this payday --
    // and every block below is the working behind it: an owner who trusts the
    // plan reads the top and presses `t`, and one who doubts a figure scrolls
    // down to the line that made it.
    let mut rows = vec![Row::heading("Transfers")];
    match &view.transfer_error {
        Some(message) => rows.push(Row::figure_text("  unresolved", message)),
        None => {
            for row in &view.transfers {
                match row {
                    transfer::Row::Transfer {
                        to,
                        name,
                        color,
                        cents,
                        lines,
                    } => {
                        // The account this row's money lands in, named in the
                        // label column -- so it is tinted there rather than
                        // in the two columns a destination row uses. The
                        // lines beneath it are the plan's own labels and name
                        // no account, so they stay plain: the account is said
                        // once, at the head of the group it heads.
                        rows.push(Row {
                            account: Some(Tint {
                                column: Column::Label,
                                id: *to,
                                color: *color,
                            }),
                            ..Row::total(&format!("  {name}"), *cents)
                        });
                        for (line, amount) in lines {
                            rows.push(Row::figure(&format!("    {}", line.label()), *amount));
                        }
                    }
                    transfer::Row::Withdrawal { line, cents } => {
                        rows.push(Row::total("  Withdrawal", *cents));
                        rows.push(Row::figure(&format!("    {}", line.label()), *cents));
                    }
                }
            }
        }
    }
    rows.push(Row::blank());

    rows.extend([
        Row::money("Target", s.target, Target::Target),
        Row::money("Buffer", s.buffer, Target::Buffer),
        Row::count(
            "Pay Periods / Year",
            s.periods_per_year,
            Target::PeriodsPerYear,
        ),
        Row::blank(),
        // The one row a scrub moves. Marked `*` like the Overview column it
        // follows, and naming the date rather than the drift: this screen has
        // no column header to hang a date off.
        Row {
            extra: match view.scrubbed_adhoc {
                Some(date) => format!("{date}*"),
                None => String::new(),
            },
            ..Row::figure("Excess (Actual)", p.excess_actual)
        },
        Row::total("Excess (Used)", p.excess_used),
        Row::blank(),
        Row::heading("Bills"),
    ]);

    // `Planning!C6` -- the housing subtotal, and the only bill line that is
    // not a bill. Its biweekly figure is `E6`, the one `lines.current_housing`
    // uses.
    let housing_monthly: Cents = view.housing.iter().map(|b| b.cents).sum();
    rows.push(Row {
        extra: p.housing_biweekly.to_whole_dollars(),
        ..Row::figure("  Mortgage + HOA", housing_monthly)
    });
    for b in &view.housing {
        rows.push(Row::bill(b, bw(b.cents)?));
    }
    for b in &view.other_bills {
        rows.push(Row::bill(b, bw(b.cents)?));
    }

    rows.push(Row::total("Remaining Excess", p.remaining_excess));
    rows.push(Row::blank());
    rows.push(Row::heading("Gates"));
    rows.push(Row::gate(Gate::EmergencyFund.label(), p.need_emergency));
    rows.push(Row::gate(Gate::Roth.label(), p.need_roth));
    rows.push(Row::blank());

    rows.push(Row::split(
        "Bill Payments",
        p.bill_payments,
        s.bill_payment_pct,
        Some(Target::BillPaymentPct),
    ));
    rows.push(Row::money(
        "  Cap",
        s.bill_payment_cap,
        Target::BillPaymentCap,
    ));
    rows.push(Row::figure("Mom & Dad", p.mom_and_dad));
    rows.push(Row::money(
        "  Annual",
        s.mom_and_dad_annual,
        Target::MomAndDadAnnual,
    ));
    rows.push(Row::total("Remainder", p.remainder));
    rows.push(Row::money(
        "  Goals Floor",
        s.goals_floor,
        Target::GoalsFloor,
    ));
    rows.push(Row::blank());

    rows.push(Row::heading("Split"));
    rows.push(Row::split(
        &format!("  {}", Line::FutureHousing.label()),
        p.future_housing,
        s.future_housing_pct,
        Some(Target::FutureHousingPct),
    ));
    rows.push(Row::split(
        &format!("  {}", Line::Retirement.label()),
        p.retirement,
        s.retirement_pct,
        Some(Target::RetirementPct),
    ));
    rows.push(Row::split(
        &format!("  {}", Line::Investment.label()),
        p.investment,
        s.investment_pct,
        Some(Target::InvestmentPct),
    ));
    // Goals is the plug and takes whatever the other three leave, through the
    // same saturating subtraction `compute` uses. Not editable: four editable
    // shares could sum to something other than 100 with no way to say which
    // one is wrong.
    let goals_pct = Percent::ONE_HUNDRED
        .saturating_sub(s.future_housing_pct + s.retirement_pct + s.investment_pct);
    rows.push(Row::split(
        &format!("  {}", Line::Goals.label()),
        p.goals,
        goals_pct,
        None,
    ));
    rows.push(Row::blank());

    // Where the money the split just divided lands. Below the figures rather
    // than beside the transfers at the top: this block is read when one of
    // them is missing or wrong, which is a question about the line above it.
    rows.push(Row::heading("Destinations"));
    rows.extend(view.wiring.iter().map(Row::destination));
    rows.push(Row::blank());
    rows.push(Row::total("Checksum", p.checksum));

    Ok(rows)
}

/// The Planning screen's view state.
pub struct Planning {
    rows: Vec<Row>,
    excess_actual: Cents,
    transfer_detail: Vec<String>,
    pinned: Option<Cents>,
    pinned_at: Option<NaiveDate>,
    /// Why the screen cannot be drawn, when it cannot. A database with no Everyday
    /// checking account has no waterfall, and every other screen still works
    /// there, so the failure lands here rather than on the reload.
    message: Option<String>,
    cursor: Cursor,
}

impl Planning {
    pub fn new() -> Planning {
        Planning {
            rows: Vec::new(),
            excess_actual: Cents::ZERO,
            transfer_detail: Vec::new(),
            pinned: None,
            pinned_at: None,
            message: None,
            cursor: Cursor::new(),
        }
    }

    /// Rebuild every row, keeping the cursor on the constant it was on.
    ///
    /// By target rather than by index: `e` writes and `App` reloads, and a
    /// cursor that reset to the top on every keystroke would make editing
    /// three constants in a row unusable. A target that no longer exists --
    /// a deleted bill -- falls back to the first editable row.
    pub fn set_view(&mut self, view: View) -> Result<()> {
        let held = self.selected_editable();
        self.rows = build(&view)?;
        self.excess_actual = view.plan.excess_actual;
        self.transfer_detail = view.transfer_detail;
        self.pinned = view.pinned;
        self.pinned_at = view.pinned_at;
        self.message = None;
        self.cursor.select(
            held.and_then(|e| self.rows.iter().position(|r| r.editable == Some(e)))
                .unwrap_or(0),
        );
        if self
            .rows
            .get(self.cursor.index())
            .is_none_or(|r| r.editable.is_none())
        {
            self.select_first();
        }
        Ok(())
    }

    /// Why the plan is unresolved, in full, or empty when it resolves.
    pub fn transfer_detail(&self) -> &[String] {
        &self.transfer_detail
    }

    pub fn set_unavailable(&mut self, message: String) {
        self.rows.clear();
        self.transfer_detail.clear();
        self.cursor.first();
        self.message = Some(message);
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn title(&self) -> &'static str {
        "Planning"
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows
            .get(self.cursor.index())
            .filter(|r| r.editable.is_some())
    }

    pub fn selected_editable(&self) -> Option<Editable> {
        self.selected().and_then(|r| r.editable)
    }

    /// The constant the cursor is on, for the callers that only deal in
    /// constants. `None` on a destination row, which `e` opens a list for
    /// rather than a field.
    pub fn selected_target(&self) -> Option<Target> {
        match self.selected_editable() {
            Some(Editable::Constant(target)) => Some(target),
            _ => None,
        }
    }

    pub fn excess_actual(&self) -> Cents {
        self.excess_actual
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }

    /// How the pin reads on screen, or `None` when the plan is not pinned.
    ///
    /// The date is omitted when it is absent: import transcribes
    /// `Planning!D3` into the pin and has no date to transcribe with it, so an
    /// imported pin is dateless and must render rather than fail.
    pub fn pin_line(&self) -> Option<String> {
        let pinned = self.pinned?;
        let mut line = format!("pinned {pinned}");
        if let Some(at) = self.pinned_at {
            line.push_str(&format!(" on {at}"));
        }
        let drift = self.excess_actual - pinned;
        if drift != Cents::ZERO {
            line.push_str(&format!(" · excess has since moved {drift}"));
        }
        Some(line)
    }

    fn editable_at_or_after(&self, from: usize) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, r)| r.editable.is_some())
            .map(|(index, _)| index)
    }

    fn editable_at_or_before(&self, from: usize) -> Option<usize> {
        self.rows
            .get(..=from)?
            .iter()
            .rposition(|r| r.editable.is_some())
    }
}

/// Planning is the one screen whose rows are not all selectable: barely a
/// third carry a `Target`, and the rest are computed lines the cursor must
/// skip. Every movement is therefore overridden — the shared cursor still
/// holds the index and the page height, but it is never told a row count it
/// could move over freely.
impl Scroll for Planning {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// Unused: every movement below is overridden. Reported honestly anyway,
    /// so a default that starts being used cannot silently see zero.
    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn select_next(&mut self) {
        if let Some((index, _)) = self
            .rows
            .iter()
            .enumerate()
            .skip(self.cursor.index() + 1)
            .find(|(_, r)| r.editable.is_some())
        {
            self.cursor.select(index);
        }
    }

    fn select_previous(&mut self) {
        if let Some((index, _)) = self.rows[..self.cursor.index()]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.editable.is_some())
        {
            self.cursor.select(index);
        }
    }

    fn select_first(&mut self) {
        self.cursor
            .select(self.editable_at_or_after(0).unwrap_or(0));
    }

    fn select_last(&mut self) {
        let index = self
            .rows
            .iter()
            .rposition(|r| r.editable.is_some())
            .unwrap_or(0);
        self.cursor.select(index);
    }

    /// A page is a screenful of *lines*, as `page_height` is measured: the
    /// cursor moves that far down the list and then settles on the nearest
    /// editable row below where it landed.
    ///
    /// Stepping `page_height` *editable* rows instead would be `End` in
    /// disguise — barely a third of these rows are editable, so on any
    /// terminal taller than about twenty lines a page exceeds the whole
    /// editable count.
    fn page_down(&mut self) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let landing = (self.cursor.index() + self.cursor.page_height()).min(last);
        match self.editable_at_or_after(landing) {
            Some(index) => self.cursor.select(index),
            None => self.select_last(),
        }
    }

    fn page_up(&mut self) {
        let landing = self
            .cursor
            .index()
            .saturating_sub(self.cursor.page_height());
        match self.editable_at_or_before(landing) {
            Some(index) => self.cursor.select(index),
            None => self.select_first(),
        }
    }
}

impl Default for Planning {
    fn default() -> Planning {
        Planning::new()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BillField {
    Label,
    Amount,
    Category,
}

impl BillField {
    pub const ORDER: [BillField; 3] = [BillField::Label, BillField::Amount, BillField::Category];

    pub fn label(self) -> &'static str {
        match self {
            BillField::Label => "Label",
            BillField::Amount => "Monthly",
            BillField::Category => "Category",
        }
    }
}

/// Adding or editing one bill. Backs `a` and `E`.
///
/// The category is a selector over `Category::ALL` rather than a text field,
/// so a category the schema's `CHECK` would refuse is unrepresentable.
#[derive(Debug)]
pub struct BillForm {
    /// `Some` when editing an existing bill, `None` when adding one.
    pub editing: Option<BillId>,
    pub focus: BillField,
    label: Field,
    amount: Field,
    category: usize,
}

impl BillForm {
    pub fn add() -> BillForm {
        BillForm {
            editing: None,
            focus: BillField::Label,
            label: Field::default(),
            amount: Field::default(),
            category: 0,
        }
    }

    pub fn edit(bill: &Bill) -> BillForm {
        BillForm {
            editing: Some(bill.id),
            focus: BillField::Label,
            label: Field::given(bill.label.clone()),
            amount: Field::given(bill.cents.to_string()),
            category: bill::Category::ALL
                .iter()
                .position(|c| *c == bill.category)
                .unwrap_or(0),
        }
    }

    pub fn category(&self) -> bill::Category {
        bill::Category::ALL[self.category]
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => "Edit bill — Tab field · ←/→ category · Enter save · Esc cancel",
            None => "Add bill — Tab field · ←/→ category · Enter save · Esc cancel",
        }
    }

    pub fn display(&self, field: BillField) -> Label {
        Label::plain(match field {
            BillField::Label => self.label.value().to_string(),
            BillField::Amount => self.amount.value().to_string(),
            BillField::Category => self.category().as_str().to_string(),
        })
    }

    pub fn commit(&self) -> Result<bill::BillEdit> {
        let label = self.label.value().trim().to_string();
        ensure!(!label.is_empty(), "a bill's label must not be empty");
        Ok(bill::BillEdit {
            label,
            cents: parse_amount(self.amount.value())?,
            category: self.category(),
        })
    }
}

impl FormFields for BillForm {
    fn next_field(&mut self) {
        self.focus = next_in(&BillField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&BillField::ORDER, self.focus, -1);
    }

    /// A no-op unless the selector is focused: `←`/`→` on a text field must
    /// not silently move a bill between subtotals.
    fn next_choice(&mut self) {
        if self.focus == BillField::Category {
            self.category = step_index(self.category, bill::Category::ALL.len(), 1);
        }
    }

    fn previous_choice(&mut self) {
        if self.focus == BillField::Category {
            self.category = step_index(self.category, bill::Category::ALL.len(), -1);
        }
    }

    fn type_char(&mut self, c: char) {
        match self.focus {
            BillField::Label => self.label.push(c),
            BillField::Amount => self.amount.push(c),
            BillField::Category => {}
        }
    }

    fn backspace(&mut self) {
        match self.focus {
            BillField::Label => self.label.backspace(),
            BillField::Amount => self.amount.backspace(),
            BillField::Category => {}
        }
    }
}

/// The confirm modal behind `t`: what will be written, and when.
///
/// The date is the only editable thing. The rows are not: they are the plan,
/// and a plan edited row by row in a modal is a worksheet, which is what
/// opens next.
pub struct TransferConfirm {
    rows: Vec<transfer::Row>,
    date: Field,
}

impl TransferConfirm {
    pub fn new(rows: Vec<transfer::Row>, date: NaiveDate) -> TransferConfirm {
        TransferConfirm {
            rows,
            date: Field::date(date),
        }
    }

    pub fn rows(&self) -> &[transfer::Row] {
        &self.rows
    }

    pub fn date_value(&self) -> &str {
        self.date.value()
    }

    pub fn type_char(&mut self, c: char) {
        self.date.push(c);
    }

    pub fn backspace(&mut self) {
        self.date.backspace();
    }

    /// Step the date by `days`, as `←`/`→` do on every date field in the app.
    pub fn step_date(&mut self, days: i64) {
        self.date.step_date(days);
    }

    /// The date as typed. Parsed before anything is written, so a typo leaves
    /// the modal up with everything still in it.
    pub fn commit(&self) -> Result<NaiveDate> {
        parse_date(self.date.value())
    }
}

use super::form::{centered, field_line, render_fields};
use super::table_state;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Clear, Paragraph, Row as TableRow, Table, Wrap};

pub fn render_bill(frame: &mut Frame, form: &BillForm) {
    let lines: Vec<TextLine> = BillField::ORDER
        .iter()
        .map(|f| field_line(f.label(), form.display(*f), form.focus == *f))
        .collect();
    render_fields(frame, form.title(), lines);
}

/// The confirm modal behind `t`: what will be written, and when. The strings
/// on screen are the strings that land in the ledger -- a transfer's own
/// name, a withdrawal's line label -- so this is a preview of the ledger
/// rows, not a summary of them.
pub fn render_transfers(frame: &mut Frame, confirm: &TransferConfirm) {
    let rows = confirm.rows();
    let mut lines: Vec<TextLine> = rows
        .iter()
        .map(|row| {
            let (label, cents) = match row {
                transfer::Row::Transfer { name, cents, .. } => (name.clone(), *cents),
                transfer::Row::Withdrawal { line, cents } => (line.label().to_string(), *cents),
            };
            TextLine::from(format!("{label:<40}{:>20}", cents.to_whole_dollars()))
        })
        .collect();
    lines.push(field_line(
        "Date",
        Label::from(confirm.date_value().to_string()),
        true,
    ));
    lines.push(TextLine::from("Enter write · Esc cancel"));
    render_fields(frame, "Confirm transfers", lines);
}

/// The long form of a failure the screen only had a table cell for.
///
/// Wrapped rather than truncated, and sized to what it holds: an empty line
/// in `lines` is a paragraph break, and every other line wraps inside the
/// panel's width.
pub fn render_details(frame: &mut Frame, title: &str, lines: &[String]) {
    const WIDTH: u16 = 68;
    let inner = WIDTH.saturating_sub(2).max(1);
    let wrapped: u16 = lines
        .iter()
        .map(|l| (l.chars().count() as u16).div_ceil(inner).max(1))
        .sum();
    let height = (wrapped + 3).min(frame.area().height);
    let area = centered(frame.area(), WIDTH, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(
            lines
                .iter()
                .map(|l| TextLine::from(l.clone()))
                .collect::<Vec<_>>(),
        )
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(format!("{title} — Esc close"))),
        area,
    );
}

/// The waterfall, top to bottom, with the pin line beneath it. Returns the
/// viewport's height, for `PageUp`/`PageDown`.
pub fn render(frame: &mut Frame, area: Rect, planning: &Planning) -> usize {
    if let Some(message) = planning.message() {
        frame.render_widget(
            Paragraph::new(TextLine::from(message.to_string()))
                .block(Block::bordered().title(planning.title())),
            area,
        );
        return 1;
    }

    let pin = planning.pin_line();
    let footer_height = if pin.is_some() { 3 } else { 0 };
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(footer_height)]).areas(area);

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let rows: Vec<TableRow> = planning
        .rows()
        .iter()
        .map(|r| {
            let tint = |column| Tint::color_in(r.account, column);
            let row = TableRow::new(vec![
                super::tinted(TextLine::from(r.label.clone()), tint(Column::Label)),
                super::tinted(
                    TextLine::from(r.value.clone()).right_aligned(),
                    // Tone first: red says this plan will not run and amber
                    // says there is a gap worth filling, and neither may be
                    // displaced by a tint that only says which account.
                    super::style::tone_color(r.tone).or_else(|| tint(Column::Value)),
                ),
                super::tinted(
                    TextLine::from(r.extra.clone()).right_aligned(),
                    tint(Column::Extra),
                ),
            ]);
            if r.bold { row.style(bold) } else { row }
        })
        .collect();
    // Both fixed columns are sized for names rather than for figures: the
    // value column carries a goal's ("Home Down Payment"), and the extra
    // column carries either its container or a suggested goal's name with a
    // question mark after it -- two characters longer again. Truncation here
    // is not a visible ellipsis but a silently missing prefix, so the room
    // comes out of the label column, which has the `Min` and every spare
    // column the terminal is wider than.
    let widths = [
        Constraint::Min(22),
        Constraint::Length(24),
        Constraint::Length(24),
    ];

    // Two borders are not available to data rows; there is no header here.
    let height = usize::from(table_area.height).saturating_sub(2);
    let mut state = table_state(planning.selected_index(), planning.rows().len(), height);
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ")
            .block(Block::bordered().title(planning.title())),
        table_area,
        &mut state,
    );

    if let Some(pin) = pin {
        frame.render_widget(
            Paragraph::new(TextLine::from(pin)).block(Block::bordered().title("Pinned")),
            footer_area,
        );
    }

    height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::planning::{PlanInputs, compute};
    use crate::db;
    use crate::db::bill::Category;
    use crate::tui::MIN_WIDTH;
    // `setting`, `key`, and `Line` already come in through `super::*`.

    /// The value column carries figures, counts and gate verdicts alike, so
    /// only the constructor that takes money gets to call a row negative. A
    /// count is a number, not an amount, and must never render red.
    #[test]
    fn only_a_money_row_below_zero_is_toned_negative() {
        assert_eq!(Row::figure("Checksum", Cents(-1)).tone, Tone::Negative);
        assert_eq!(
            Row::total("Remaining Excess", Cents(-100)).tone,
            Tone::Negative
        );
        assert_eq!(Row::figure("Checksum", Cents::ZERO).tone, Tone::Plain);
        assert_eq!(Row::total("Remaining Excess", Cents(1)).tone, Tone::Plain);
        assert_eq!(
            Row::count("Pay Periods", 26, Target::PeriodsPerYear).tone,
            Tone::Plain
        );
        assert_eq!(Row::heading("Gates").tone, Tone::Plain);
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

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

    fn bill(id: i64, label: &str, dollars: i64, category: Category, sort: i64) -> Bill {
        Bill {
            id: BillId(id),
            label: label.to_string(),
            cents: Cents::from_dollars(dollars),
            category,
            sort,
        }
    }

    fn housing() -> Vec<Bill> {
        vec![
            bill(1, "Mortgage", 1_200, Category::Housing, 0),
            bill(2, "HOA", 300, Category::Housing, 1),
        ]
    }

    fn other_bills() -> Vec<Bill> {
        vec![
            bill(3, "Plumber", 90, Category::Other, 0),
            bill(4, "Phone", 60, Category::Other, 1),
            bill(5, "Newspaper", 25, Category::Other, 2),
            bill(6, "Coworking", 1_000, Category::Other, 3),
        ]
    }

    /// Three transfers grouping the plan's nine lines the way the workbook's
    /// own Rainy Day/Brokerage/Nest Egg containers do, built by hand rather than
    /// through `transfer::plan` -- these tests hand-build a `Plan` with no
    /// `Db` behind it, and the destination block only ever renders what it is
    /// given.
    fn transfers(plan: &Plan) -> Vec<transfer::Row> {
        let l = &plan.lines;
        vec![
            transfer::Row::Transfer {
                to: crate::db::AccountId(1),
                name: "Rainy Day".to_string(),
                color: None,
                cents: l.bills + l.current_housing + l.goals + l.roth,
                lines: vec![
                    (Line::Bills, l.bills),
                    (Line::CurrentHousing, l.current_housing),
                    (Line::Goals, l.goals),
                    (Line::Roth, l.roth),
                ],
            },
            transfer::Row::Transfer {
                to: crate::db::AccountId(2),
                name: "Brokerage".to_string(),
                color: None,
                cents: l.future_housing + l.mom_and_dad + l.emergency_fund,
                lines: vec![
                    (Line::FutureHousing, l.future_housing),
                    (Line::MomAndDad, l.mom_and_dad),
                    (Line::EmergencyFund, l.emergency_fund),
                ],
            },
            transfer::Row::Transfer {
                to: crate::db::AccountId(3),
                name: "Nest Egg".to_string(),
                color: None,
                cents: l.retirement + l.investment,
                lines: vec![
                    (Line::Retirement, l.retirement),
                    (Line::Investment, l.investment),
                ],
            },
        ]
    }

    fn goal(name: &str, container: i64) -> crate::db::goal::Goal {
        crate::db::goal::Goal {
            id: crate::db::GoalId(1),
            name: name.to_string(),
            container_account_id: crate::db::AccountId(container),
            goal_cents: Cents::from_dollars(1_000),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            closed: false,
            sort: 0,
        }
    }

    fn wired(line: Line, landing: Landing) -> Wiring {
        Wiring {
            line,
            landing,
            suggestion: None,
        }
    }

    /// A container by name, with an invented id so the two in this fixture
    /// are distinguishable -- the screen tints by id, so two containers
    /// sharing one would stop the tint tests saying anything.
    fn container(name: &str) -> Container {
        Container {
            id: AccountId(if name == "Brokerage" { 2 } else { 1 }),
            name: name.to_string(),
            color: None,
        }
    }

    fn in_goal(name: &str, container_name: &str) -> Landing {
        Landing::Goal {
            goal: name.to_string(),
            container: container(container_name),
        }
    }

    /// The owner's own database, a fortnight after the destination keys were
    /// added: everything the import matched is pointed somewhere, and the one
    /// line whose key that import predates is unset with its goal sitting
    /// there unclaimed.
    fn wiring() -> Vec<Wiring> {
        vec![
            wired(Line::Bills, in_goal("Bill Payments", "Rainy Day")),
            wired(Line::CurrentHousing, in_goal("Housing", "Rainy Day")),
            wired(
                Line::Goals,
                Landing::Spread {
                    container: container("Rainy Day"),
                },
            ),
            wired(Line::Roth, in_goal("Roth IRA", "Rainy Day")),
            Wiring {
                line: Line::FutureHousing,
                landing: Landing::Withdrawal,
                suggestion: Some(goal("Home Down Payment", 2)),
            },
            wired(Line::MomAndDad, in_goal("Mom & Dad", "Brokerage")),
            wired(
                Line::EmergencyFund,
                in_goal("Emergency Savings", "Brokerage"),
            ),
            wired(Line::Retirement, Landing::Withdrawal),
            wired(Line::Investment, Landing::Withdrawal),
        ]
    }

    /// The workbook's own inputs, so every figure on the screen is one the
    /// `calc::planning` tests already pin against a cell.
    fn view(pinned: Option<Cents>, pinned_at: Option<NaiveDate>) -> View {
        let settings = settings();
        let inputs = PlanInputs {
            checking_at_adhoc: Cents(3_250_075),
            pinned_excess: pinned,
            housing_monthly: housing().iter().map(|b| b.cents).collect(),
            other_bills_monthly: other_bills().iter().map(|b| b.cents).collect(),
            remaining_emergency: Cents::ZERO,
            remaining_roth: Cents::ZERO,
        };
        let plan = compute(&settings, &inputs).unwrap();
        let transfers = transfers(&plan);
        View {
            plan,
            settings,
            housing: housing(),
            other_bills: other_bills(),
            pinned,
            pinned_at,
            scrubbed_adhoc: None,
            wiring: wiring(),
            transfers,
            transfer_error: None,
            transfer_detail: Vec::new(),
        }
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

    /// A marked date is 11 characters in a column sized for goal names, so
    /// it fits with room to spare -- but a truncated date is a *different*
    /// date rather than a visible ellipsis, and the column's width is set
    /// for other content entirely, so nothing but this says the date still
    /// fits. Drawn at `MIN_WIDTH`, where the column is narrowest.
    #[test]
    fn a_scrubbed_date_is_whole_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut v = view(None, None);
        v.scrubbed_adhoc = Some(day(2026, 8, 29));
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn: String = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(drawn.contains("2026-08-29*"), "{drawn}");
    }

    /// The tint reaches the screen, and reaches the container's name rather
    /// than the goal's. `render` puts the tone ahead of it on the value
    /// column -- red and amber carry instructions where a tint only says
    /// which account -- which no landing currently exercises, because every
    /// landing carrying an account is one that resolved.
    #[test]
    fn a_container_is_drawn_in_its_accounts_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // A dangling account key: red, and naming nothing that exists.
        let mut v = view(None, None);
        v.wiring = wiring();
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // The destination row, located by its label: the screen leads with
        // the transfers, which name accounts of their own, so a search over
        // the whole buffer would find one of those instead.
        // Both needles, because the transfer block above carries a row
        // under the same label -- the destination row is the one that also
        // names the container.
        let (y, line) = (0..height)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains(Line::MomAndDad.label()) && line.contains("Brokerage"))
            .expect("no Mom & Dad destination row");
        let fg = |needle: &str| buffer[(super::super::column_of(&line, needle), y)].fg;

        // `Mom & Dad` lands in Brokerage, whose container name is tinted.
        assert_eq!(
            fg("Brokerage"),
            super::super::style::account_color(AccountId(2), None),
            "{line:?}"
        );
        // And the goal's own name is not an account, so it stays plain.
        assert_eq!(fg("Mom & Dad"), ratatui::style::Color::Reset, "{line:?}");
    }

    #[test]
    fn an_unscrubbed_plan_leaves_the_excess_actual_extra_column_empty() {
        let planning = screen();

        assert_eq!(row(&planning, "Excess (Actual)").extra, "");
    }

    /// The screen with one destination row replaced, for the states a
    /// healthy database does not hold.
    fn screen_with(line: Line, landing: Landing) -> Planning {
        let mut v = view(None, None);
        let row = v
            .wiring
            .iter_mut()
            .find(|w| w.line == line)
            .expect("every line is wired");
        row.landing = landing;
        row.suggestion = None;
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();
        planning
    }

    fn screen() -> Planning {
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(17_500)), None))
            .unwrap();
        planning
    }

    fn row<'a>(planning: &'a Planning, label: &str) -> &'a Row {
        planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == label)
            .unwrap_or_else(|| panic!("no row labelled {label:?}"))
    }

    /// The destination rows repeat labels the screen already uses -- "Bills"
    /// heads the bill block and names a transfer's line -- so they are found
    /// by walking down from the block's own heading rather than by label
    /// alone.
    fn destination(planning: &Planning, line: Line) -> &Row {
        let start = planning
            .rows()
            .iter()
            .position(|r| r.label == "Destinations")
            .expect("no Destinations heading");
        planning.rows()[start..]
            .iter()
            .take_while(|r| !r.label.is_empty())
            .find(|r| r.label.trim() == line.label())
            .unwrap_or_else(|| panic!("no destination row for {line:?}"))
    }

    #[test]
    fn a_configured_destination_names_its_goal_and_its_container() {
        let planning = screen();
        let row = destination(&planning, Line::EmergencyFund);
        assert_eq!(row.value, "Emergency Savings");
        assert_eq!(row.extra, "Brokerage");
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

    /// The tint reaches the transfer's account name and stops at the indent
    /// in front of it. Planning indents its labels to show nesting, and an
    /// indent is structure rather than content -- a colored run of leading
    /// spaces is invisible until something reverses the row, and then it is
    /// a block of background sitting in front of the name.
    #[test]
    fn a_transfer_label_is_tinted_from_its_first_glyph() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let planning = screen();
        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let (y, line) = (0..height)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains("Rainy Day") && !line.contains("spread"))
            .expect("no Rainy Day transfer row");

        let at = super::super::column_of(&line, "Rainy Day");
        let expected = super::super::style::account_color(AccountId(1), None);
        assert_eq!(buffer[(at, y)].fg, expected, "{line:?}");
        // The indent in front of it is untinted, and so is everything before.
        for x in 0..at {
            assert_eq!(
                buffer[(x, y)].fg,
                super::super::style::Color::Reset,
                "column {x} of {line:?} is tinted"
            );
        }
    }

    /// The transfers block heads each row with the account the money lands
    /// in, so that name is tinted like the same account everywhere else --
    /// in the *label* column, which is where a transfer names its account
    /// and where no other row on the screen names one.
    #[test]
    fn a_transfer_row_is_tinted_by_the_account_it_lands_in() {
        let planning = screen();
        let row = planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "Rainy Day")
            .expect("no Rainy Day transfer row");
        assert_eq!(
            row.account,
            Some(Tint {
                column: Column::Label,
                id: AccountId(1),
                color: None
            })
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

    /// Half the screen is labels and computed figures. A cursor that could
    /// land on one would make `e` do nothing, with no way to tell that from a
    /// key that failed.
    #[test]
    fn the_cursor_only_ever_lands_on_an_editable_row() {
        let mut planning = screen();
        assert!(planning.selected().unwrap().editable.is_some());

        let mut seen = 0;
        for _ in 0..planning.rows().len() * 2 {
            planning.select_next();
            assert!(
                planning.selected().unwrap().editable.is_some(),
                "the cursor stopped on {:?}",
                planning.selected().unwrap().label
            );
            seen += 1;
        }
        assert!(seen > 0);
    }

    #[test]
    fn the_first_editable_row_is_selected_when_the_screen_loads() {
        let planning = screen();
        assert_eq!(planning.selected_target(), Some(Target::Target));
    }

    /// The destination block sits below the split, so the end of the screen
    /// is the last line whose destination `e` can point somewhere -- the
    /// Emergency Fund row below it shares its key with a gate, and the two
    /// account lines below that are display only.
    #[test]
    fn the_last_editable_row_is_the_last_editable_destination() {
        let mut planning = screen();
        planning.select_last();
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// `select_previous` is the one cursor mover that slices `rows[..selected]`
    /// -- the one place an off-by-one would panic rather than merely
    /// misbehave. Walking all the way back must stop on the first editable
    /// row rather than wrapping or panicking at the start of the list.
    #[test]
    fn repeated_select_previous_walks_back_to_the_first_editable_row_and_stops() {
        let mut planning = screen();
        planning.select_last();
        for _ in 0..planning.rows().len() {
            planning.select_previous();
        }
        assert_eq!(planning.selected_target(), Some(Target::Target));
    }

    /// The far end of the same invariant: `select_next` past the last
    /// editable row is a no-op, not a wrap back to the top.
    #[test]
    fn repeated_select_next_past_the_last_editable_row_stays_put() {
        let mut planning = screen();
        planning.select_last();
        for _ in 0..3 {
            planning.select_next();
        }
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// `page_height` is a count of *lines*, so a page has to move that far
    /// down the list and settle on the nearest editable row. Twenty lines from
    /// the top of this screen is the bill-payment block; twenty *editable*
    /// rows would be off the end of a list that has twenty-two of them,
    /// making `PageDown` an `End` in disguise.
    #[test]
    fn paging_down_moves_a_screenful_of_lines_not_the_whole_list() {
        let mut planning = screen();
        planning.set_page_height(20);

        planning.page_down();
        assert_eq!(planning.selected_target(), Some(Target::BillPaymentPct));

        planning.page_down();
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// The same, upwards: one page back from the last destination is the
    /// bill block, not the top of the screen.
    #[test]
    fn paging_up_moves_a_screenful_of_lines_back() {
        let mut planning = screen();
        planning.set_page_height(20);
        planning.select_last();

        planning.page_up();

        assert_eq!(planning.selected_target(), Some(Target::Bill(BillId(6))));
    }

    /// Paging past either end stops there rather than panicking or wrapping,
    /// the same guarantee the single-step movers give.
    #[test]
    fn paging_past_either_end_stops_at_that_end() {
        let mut planning = screen();
        planning.set_page_height(20);

        for _ in 0..10 {
            planning.page_up();
        }
        assert_eq!(planning.selected_target(), Some(Target::Target));

        for _ in 0..10 {
            planning.page_down();
        }
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
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
            before, 10,
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

    /// The Transfers block at the top and everything below the split are the
    /// widest the screen gets: the labels are the longest ("Current
    /// Housing", "Emergency Fund") and the deepest indented, and the
    /// Destinations block puts a goal's name and its container into two
    /// fixed-width columns that were sized for figures. A right-aligned cell
    /// that gets truncated loses its *leading* characters, which turns a
    /// wrong width into a wrong number rather than a visible ellipsis, so
    /// every cell has to survive whole, not just the labels. Every row is
    /// checked, since the cheap parts of the screen cost nothing to cover.
    ///
    /// "Emergency Fund" also labels the Gates row and "  Future Housing" the
    /// Split row, both rendered on every load, so a bare substring search
    /// over the whole screen would pass on the strength of either of those
    /// even if the copy below the split were the one cut short. Each row is
    /// checked against the specific buffer line it rendered on, located by
    /// mapping `planning.rows()`'s index to the buffer through the model's
    /// own "Checksum" row -- its fixed last entry.
    #[test]
    fn every_row_renders_whole_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let v = view(Some(Cents::from_dollars(17_500)), None);
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        // Tall enough for every row plus both borders and the pinned footer,
        // so nothing scrolls out of view before the assertions below run.
        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();

        let model_rows = planning.rows();
        let checksum_y = lines
            .iter()
            .position(|l| l.contains("Checksum"))
            .expect("no Checksum row rendered");
        let offset = checksum_y - (model_rows.len() - 1);

        // The Destinations block sits between the Split section's own
        // "Goals" row (plus its trailing blank) and the blank row before
        // Checksum -- the same boundaries `build` draws it with.
        let goals_idx = model_rows
            .iter()
            .position(|r| r.label.trim() == "Goals")
            .expect("no Goals split row");
        let block_start = goals_idx + 2;
        let block_end = model_rows.len() - 2;
        assert!(block_start < block_end, "nothing rendered below the split");

        let targets = [
            Line::CurrentHousing.label(),
            Line::EmergencyFund.label(),
            Line::FutureHousing.label(),
        ];
        let mut found = [false; 3];
        for (i, row) in model_rows.iter().enumerate() {
            let below_the_split = (block_start..block_end).contains(&i);
            let label = row.label.trim();
            let line = &lines[offset + i];
            assert!(
                line.contains(label),
                "{label:?} truncated on its own rendered row:\n{line}"
            );
            for cell in [&row.value, &row.extra] {
                if !cell.is_empty() {
                    assert!(
                        line.contains(cell.as_str()),
                        "{cell:?} truncated on its own rendered row:\n{line}"
                    );
                }
            }
            for (target, seen) in targets.iter().zip(found.iter_mut()) {
                *seen |= below_the_split && label == *target;
            }
        }
        for (target, seen) in targets.iter().zip(found.iter()) {
            assert!(seen, "{target:?} rendered nowhere below the split");
        }
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

    /// `e` writes and `App` reloads. A cursor that reset to the top on every
    /// keystroke would make editing three constants in a row unusable.
    #[test]
    fn the_cursor_stays_on_its_target_across_a_reload() {
        let mut planning = screen();
        planning.select_next();
        planning.select_next();
        let held = planning.selected_target();
        assert_eq!(held, Some(Target::PeriodsPerYear));

        planning
            .set_view(view(Some(Cents::from_dollars(17_500)), None))
            .unwrap();

        assert_eq!(planning.selected_target(), held);
    }

    /// A bill deleted out from under the cursor has no row to return to.
    #[test]
    fn a_cursor_on_a_deleted_bill_falls_back_to_the_first_editable_row() {
        let mut planning = screen();
        while planning.selected_target() != Some(Target::Bill(BillId(6))) {
            planning.select_next();
        }

        let mut without_wework = view(Some(Cents::from_dollars(17_500)), None);
        without_wework.other_bills.pop();
        planning.set_view(without_wework).unwrap();

        assert_eq!(planning.selected_target(), Some(Target::Target));
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
        assert_eq!(Row::figure("Checksum", Cents(-20_099)).value, "-200");
        assert_eq!(Row::figure("Checksum", Cents(20_099)).value, "200");
    }

    /// Dropping the digits leaves a sub-dollar negative as `-0`. Shared with
    /// the Savings screen, which renders the same way -- worth knowing before
    /// reading a `-0` as a bug in the waterfall rather than in the format.
    #[test]
    fn a_negative_under_a_dollar_keeps_its_sign_over_a_zero() {
        assert_eq!(Row::figure("Checksum", Cents(-1)).value, "-0");
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

    /// `p` pins the live figure, so the screen has to carry it.
    #[test]
    fn the_screen_carries_the_live_excess_for_the_pin_key() {
        let planning = screen();
        assert_eq!(planning.excess_actual(), Cents(1_750_075));
    }

    /// A database with no Everyday checking account has no waterfall. Every other
    /// screen still works there, so the failure belongs on this one.
    #[test]
    fn an_unavailable_screen_holds_a_message_and_no_rows() {
        let mut planning = screen();
        planning.set_unavailable("no Everyday cash account".to_string());
        assert!(planning.rows().is_empty());
        assert!(planning.selected().is_none());
        assert_eq!(planning.message(), Some("no Everyday cash account"));
    }

    #[test]
    fn each_target_writes_its_own_setting() {
        let db = db::open_in_memory().unwrap();
        Target::Target.write(&db, "1,000").unwrap();
        Target::Buffer.write(&db, "$2,000.50").unwrap();
        Target::PeriodsPerYear.write(&db, "24").unwrap();
        Target::BillPaymentCap.write(&db, "3000").unwrap();
        Target::BillPaymentPct.write(&db, "60").unwrap();
        Target::MomAndDadAnnual.write(&db, "4000").unwrap();
        Target::GoalsFloor.write(&db, "500").unwrap();
        Target::FutureHousingPct.write(&db, "30").unwrap();
        Target::RetirementPct.write(&db, "20").unwrap();
        Target::InvestmentPct.write(&db, "10").unwrap();

        let cents = |k| setting::get(&db, k).unwrap().unwrap();
        let pct = |k| setting::get(&db, k).unwrap().unwrap();
        assert_eq!(cents(key::PLANNING_TARGET), Cents::from_dollars(1_000));
        assert_eq!(cents(key::PLANNING_BUFFER), Cents(200_050));
        assert_eq!(
            setting::get(&db, key::PAY_PERIODS_PER_YEAR).unwrap(),
            Some(24)
        );
        assert_eq!(cents(key::BILL_PAYMENT_CAP), Cents::from_dollars(3_000));
        assert_eq!(pct(key::BILL_PAYMENT_PCT), Percent(60));
        assert_eq!(cents(key::MOM_AND_DAD_ANNUAL), Cents::from_dollars(4_000));
        assert_eq!(cents(key::GOALS_FLOOR), Cents::from_dollars(500));
        assert_eq!(pct(key::SPLIT_FUTURE_HOUSING_PCT), Percent(30));
        assert_eq!(pct(key::SPLIT_RETIREMENT_PCT), Percent(20));
        assert_eq!(pct(key::SPLIT_INVESTMENT_PCT), Percent(10));
    }

    #[test]
    fn a_bill_target_writes_the_bill_rather_than_a_setting() {
        let db = db::open_in_memory().unwrap();
        let id = crate::db::bill::insert(
            &db,
            &crate::db::bill::NewBill {
                label: "Mortgage".to_string(),
                cents: Cents::from_dollars(1_200),
                category: Category::Housing,
                sort: 0,
            },
        )
        .unwrap();

        Target::Bill(id).write(&db, "3,100").unwrap();

        let found = crate::db::bill::get(&db, id).unwrap();
        assert_eq!(found.cents, Cents::from_dollars(3_100));
        assert_eq!(found.label, "Mortgage");
    }

    /// `Percent` is whole percent. Accepting `0.35` would silently divide the
    /// split by a hundred and reroute every discretionary dollar.
    #[test]
    fn a_percentage_takes_a_bare_number_or_a_trailing_sign_and_nothing_else() {
        let db = db::open_in_memory().unwrap();
        Target::RetirementPct.write(&db, " 15% ").unwrap();
        assert_eq!(
            setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(),
            Some(Percent(15))
        );

        let err = Target::RetirementPct.write(&db, "0.35").unwrap_err();
        assert!(err.to_string().contains("0.35"), "{err}");
        assert!(Target::RetirementPct.write(&db, "fifteen").is_err());
    }

    /// `Percent::of` does not clamp, so a percentage outside `0..=100` would
    /// write a negative or over-100 allocation straight into the waterfall
    /// with no error at any layer downstream.
    #[test]
    fn a_percentage_outside_zero_to_one_hundred_is_refused() {
        let db = db::open_in_memory().unwrap();
        let err = Target::RetirementPct.write(&db, "-15").unwrap_err();
        assert!(err.to_string().contains("-15"), "{err}");
        assert!(Target::RetirementPct.write(&db, "101").is_err());
        assert_eq!(setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(), None);
    }

    /// The `.max(1)` clamps downstream are the backstop; the form is where a
    /// nonsense count should be refused with a message that names it.
    #[test]
    fn a_non_positive_pay_period_count_is_refused() {
        let db = db::open_in_memory().unwrap();
        let err = Target::PeriodsPerYear.write(&db, "0").unwrap_err();
        assert!(err.to_string().contains('0'), "{err}");
        assert!(Target::PeriodsPerYear.write(&db, "-4").is_err());
        assert!(Target::PeriodsPerYear.write(&db, "26.5").is_err());
        assert_eq!(setting::get(&db, key::PAY_PERIODS_PER_YEAR).unwrap(), None);
    }

    #[test]
    fn a_bill_form_commits_what_was_typed() {
        let mut form = BillForm::add();
        assert_eq!(form.editing, None);
        assert_eq!(form.category(), Category::Housing);

        for c in "Plumber".chars() {
            form.type_char(c);
        }
        form.next_field();
        for c in "$82.00".chars() {
            form.type_char(c);
        }
        form.next_field();
        form.next_choice();

        let edit = form.commit().unwrap();
        assert_eq!(edit.label, "Plumber");
        assert_eq!(edit.cents, Cents::from_dollars(82));
        assert_eq!(edit.category, Category::Other);
    }

    #[test]
    fn a_bill_form_opened_on_a_bill_prefills_every_field() {
        let form = BillForm::edit(&bill(4, "Phone", 60, Category::Other, 1));
        assert_eq!(form.editing, Some(BillId(4)));
        assert_eq!(form.display(BillField::Label).plain_text(), "Phone");
        assert_eq!(form.display(BillField::Amount).plain_text(), "60.00");
        assert_eq!(form.category(), Category::Other);
    }

    /// A blank-labelled bill is the state §3 makes an import error; the form
    /// must not be the way back into it.
    #[test]
    fn a_bill_with_no_label_is_refused() {
        let mut form = BillForm::add();
        form.next_field();
        for c in "82".chars() {
            form.type_char(c);
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("label"), "{err}");
    }

    #[test]
    fn a_bill_with_an_unparseable_amount_is_refused_with_the_text_that_failed() {
        let mut form = BillForm::add();
        for c in "Plumber".chars() {
            form.type_char(c);
        }
        form.next_field();
        for c in "eighty".chars() {
            form.type_char(c);
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("eighty"), "{err}");
    }

    /// The selector cycles both categories and comes back round -- there is
    /// no third one to reach.
    #[test]
    fn the_category_selector_cycles_both_ways_through_both_categories() {
        let mut form = BillForm::add();
        form.next_field();
        form.next_field();
        assert_eq!(form.category(), Category::Housing);
        form.next_choice();
        assert_eq!(form.category(), Category::Other);
        form.next_choice();
        assert_eq!(form.category(), Category::Housing);
        form.previous_choice();
        assert_eq!(form.category(), Category::Other);
    }

    /// `←`/`→` on a text field must not silently change the category.
    #[test]
    fn cycling_does_nothing_unless_the_category_is_focused() {
        let mut form = BillForm::add();
        form.next_choice();
        form.next_choice();
        assert_eq!(form.category(), Category::Housing);
    }

    /// Every rendered line of the confirm modal, inside the border.
    fn drawn_confirm(confirm: &TransferConfirm) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 12)).unwrap();
        terminal
            .draw(|frame| {
                render_transfers(frame, confirm);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..12)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The confirm modal is part of the Planning screen, so its amounts drop
    /// the cents like every other figure on it. It is neither the pin-drift
    /// footer nor the `edit` prefill, the two places that keep full
    /// precision on purpose.
    #[test]
    fn the_confirm_modal_renders_whole_dollars() {
        let rows = vec![
            transfer::Row::Transfer {
                to: crate::db::AccountId(1),
                name: "Brokerage".to_string(),
                color: None,
                cents: Cents(123_456),
                lines: Vec::new(),
            },
            transfer::Row::Withdrawal {
                line: Line::Retirement,
                cents: Cents(197_900),
            },
        ];
        let text = drawn_confirm(&TransferConfirm::new(rows, day(2026, 8, 24)));

        assert!(text.contains("1,234"), "{text}");
        assert!(text.contains("1,979"), "{text}");
        assert!(!text.contains("1,234.56"), "{text}");
        assert!(!text.contains("1,979.00"), "{text}");
    }

    /// The date opens two business days out and is editable: an unparseable
    /// one is refused with the modal still up, so nothing is written on a
    /// typo.
    #[test]
    fn the_confirm_modal_opens_two_business_days_out_and_refuses_a_bad_date() {
        let date = crate::calc::business_day::add(day(2026, 8, 20), 2).unwrap();
        let mut confirm = TransferConfirm::new(Vec::new(), date);
        assert_eq!(confirm.date_value(), "2026-08-24");
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 24));

        for _ in 0..10 {
            confirm.backspace();
        }
        for c in "not-a-date".chars() {
            confirm.type_char(c);
        }
        assert!(confirm.commit().is_err());
    }

    /// The dialog's one field is a date, so `←`/`→` step it a day at a time —
    /// the same meaning they carry on every other date field in the app.
    #[test]
    fn the_arrows_step_the_confirm_date_by_a_day() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 24));
        confirm.step_date(1);
        assert_eq!(confirm.date_value(), "2026-08-25");
        confirm.step_date(-1);
        confirm.step_date(-1);
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 23));
    }

    /// A half-typed date keeps what was typed: the arrows nudge a date that
    /// is already there rather than conjuring one.
    #[test]
    fn the_arrows_leave_a_half_typed_confirm_date_alone() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 24));
        for _ in 0..10 {
            confirm.backspace();
        }
        for c in "2026-0".chars() {
            confirm.type_char(c);
        }
        confirm.step_date(1);
        assert_eq!(confirm.date_value(), "2026-0");
    }
}
