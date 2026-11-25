use super::biweekly;
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::Result;

/// The tuned constants of the waterfall. Every one is editable in the UI.
#[derive(Debug, Clone)]
pub struct PlanSettings {
    /// Cash to retain in checking. Sheet `Planning!D1`.
    pub target: Cents,
    /// Additional checking buffer held back. `Planning!J11`.
    pub buffer: Cents,
    /// `Constants!G2`.
    pub periods_per_year: i64,
    /// Ceiling on the bill-payment allocation. `Planning!E19`.
    pub bill_payment_cap: Cents,
    /// Share of remaining excess offered to bill payments. `Planning!F19`.
    pub bill_payment_pct: BasisPoints,
    /// Annual Mom & Dad commitment, divided by pay periods. `Planning!E20`.
    pub mom_and_dad_annual: Cents,
    /// Below this remainder, everything goes to Goals. `Planning!E24`.
    pub goals_floor: Cents,
    /// `Planning!F25`, `F26`, `F27`. Goals takes whatever is left over.
    pub future_housing_pct: BasisPoints,
    pub retirement_pct: BasisPoints,
    pub investment_pct: BasisPoints,
}

impl PlanSettings {
    /// Goals is the plug of the four-way split: whatever the other three
    /// leave.
    ///
    /// On the settings rather than beside a screen, because the waterfall
    /// spends it and both sinks label the Goals row with it -- three copies of
    /// one subtraction is three chances to disagree about what the split even
    /// is. Not editable for the same reason it is derived: four editable
    /// shares could sum to something other than 100 with no way to say which
    /// one is wrong.
    ///
    /// `saturating_sub` rather than a plain subtraction: the three shares are
    /// user-editable and can be configured to more than 100 between them,
    /// which would otherwise allocate a negative share to Goals.
    pub fn goals_pct(&self) -> BasisPoints {
        BasisPoints::ONE
            .saturating_sub(self.future_housing_pct + self.retirement_pct + self.investment_pct)
    }
}

/// How many gates the waterfall runs.
///
/// `calc` knows a gate by its index and nothing else: `gate::Gate` owns a
/// setting key, so it lives above the database boundary. `Gate::index` is what
/// ties the two.
pub const GATES: usize = 2;

/// Where a gate's goal stands: what it holds against what it is funded to.
///
/// The pair rather than the difference, because both sinks draw an unmet gate
/// as the two figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub current: Cents,
    pub target: Cents,
}

impl Progress {
    /// What the goal still needs.
    ///
    /// Clamped because goals overshoot, and a negative need would read as
    /// "needs funding" at every call site that compares against zero.
    pub fn shortfall(self) -> Cents {
        max_zero(self.target - self.current)
    }

    /// Whether the gate is open: its goal still needs something.
    pub fn needed(self) -> bool {
        self.shortfall() > Cents::ZERO
    }
}

/// What a gate finds at the goal it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateState {
    /// The gate names no goal, which is how one is switched off.
    #[default]
    Off,
    /// The gate names a goal that has ended. Nothing is drawn for it, and
    /// `transfer::plan` refuses the payday until it is pointed elsewhere: a
    /// closed goal's target is not one anybody is funding any more.
    Closed,
    Open(Progress),
}

impl GateState {
    /// What the gate draws for, at most.
    pub fn shortfall(self) -> Cents {
        match self {
            GateState::Open(progress) => progress.shortfall(),
            GateState::Off | GateState::Closed => Cents::ZERO,
        }
    }
}

/// Everything the waterfall reads from the ledger.
#[derive(Debug, Clone)]
pub struct PlanInputs {
    /// Checking balance at the ad-hoc projection date.
    pub checking_at_adhoc: Cents,
    /// The pinned excess, if the plan is pinned.
    pub pinned_excess: Option<Cents>,
    /// Monthly housing costs, summed into `Planning!E6`.
    pub housing_monthly: Vec<Cents>,
    /// Other monthly recurring bills, `Planning!E9:E12`.
    pub other_bills_monthly: Vec<Cents>,
    /// What each gate finds at its goal, by gate index.
    pub gates: [GateState; GATES],
    /// The gate indices, each once, highest priority first.
    pub gate_order: [usize; GATES],
}

/// One `Cents` per Planning line: the nine amounts a payday moves.
///
/// Plainly named and knowing nothing about destinations, because `calc` may
/// not name `plan_line::Line` — that type owns a `Key<GoalId>` and so lives
/// above the database boundary. `Line::amount` is the exhaustive match that
/// ties the enum to these fields.
///
/// Nested rather than prefixed: four of these names also name the waterfall
/// figure they derive from. `Plan::mom_and_dad` is the annual commitment
/// divided by pay periods; `Plan::lines.mom_and_dad` is that figure floored
/// to the dollar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lines {
    /// What each gate draws, by gate index.
    pub gates: [Cents; GATES],
    pub bills: Cents,
    pub current_housing: Cents,
    pub goals: Cents,
    pub future_housing: Cents,
    pub mom_and_dad: Cents,
    pub retirement: Cents,
    pub investment: Cents,
}

impl Lines {
    /// Every line, summed. Never more than `excess_used`, and equal to it
    /// for any excess the app can produce: the two lines that could once
    /// overdraw it are capped, and the Goals plug takes up whatever the
    /// flooring leaves.
    pub fn total(&self) -> Cents {
        self.gates.into_iter().sum::<Cents>()
            + self.bills
            + self.current_housing
            + self.goals
            + self.future_housing
            + self.mom_and_dad
            + self.retirement
            + self.investment
    }
}

/// Every line of the sheet, so the UI can render it and the tests can assert it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub excess_actual: Cents,
    pub excess_used: Cents,
    pub housing_biweekly: Cents,
    pub other_bills_biweekly: Cents,
    pub remaining_excess: Cents,
    pub bill_payments: Cents,
    pub mom_and_dad: Cents,
    pub remainder: Cents,

    pub goals: Cents,
    pub future_housing: Cents,
    pub retirement: Cents,
    pub investment: Cents,

    /// The gates as they were read, by gate index, and the priority order
    /// they drew in.
    pub gates: [GateState; GATES],
    pub gate_order: [usize; GATES],

    /// What each line moves, after the gates, the caps and the flooring.
    pub lines: Lines,

    /// What each line lost to the cap above it, line for line with
    /// [`Plan::lines`] — zero for every line that got what it asked for.
    ///
    /// Only the two fixed-bill lines can carry one. Everything below them is
    /// a share of `remaining_excess`, which is already nothing by the time a
    /// cap could bind, and the Goals plug is what is left rather than a claim
    /// on it. The three split shares are clamped too, but silently: that
    /// clamp only binds on a combination `Target::write` refuses, so there is
    /// no state for a screen to report.
    ///
    /// A `Lines` rather than a pair, so `plan_line::Line::amount` reaches a
    /// line's shortfall by the same exhaustive match it reaches its amount
    /// by, and `total` gives what the payday came up short by overall.
    pub shortfall: Lines,
}

/// `annual` spread over one pay period.
///
/// `periods` is clamped by the caller, which is what keeps the divide here
/// safe -- it comes from a user-editable setting.
pub(crate) fn per_period(annual: Cents, periods: i64) -> Cents {
    Cents(annual.0 / periods)
}

fn max_zero(value: Cents) -> Cents {
    if value < Cents::ZERO {
        Cents::ZERO
    } else {
        value
    }
}

/// Take up to `need` out of `shares`, emptying each before the next.
fn draw(shares: &mut [Cents], need: Cents) -> Cents {
    let mut taken = Cents::ZERO;
    for share in shares {
        let part = (*share).min(need - taken);
        *share = *share - part;
        taken += part;
    }
    taken
}

pub fn compute(settings: &PlanSettings, inputs: &PlanInputs) -> Result<Plan> {
    let excess_actual = max_zero(inputs.checking_at_adhoc - settings.target - settings.buffer);
    let excess_used = match inputs.pinned_excess {
        Some(pinned) => pinned,
        None => excess_actual.floor_to_dollar(),
    };

    // Never divide by a user-supplied zero: a nonsense pay-period count should
    // not take down the whole Planning screen.
    let periods_per_year = settings.periods_per_year.max(1);

    let bw = |monthly: &Vec<Cents>| -> Result<Cents> {
        monthly
            .iter()
            .map(|m| biweekly(*m, periods_per_year))
            .sum::<Result<Cents>>()
    };
    let housing_biweekly = bw(&inputs.housing_monthly)?;
    let other_bills_biweekly = bw(&inputs.other_bills_monthly)?;

    let remaining_excess = max_zero(excess_used - housing_biweekly - other_bills_biweekly);

    let bill_payments = settings
        .bill_payment_cap
        .min(settings.bill_payment_pct.of(remaining_excess));

    let mom_and_dad = max_zero(remaining_excess - bill_payments)
        .min(per_period(settings.mom_and_dad_annual, periods_per_year));

    let remainder = remaining_excess - bill_payments - mom_and_dad;

    let (goals, future_housing, retirement, investment) = if remainder <= settings.goals_floor {
        (max_zero(remainder), Cents::ZERO, Cents::ZERO, Cents::ZERO)
    } else {
        // Each share is capped by what the shares above it left, the same
        // rule the fixed bills follow below. It binds only when the three
        // total over 100%, which `Target::write` refuses -- so this is the
        // backstop for a database written before that rule, and Goals
        // saturates at nothing underneath it either way.
        let left = |taken: Cents| max_zero(remainder - taken);
        let fh = settings.future_housing_pct.of(remainder).min(remainder);
        let rt = settings.retirement_pct.of(remainder).min(left(fh));
        let inv = settings.investment_pct.of(remainder).min(left(fh + rt));
        (settings.goals_pct().of(remainder), fh, rt, inv)
    };

    // The excess cannot be spent twice. These two are the only lines that
    // could ever try: both are fixed biweekly figures that do not scale with
    // the excess, so a payday too small to cover them used to write transfers
    // totalling more than the account had to give.
    //
    // Housing is paid first. The waterfall is an ordered priority list and
    // `Mortgage + HOA` is the payment least able to wait, so `Bills` is what
    // takes the cut -- against the budget less the *floored* housing, so no
    // cents leak between the two.
    //
    // Floored to the dollar before the cap rather than after: `Bills` is then
    // capped by a whole number and stays one itself.
    let budget = max_zero(excess_used).floor_to_dollar();
    let housing_ask = housing_biweekly.floor_to_dollar();
    let bills_ask = (other_bills_biweekly + bill_payments).floor_to_dollar();
    let current_housing = housing_ask.min(budget);
    let bills = bills_ask.min(max_zero(budget - current_housing));
    // Each gate in turn takes what it needs out of the three shares, and what
    // none of them needs stays on the line the share belongs to. The need is
    // floored before it is drawn as well as the line after: cents drawn and
    // then floored away would come off a share's own line and reach no gate.
    let mut shares = [future_housing, retirement, investment];
    let mut gate_lines = [Cents::ZERO; GATES];
    for gate in inputs.gate_order {
        let need = inputs.gates[gate].shortfall().floor_to_dollar();
        gate_lines[gate] = draw(&mut shares, need).floor_to_dollar();
    }

    // What the gates left of the share, under one destination. Whether it is
    // a down payment or a mortgage payment is which account
    // `Line::FutureHousing` resolves to, which `calc` cannot see and does not
    // need to.
    let [future_housing_line, retirement_line, investment_line] =
        shares.map(Cents::floor_to_dollar);

    let mom_and_dad_line = mom_and_dad.floor_to_dollar();

    // Goals is the plug: it absorbs every floor above so the plan balances.
    //
    // Clamped at zero, which is now a formality rather than the thing holding
    // the plan together: every claim above is capped by what the excess had
    // left, so `claimed` never passes it. The clamp stays because a negative
    // allocation to a savings goal is not a thing whatever the arithmetic.
    let claimed = gate_lines.into_iter().sum::<Cents>()
        + bills
        + current_housing
        + future_housing_line
        + mom_and_dad_line
        + retirement_line
        + investment_line;
    let goals_line = max_zero(excess_used - claimed);

    let lines = Lines {
        gates: gate_lines,
        bills,
        current_housing,
        goals: goals_line,
        future_housing: future_housing_line,
        mom_and_dad: mom_and_dad_line,
        retirement: retirement_line,
        investment: investment_line,
    };

    let shortfall = Lines {
        bills: bills_ask - bills,
        current_housing: housing_ask - current_housing,
        ..Lines::default()
    };

    Ok(Plan {
        excess_actual,
        excess_used,
        housing_biweekly,
        other_bills_biweekly,
        remaining_excess,
        bill_payments,
        mom_and_dad,
        remainder,
        goals,
        future_housing,
        retirement,
        investment,
        gates: inputs.gates,
        gate_order: inputs.gate_order,
        lines,
        shortfall,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Cents;

    /// The gates by priority, which is all `calc` knows them by.
    const FIRST: usize = 0;
    const SECOND: usize = 1;

    /// Priority is the whole of what tells two gates apart: alone, either one
    /// leaves every other line the same at every need.
    #[test]
    fn gates_differ_in_nothing_but_their_priority() {
        let d = Cents::from_dollars;
        for need in [1, 1_000, 4_200, 5_000, 7_500, 50_000] {
            let alone = |gate: usize| {
                let mut inputs = plan_inputs();
                inputs.gates[gate] = unfunded(d(need));
                compute(&plan_settings(), &inputs).unwrap().lines
            };
            let (first, mut second) = (alone(FIRST), alone(SECOND));
            second.gates.swap(FIRST, SECOND);
            assert_eq!(first, second, "needing {need}");
        }
    }

    /// A gate whose goal is empty and needs `target`.
    fn unfunded(target: Cents) -> GateState {
        GateState::Open(Progress {
            current: Cents::ZERO,
            target,
        })
    }

    #[test]
    fn reversing_the_order_reverses_which_gate_draws_first() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates = [unfunded(d(8_000)), unfunded(d(3_000))];
        inputs.gate_order = [SECOND, FIRST];
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // 8400.87 of shares between them.
        assert_eq!(plan.lines.gates[SECOND], d(3_000));
        assert_eq!(plan.lines.gates[FIRST], d(5_400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_closed_or_switched_off_gate_draws_nothing() {
        let off = compute(&plan_settings(), &plan_inputs()).unwrap().lines;
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = GateState::Closed;
        assert_eq!(compute(&plan_settings(), &inputs).unwrap().lines, off);
        assert_eq!(off.gates, [Cents::ZERO; GATES]);
    }

    #[test]
    fn an_overfunded_gate_needs_nothing_rather_than_less_than_nothing() {
        let d = Cents::from_dollars;
        let over = Progress {
            current: d(1_200),
            target: d(1_000),
        };
        assert_eq!(over.shortfall(), Cents::ZERO);
        assert!(!over.needed());
    }

    /// Invented settings, chosen so each stage of the waterfall is actually
    /// exercised: the cap binds rather than the percentage, the Mom & Dad
    /// per-period figure lands between two dollars so the flooring shows,
    /// and the three shares leave a real slice for Goals.
    fn plan_settings() -> PlanSettings {
        let d = Cents::from_dollars;
        PlanSettings {
            target: d(10_000),
            buffer: d(5_000),
            periods_per_year: 26,
            bill_payment_cap: d(1_800),
            bill_payment_pct: BasisPoints(4_000),
            mom_and_dad_annual: d(12_000),
            goals_floor: d(400),
            future_housing_pct: BasisPoints(3_000),
            retirement_pct: BasisPoints(2_000),
            investment_pct: BasisPoints(1_000),
        }
    }

    /// The checking balance carries cents the pinned excess does not, so
    /// `excess_actual` and `excess_used` are distinguishable figures rather
    /// than the same one twice.
    fn plan_inputs() -> PlanInputs {
        let d = Cents::from_dollars;
        PlanInputs {
            checking_at_adhoc: Cents(3_250_075),
            pinned_excess: Some(d(17_500)),
            housing_monthly: vec![d(1_200), d(300)],
            other_bills_monthly: vec![d(90), d(60), d(25), d(1_000)],
            gates: Default::default(),
            gate_order: [FIRST, SECOND],
        }
    }

    /// The nine lines are the whole of the allocation: `lines.goals` is the
    /// plug that absorbs every other line's flooring, so the lines total
    /// exactly what there was to spend -- and with an excess ample enough to
    /// pay the fixed bills in full, no line is cut to get there.
    #[test]
    fn the_lines_account_for_every_dollar_of_the_excess_used() {
        let plan = compute(&plan_settings(), &plan_inputs()).unwrap();
        assert_eq!(plan.lines.total(), plan.excess_used);
        assert_eq!(plan.shortfall.total(), Cents::ZERO);
    }

    /// Future Housing is the whole `future_housing_pct` share under one
    /// destination: `plan.future_housing` is the raw share, and
    /// `lines.future_housing` floors that whole share to the dollar exactly
    /// once, at the line itself rather than at any component of it.
    #[test]
    fn future_housing_is_the_whole_share_floored_once() {
        let settings = plan_settings();
        let plan = compute(&settings, &plan_inputs()).unwrap();
        let share = settings.future_housing_pct.of(plan.remainder);
        assert_eq!(plan.future_housing, share);
        assert_eq!(plan.lines.future_housing, share.floor_to_dollar());
    }

    /// A first-gate need past every share zeros the future housing, retirement,
    /// and investment lines and pours the same three amounts into the first
    /// gate's line instead, and the lines still total the excess used.
    #[test]
    fn a_first_gate_need_past_every_share_sweeps_them_into_its_line() {
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = unfunded(Cents::from_dollars(50_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();

        assert!(plan.gates[FIRST].shortfall() > Cents::ZERO);
        assert_eq!(plan.lines.future_housing, Cents::ZERO);
        assert_eq!(plan.lines.retirement, Cents::ZERO);
        assert_eq!(plan.lines.investment, Cents::ZERO);
        assert_eq!(
            plan.lines.gates[FIRST],
            (plan.future_housing + plan.retirement + plan.investment).floor_to_dollar()
        );
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    /// Every stage of the waterfall, from one set of inputs, against the
    /// cell of sheet `Planning` it reproduces. Each figure is derived by
    /// hand from `plan_settings` and `plan_inputs` rather than read back out
    /// of `compute`, which is what makes this a check rather than a
    /// snapshot.
    #[test]
    fn every_stage_of_the_waterfall_falls_out_of_one_set_of_inputs() {
        let d = Cents::from_dollars;
        let plan = compute(&plan_settings(), &plan_inputs()).unwrap();

        // 32,500.75 checking, less the 10,000 target and 5,000 buffer.
        assert_eq!(plan.excess_actual, Cents(1_750_075)); // D2
        assert_eq!(plan.excess_used, d(17_500)); // D3  the pin
        // 1,200 and 300 monthly, each ceilinged to the dollar: 554 + 139.
        assert_eq!(plan.housing_biweekly, d(693)); // E6
        // 90, 60, 25, 1,000 monthly: 42 + 28 + 12 + 462.
        assert_eq!(plan.other_bills_biweekly, d(544)); // E9:E12
        assert_eq!(plan.remaining_excess, d(16_263)); // D14
        // 40% of 16,263 is 6,505.20, so the 1,800 cap binds.
        assert_eq!(plan.bill_payments, d(1_800)); // D19
        assert_eq!(plan.mom_and_dad, Cents(46_153)); // D20  12,000 / 26
        assert_eq!(plan.remainder, Cents(1_400_147)); // D22  14,001.47

        assert_eq!(plan.lines.bills, d(2_344)); // D30
        assert_eq!(plan.lines.current_housing, d(693)); // D31
        assert_eq!(plan.lines.goals, d(5_602)); // D32  (the plug)
        assert_eq!(plan.lines.gates[SECOND], Cents::ZERO); // D33
        assert_eq!(
            plan.lines.bills
                + plan.lines.current_housing
                + plan.lines.goals
                + plan.lines.gates[SECOND],
            d(8_639)
        ); // D29

        // 30% of the remainder, floored once at the line.
        assert_eq!(plan.lines.future_housing, d(4_200));
        assert_eq!(plan.lines.mom_and_dad, d(461)); // D36
        assert_eq!(plan.lines.gates[FIRST], Cents::ZERO); // D37
        assert_eq!(
            plan.lines.future_housing + plan.lines.mom_and_dad + plan.lines.gates[FIRST],
            d(4_661)
        ); // D34

        assert_eq!(plan.lines.retirement, d(2_800)); // D39
        assert_eq!(plan.lines.investment, d(1_400)); // D40
        assert_eq!(plan.lines.retirement + plan.lines.investment, d(4_200)); // D38

        assert_eq!(plan.lines.total(), plan.excess_used); // D42 = the green tick
    }

    #[test]
    fn the_lines_account_for_the_excess_used_when_unpinned_too() {
        let mut inputs = plan_inputs();
        inputs.pinned_excess = None;
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // Unpinned floors the live excess itself, giving the same figure here.
        assert_eq!(plan.excess_used, Cents::from_dollars(17_500));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_small_remainder_goes_entirely_to_goals() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        // 2,600 less 1,237 of fixed bills leaves 1,363, of which bill
        // payments take 40% (545.20) and Mom & Dad 461.53, leaving 356.27 --
        // under the 400 floor.
        inputs.pinned_excess = Some(d(2_600));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        assert_eq!(plan.remainder, Cents(35_627));
        assert!(plan.remainder <= d(400));
        assert_eq!(plan.future_housing, Cents::ZERO);
        assert_eq!(plan.retirement, Cents::ZERO);
        assert_eq!(plan.investment, Cents::ZERO);
        assert_eq!(plan.goals, plan.remainder);
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_first_gate_need_past_every_share_leaves_the_second_nothing() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = unfunded(d(50_000));
        // Non-zero, so the second gate would otherwise draw too: the
        // assertion below is that the first wins the priority order, not that
        // there is nothing for the second to claim.
        inputs.gates[SECOND] = unfunded(d(50_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // Nothing is left for the second gate or any share's own line.
        assert_eq!(plan.lines.gates[SECOND], Cents::ZERO);
        assert_eq!(plan.lines.future_housing, Cents::ZERO);
        assert_eq!(plan.lines.retirement + plan.lines.investment, Cents::ZERO);
        // Future Housing, Retirement, and Investment pile into the first gate.
        // 4200.44 + 2800.29 + 1400.14 = 8400.87, floored once after summing.
        assert_eq!(plan.lines.gates[FIRST], d(8_400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_first_gate_need_of_a_dollar_takes_a_dollar() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = unfunded(d(1));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // 1 of the 4200.44 of future housing; the other two are whole.
        assert_eq!(plan.lines.gates[FIRST], d(1));
        assert_eq!(plan.lines.future_housing, d(4_199));
        assert_eq!(plan.lines.retirement, d(2_800));
        assert_eq!(plan.lines.investment, d(1_400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_gate_needing_less_than_a_dollar_takes_nothing_from_any_line() {
        let off = compute(&plan_settings(), &plan_inputs()).unwrap().lines;
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = unfunded(Cents(99));
        assert_eq!(compute(&plan_settings(), &inputs).unwrap().lines, off);
    }

    #[test]
    fn the_second_gate_fills_from_what_the_first_leaves() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[FIRST] = unfunded(d(5_000));
        inputs.gates[SECOND] = unfunded(d(3_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // The first takes 4200.44 of future housing and 799.56 of
        // retirement; the second takes the 2000.73 left of that and 999.27 of
        // the 1400.14 of investment.
        assert_eq!(plan.lines.gates[FIRST], d(5_000));
        assert_eq!(plan.lines.gates[SECOND], d(3_000));
        assert_eq!(plan.lines.future_housing, Cents::ZERO);
        assert_eq!(plan.lines.retirement, Cents::ZERO);
        assert_eq!(plan.lines.investment, d(400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_gate_spends_the_future_housing_share_before_the_others() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[SECOND] = unfunded(d(1_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // 1,000 of the 4200.44 of future housing; the other two are whole.
        assert_eq!(plan.lines.gates[SECOND], d(1_000));
        assert_eq!(plan.lines.future_housing, d(3_200));
        assert_eq!(plan.lines.retirement, d(2_800));
        assert_eq!(plan.lines.investment, d(1_400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_gate_splits_retirement_between_its_own_and_the_retirement_line() {
        let d = Cents::from_dollars;
        let mut settings = plan_settings();
        settings.future_housing_pct = BasisPoints::ZERO;
        let mut inputs = plan_inputs();
        inputs.gates[SECOND] = unfunded(d(1_000));
        let plan = compute(&settings, &inputs).unwrap();
        // Retirement is 2800.29: 1,000 fills the gate, the rest stays on the
        // Retirement line.
        assert_eq!(plan.lines.gates[SECOND], d(1_000));
        assert_eq!(plan.lines.retirement, d(1_800));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    /// With neither share ahead of it a gate still fills,
    /// out of the investment share, and only the balance is invested.
    #[test]
    fn a_gate_fills_from_the_investment_share_when_it_is_the_only_one() {
        let d = Cents::from_dollars;
        let mut settings = plan_settings();
        settings.future_housing_pct = BasisPoints::ZERO;
        settings.retirement_pct = BasisPoints::ZERO;
        let mut inputs = plan_inputs();
        inputs.gates[SECOND] = unfunded(d(1_000));
        let plan = compute(&settings, &inputs).unwrap();
        // Investment is 1400.14.
        assert_eq!(plan.lines.gates[SECOND], d(1_000));
        assert_eq!(plan.lines.retirement, Cents::ZERO);
        assert_eq!(plan.lines.investment, d(400));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_gate_spends_the_retirement_share_before_the_investment_share() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[SECOND] = unfunded(d(7_200));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // 4200.44 of future housing and 2800.29 of retirement, then 199.27
        // of the 1400.14 of investment.
        assert_eq!(plan.lines.gates[SECOND], d(7_200));
        assert_eq!(plan.lines.future_housing, Cents::ZERO);
        assert_eq!(plan.lines.retirement, Cents::ZERO);
        assert_eq!(plan.lines.investment, d(1_200));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    #[test]
    fn a_second_gate_need_past_every_share_takes_all_of_each_and_nothing_else() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.gates[SECOND] = unfunded(d(50_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();
        // 4200.44 + 2800.29 + 1400.14 = 8400.87, floored once after summing.
        assert_eq!(plan.lines.gates[SECOND], d(8_400));
        assert_eq!(plan.lines.retirement, Cents::ZERO);
        assert_eq!(plan.lines.investment, Cents::ZERO);
        assert_eq!(plan.lines.future_housing, Cents::ZERO);
        assert_eq!(plan.lines.mom_and_dad, d(461));
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    /// The waterfall is an ordered priority list, and `Mortgage + HOA` is the
    /// payment least able to wait: it is paid in full while there is anything
    /// to pay it with, and `Bills` takes the cut.
    #[test]
    fn an_excess_short_of_the_fixed_bills_pays_housing_first_and_cuts_bills() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        // 693 of housing and 544 of other bills, against 1,000.
        inputs.pinned_excess = Some(d(1_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();

        assert_eq!(plan.lines.current_housing, d(693));
        assert_eq!(plan.lines.bills, d(307));
        assert_eq!(plan.lines.total(), d(1_000));
    }

    /// What the cut line lost, so the screen has a figure to draw beside it.
    /// The line's own amount is right either way; nothing else says it was
    /// meant to be larger.
    #[test]
    fn a_cut_line_records_what_it_lost() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.pinned_excess = Some(d(1_000));
        let plan = compute(&plan_settings(), &inputs).unwrap();

        assert_eq!(plan.shortfall.bills, d(237));
        assert_eq!(plan.shortfall.current_housing, Cents::ZERO);
        assert_eq!(plan.shortfall.total(), d(237));
    }

    /// An excess of nothing is not "pay the bills anyway": every line the
    /// excess cannot cover is a transfer that would overdraw the account it
    /// comes out of.
    #[test]
    fn an_excess_of_nothing_moves_nothing_and_both_bill_lines_report_it() {
        let d = Cents::from_dollars;
        let mut inputs = plan_inputs();
        inputs.checking_at_adhoc = d(1_000);
        inputs.pinned_excess = None;
        let plan = compute(&plan_settings(), &inputs).unwrap();

        assert_eq!(plan.excess_used, Cents::ZERO);
        assert_eq!(plan.lines.total(), Cents::ZERO);
        assert_eq!(plan.shortfall.current_housing, d(693));
        assert_eq!(plan.shortfall.bills, d(544));
    }

    /// The one rule the whole waterfall answers to. Swept across the excess
    /// rather than asserted at one figure, because the interesting values are
    /// the ones either side of the fixed bills' own total.
    #[test]
    fn the_lines_never_total_more_than_the_excess_used() {
        let d = Cents::from_dollars;
        for pinned in [0, 1, 500, 693, 1_000, 1_237, 1_238, 5_000, 17_500] {
            let mut inputs = plan_inputs();
            inputs.pinned_excess = Some(d(pinned));
            let plan = compute(&plan_settings(), &inputs).unwrap();
            assert!(
                plan.lines.total() <= plan.excess_used,
                "pinned {pinned}: lines total {} against an excess of {}",
                plan.lines.total(),
                plan.excess_used
            );
        }
    }

    /// `Target::write` refuses a combination over 100%, so this only ever
    /// binds on a database written before that rule -- but bind it must, or
    /// three shares of a healthy remainder overdraw it between them. Clamped
    /// in the order the screen lists them, the same rule the fixed bills
    /// follow, and silently: the state cannot be reached through the app, so
    /// there is nothing here for a screen to report.
    #[test]
    fn splits_totalling_over_one_hundred_are_clamped_in_order() {
        let mut settings = plan_settings();
        settings.future_housing_pct = BasisPoints(6_000);
        settings.retirement_pct = BasisPoints(3_000);
        settings.investment_pct = BasisPoints(3_000); // 120 between them
        let plan = compute(&settings, &plan_inputs()).unwrap();

        assert_eq!(plan.goals, Cents::ZERO);
        assert_eq!(
            plan.future_housing + plan.retirement + plan.investment,
            plan.remainder
        );
        assert_eq!(plan.lines.total(), plan.excess_used);
    }

    /// `periods_per_year` is the only divisor left that a setting can zero,
    /// and `compute` clamps it, so a zero must still produce a plan rather
    /// than surfacing `div_ceil`'s error. The figures are not meaningful
    /// here; completing the call is the point.
    #[test]
    fn a_zero_pay_period_count_still_produces_a_plan() {
        let mut settings = plan_settings();
        settings.periods_per_year = 0;
        assert!(compute(&settings, &plan_inputs()).is_ok());
    }
}
