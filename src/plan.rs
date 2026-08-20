use crate::calc::planning::{self, PlanInputs, PlanSettings};
use crate::db::account;
use crate::db::setting::key;
use crate::db::{Db, bill, goal, setting, txn};
use crate::gate::Gate;
use crate::money::Cents;
use crate::rate::Percent;
use anyhow::{Context, Result};
use chrono::NaiveDate;

/// The outstanding need behind one Planning gate.
///
/// Two different failures, treated differently. An **unset** setting means
/// the gate is not configured for this database, so it is off. A setting
/// pointing at a goal that no longer exists is a **corrupt** database, and
/// is an error rather than a silently disabled gate: the gates decide
/// where roughly half the waterfall's money goes, and one switching itself
/// off unnoticed is the bug this indirection exists to prevent.
fn remaining(db: &Db, gate: Gate) -> Result<Cents> {
    let key = gate.key();
    match setting::get(db, key)? {
        None => Ok(Cents::ZERO),
        Some(id) => goal::shortfall(db, id).with_context(|| format!("setting {key} = {id}")),
    }
}

/// The tuned constants, as stored -- each with the default the waterfall has
/// always used when its key is unset.
///
/// Split out of [`compute_from_db`] because the Planning screen renders these
/// beside the figures they produce, and a second copy of the defaults in the
/// UI would be a second place for them to be wrong.
pub fn settings_from_db(db: &Db) -> Result<PlanSettings> {
    let dollars = Cents::from_dollars;
    Ok(PlanSettings {
        target: setting::get_or(db, key::PLANNING_TARGET, dollars(10_000))?,
        buffer: setting::get_or(db, key::PLANNING_BUFFER, dollars(5_000))?,
        periods_per_year: setting::get_or(db, key::PAY_PERIODS_PER_YEAR, 26)?,
        bill_payment_cap: setting::get_or(db, key::BILL_PAYMENT_CAP, dollars(1_800))?,
        bill_payment_pct: setting::get_or(db, key::BILL_PAYMENT_PCT, Percent(40))?,
        mom_and_dad_annual: setting::get_or(db, key::MOM_AND_DAD_ANNUAL, dollars(12_000))?,
        goals_floor: setting::get_or(db, key::GOALS_FLOOR, dollars(400))?,
        future_housing_pct: setting::get_or(db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(30))?,
        retirement_pct: setting::get_or(db, key::SPLIT_RETIREMENT_PCT, Percent(20))?,
        investment_pct: setting::get_or(db, key::SPLIT_INVESTMENT_PCT, Percent(10))?,
    })
}

/// Run the Planning waterfall against imported settings and balances.
///
/// `adhoc` is the date the checking balance is quoted at -- [`crate::projection::Dates::adhoc`]
/// as derived, or wherever the Overview's scrub has moved it to. The caller
/// supplies it rather than the waterfall re-deriving it, so a scrubbed screen
/// and the payday it writes cannot disagree about which day they mean.
pub fn compute_from_db(db: &Db, adhoc: NaiveDate) -> Result<planning::Plan> {
    let checking = account::checking(db)?.id;

    let settings = settings_from_db(db)?;

    let inputs = PlanInputs {
        checking_at_adhoc: txn::balance_at(db, checking, adhoc)?,
        pinned_excess: setting::get(db, key::PINNED_EXCESS)?,
        // `calc` still takes two bare lists: the table is a UI and importer
        // concern, and the waterfall is unchanged by this stage.
        housing_monthly: bill::amounts(db, bill::Category::Housing)?,
        other_bills_monthly: bill::amounts(db, bill::Category::Other)?,
        remaining_emergency: remaining(db, Gate::EmergencyFund)?,
        remaining_roth: remaining(db, Gate::Roth)?,
    };

    planning::compute(&settings, &inputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::GoalId;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::NewGoal;

    /// A goal the database does not have configured must leave its gate off,
    /// so a hand-built database -- or a workbook with no Roth row -- can
    /// still compute a plan.
    #[test]
    fn an_unset_gate_setting_leaves_the_gate_off() {
        let db = db::open_in_memory().unwrap();
        assert_eq!(remaining(&db, Gate::Roth).unwrap(), Cents::ZERO);
    }

    #[test]
    fn a_gate_setting_reports_its_goals_outstanding_need() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &NewGoal {
                name: "Roth IRA".to_string(),
                container_account_id: savings,
                goal_cents: Cents::from_dollars(5_500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(2_000),
            None,
            None,
        )
        .unwrap();
        setting::set(&db, Gate::Roth.key(), id).unwrap();

        assert_eq!(
            remaining(&db, Gate::Roth).unwrap(),
            Cents::from_dollars(3_500)
        );
    }

    /// A recorded id pointing at a goal that is gone must fail loudly and
    /// name the setting, so the fix is obvious. Silently reporting zero would
    /// turn the gate off and change where every discretionary dollar goes,
    /// with no signal beyond a healthy exit code.
    #[test]
    fn a_dangling_gate_setting_is_an_error_naming_the_key() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, Gate::EmergencyFund.key(), GoalId(999)).unwrap();

        let err = remaining(&db, Gate::EmergencyFund).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("planning.goal.emergency_id"), "{text}");
        assert!(text.contains("999"), "{text}");
    }

    /// `compute_from_db` wires two setting keys to `remaining_emergency` and
    /// `remaining_roth`. Nothing else in the test suite would notice if
    /// those two wires were crossed: every other test either hand-builds
    /// `PlanInputs` directly (`calc::planning`) or runs against the real
    /// workbook, where both shortfalls happen to be zero. A transposition
    /// there would still pass every existing test, including under
    /// `MM_REQUIRE_WORKBOOK=1`, while quietly sending money to the wrong
    /// bucket. This test goes through `compute_from_db` end to end with two
    /// goals of distinct, non-zero shortfalls -- distinct so a swapped
    /// mapping produces a visibly wrong number instead of coincidentally
    /// matching -- and checks both the `need_*` flags and a downstream
    /// amount fed by `remaining_roth`.
    ///
    /// Emergency is funded exactly to its target (shortfall zero) so its gate
    /// stays off; `compute` shuts the Roth line off entirely whenever
    /// `need_emergency` is true, which would make it insensitive to a swap
    /// with Emergency.
    ///
    /// The Home Down Payment goal is set up below too, for realism, but
    /// nothing reads its shortfall into a `PlanInputs` field.
    #[test]
    fn compute_from_db_wires_each_gate_setting_to_its_own_plan_input() {
        let db = db::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        txn::insert(
            &db,
            &txn::NewTxn {
                date: today,
                cents: Cents::from_dollars(50_000),
                account_id: checking,
                description: "opening balance".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();

        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();

        // Emergency: funded exactly to target, so its shortfall -- and its
        // gate -- is zero.
        let emergency = goal::insert(
            &db,
            &NewGoal {
                name: "Emergency Savings".to_string(),
                container_account_id: brokerage,
                goal_cents: Cents::from_dollars(100_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            emergency,
            today,
            Cents::from_dollars(100_000),
            None,
            None,
        )
        .unwrap();

        // Roth: $1,000 outstanding.
        let roth = goal::insert(
            &db,
            &NewGoal {
                name: "Roth IRA".to_string(),
                container_account_id: savings,
                goal_cents: Cents::from_dollars(5_500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, roth, today, Cents::from_dollars(4_500), None, None).unwrap();

        // Home Down Payment: a goal present in the database for realism.
        // No gate or `PlanInputs` field reads its shortfall -- Future
        // Housing is a destination, resolved through `plan_line::Line`,
        // not a value `compute_from_db` computes.
        let down_payment = goal::insert(
            &db,
            &NewGoal {
                name: "Home Down Payment".to_string(),
                container_account_id: brokerage,
                goal_cents: Cents::from_dollars(500_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: false,
                sort: 1,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            down_payment,
            today,
            Cents::from_dollars(500_000),
            None,
            None,
        )
        .unwrap();

        setting::set(&db, Gate::EmergencyFund.key(), emergency).unwrap();
        setting::set(&db, Gate::Roth.key(), roth).unwrap();

        let settings = settings_from_db(&db).unwrap();
        let plan = compute_from_db(&db, today).unwrap();

        assert!(!plan.need_emergency);
        assert!(plan.need_roth);
        // `lines.roth` reports the Roth's own shortfall ($1,000) rather than
        // being clamped by it, since the computed retirement share comfortably
        // exceeds it -- a wire crossed with Emergency (whose shortfall is
        // zero) would report a different figure here.
        assert_eq!(plan.lines.roth, Cents::from_dollars(1_000));
        // Future Housing is not fed by any gate, so it is one whole share of
        // the remainder, floored.
        assert_eq!(
            plan.lines.future_housing,
            settings
                .future_housing_pct
                .of(plan.remainder)
                .floor_to_dollar()
        );
    }

    /// The two bill categories reach two different lines of the waterfall, and
    /// only housing reaches `lines.current_housing`. Reading them from one table means
    /// the category column is the only thing keeping them apart, so a
    /// transposition here is a plausible bug that nothing else would catch.
    #[test]
    fn compute_from_db_reads_each_bill_category_into_its_own_waterfall_line() {
        use crate::db::bill::{self, Category, NewBill};

        let db = db::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        txn::insert(
            &db,
            &txn::NewTxn {
                date: today,
                cents: Cents::from_dollars(50_000),
                account_id: checking,
                description: "opening balance".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();

        let bill = |label: &str, dollars: i64, category, sort| NewBill {
            label: label.to_string(),
            cents: Cents::from_dollars(dollars),
            category,
            sort,
        };
        bill::insert(&db, &bill("Mortgage", 1_200, Category::Housing, 0)).unwrap();
        bill::insert(&db, &bill("HOA", 300, Category::Housing, 1)).unwrap();
        bill::insert(&db, &bill("Coworking", 1_000, Category::Other, 0)).unwrap();

        let plan = compute_from_db(&db, today).unwrap();

        // 1,200 and 300 monthly over 26 periods: 554 + 139. `calc::biweekly`
        // rounds each up to a whole dollar before summing.
        assert_eq!(plan.housing_biweekly, Cents::from_dollars(693));
        assert_eq!(plan.other_bills_biweekly, Cents::from_dollars(462));
    }

    /// The screen renders every tuned constant beside the figure it produces,
    /// so the defaults have to be readable without recomputing the plan -- and
    /// they must be the same defaults `compute_from_db` uses.
    #[test]
    fn settings_from_db_falls_back_to_the_same_defaults_the_waterfall_uses() {
        let db = db::open_in_memory().unwrap();
        let settings = settings_from_db(&db).unwrap();
        assert_eq!(settings.target, Cents::from_dollars(10_000));
        assert_eq!(settings.buffer, Cents::from_dollars(5_000));
        assert_eq!(settings.periods_per_year, 26);
        assert_eq!(settings.future_housing_pct, Percent(30));

        setting::set(&db, key::PLANNING_TARGET, Cents::from_dollars(9_000)).unwrap();
        assert_eq!(
            settings_from_db(&db).unwrap().target,
            Cents::from_dollars(9_000)
        );
    }
}
