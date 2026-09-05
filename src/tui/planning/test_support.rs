//! The fixtures four of these modules' tests share: one plan, one wiring, and
//! the screen built from them. `confirm` is the one that does not read it --
//! its rows are the transfers a payday would write, which it builds itself.
//!
//! Here rather than in each module for the reason `tui::app::test_support`
//! gives: these are one database's worth of figures, and a copy per module
//! would be free to drift from the assertions the other modules make against
//! it. What a module *chose* still stays in the module.

use super::{Planning, Row, View};
use crate::calc::planning::{Plan, PlanInputs, PlanSettings, compute};
use crate::db::bill::{Bill, Category};
use crate::db::{AccountId, BillId};
use crate::money::Cents;
use crate::plan_line::Line;
use crate::rate::Percent;
use crate::transfer::{self, Container, Landing, Wiring};
use chrono::NaiveDate;

pub(super) fn settings() -> PlanSettings {
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

pub(super) fn bill(id: i64, label: &str, dollars: i64, category: Category, sort: i64) -> Bill {
    Bill {
        id: BillId(id),
        label: label.to_string(),
        cents: Cents::from_dollars(dollars),
        category,
        sort,
    }
}

pub(super) fn housing() -> Vec<Bill> {
    vec![
        bill(1, "Mortgage", 1_200, Category::Housing, 0),
        bill(2, "HOA", 300, Category::Housing, 1),
    ]
}

pub(super) fn other_bills() -> Vec<Bill> {
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
pub(super) fn transfers(plan: &Plan) -> Vec<transfer::Row> {
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

pub(super) fn goal(name: &str, container: i64) -> crate::db::goal::Goal {
    crate::db::goal::Goal {
        id: crate::db::GoalId(1),
        name: name.to_string(),
        container_account_id: crate::db::AccountId(container),
        base_cents: Cents::from_dollars(1_000),
        goal_date: None,
        recurring_goal_id: None,
        interest_eligible: true,
        closed: false,
        sort: 0,
        favorite: false,
        taxed: false,
        floating: false,
    }
}

pub(super) fn wired(line: Line, landing: Landing) -> Wiring {
    Wiring {
        line,
        landing,
        suggestion: None,
    }
}

/// A container by name, with an invented id so the two in this fixture
/// are distinguishable -- the screen tints by id, so two containers
/// sharing one would stop the tint tests saying anything.
pub(super) fn container(name: &str) -> Container {
    Container {
        id: AccountId(if name == "Brokerage" { 2 } else { 1 }),
        name: name.to_string(),
        color: None,
    }
}

pub(super) fn in_goal(name: &str, container_name: &str) -> Landing {
    Landing::Goal {
        goal: name.to_string(),
        container: container(container_name),
    }
}

/// The owner's own database, a fortnight after the destination keys were
/// added: everything the import matched is pointed somewhere, and the one
/// line whose key that import predates is unset with its goal sitting
/// there unclaimed.
pub(super) fn wiring() -> Vec<Wiring> {
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
pub(super) fn view(pinned: Option<Cents>, pinned_at: Option<NaiveDate>) -> View {
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
        spread_ask_total: Cents::ZERO,
        transfer_error: None,
        transfer_detail: Vec::new(),
    }
}

/// The screen with one destination row replaced, for the states a
/// healthy database does not hold.
pub(super) fn screen_with(line: Line, landing: Landing) -> Planning {
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

pub(super) fn screen() -> Planning {
    let mut planning = Planning::new();
    planning
        .set_view(view(Some(Cents::from_dollars(17_500)), None))
        .unwrap();
    planning
}

pub(super) fn row<'a>(planning: &'a Planning, label: &str) -> &'a Row {
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
pub(super) fn destination(planning: &Planning, line: Line) -> &Row {
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

/// The transfers block repeats labels the screen uses elsewhere -- the
/// Split section and the Destinations block both carry a "Goals" row --
/// so a line of it is found by walking down from the heading, the way
/// [`destination`] walks down from its own.
pub(super) fn transfer_line(planning: &Planning, line: Line) -> &Row {
    let start = planning
        .rows()
        .iter()
        .position(|r| r.label == "Transfers")
        .expect("no Transfers heading");
    planning.rows()[start..]
        .iter()
        .take_while(|r| !(r.label.is_empty() && r.value.is_empty()))
        .find(|r| r.label.trim() == line.label())
        .unwrap_or_else(|| panic!("no transfer line for {line:?}"))
}
