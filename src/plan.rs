use crate::calc::planning::{self, PlanInputs, PlanSettings};
use crate::db::account;
use crate::db::setting::key;
use crate::db::{Db, bill, setting, txn};
use crate::gate::Gate;
use crate::goal as goal_engine;
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
        Some(id) => goal_engine::shortfall(db, id).with_context(|| format!("setting {key} = {id}")),
    }
}

/// The tuned constants, as stored -- each with the default the waterfall has
/// always used when its key is unset.
///
/// Read here rather than inside [`compute_from_db`] because both sinks that
/// draw the waterfall render these beside the figures they produce: one read
/// per sink, handed on to the waterfall, so a second copy of the defaults in
/// the UI cannot be a second place for them to be wrong and neither key is
/// read twice to draw one screen.
pub fn settings_from_db(db: &Db) -> Result<PlanSettings> {
    let dollars = Cents::from_dollars;
    // Ten keys off one `SELECT` rather than ten prepared statements. Both
    // sinks that draw the waterfall come through here on every render, and
    // ten round-trips were ten answers to one question -- `setting::Snapshot`
    // is read through the same `Key<T>` constants, so nothing about which key
    // holds which type is given up to buy them back.
    let s = setting::all(db)?;
    Ok(PlanSettings {
        target: s.get_or(key::PLANNING_TARGET, dollars(10_000))?,
        buffer: s.get_or(key::PLANNING_BUFFER, dollars(5_000))?,
        periods_per_year: s.get_or(key::PAY_PERIODS_PER_YEAR, 26)?,
        bill_payment_cap: s.get_or(key::BILL_PAYMENT_CAP, dollars(1_800))?,
        bill_payment_pct: s.get_or(key::BILL_PAYMENT_PCT, Percent(40))?,
        mom_and_dad_annual: s.get_or(key::MOM_AND_DAD_ANNUAL, dollars(12_000))?,
        goals_floor: s.get_or(key::GOALS_FLOOR, dollars(400))?,
        future_housing_pct: s.get_or(key::SPLIT_FUTURE_HOUSING_PCT, Percent(30))?,
        retirement_pct: s.get_or(key::SPLIT_RETIREMENT_PCT, Percent(20))?,
        investment_pct: s.get_or(key::SPLIT_INVESTMENT_PCT, Percent(10))?,
    })
}

/// Refuse a set of splits that leaves Goals less than nothing.
///
/// Goals takes what the other three leave -- `100 - (fh + rt + inv)`,
/// saturating at zero -- so a set totalling over 100 hands every discretionary
/// dollar to the other three and Goals nothing at all. `calc::planning` clamps
/// each share against what the ones above it left, but that is a backstop: the
/// figures it produces are not the ones the percentages claim, and nothing on
/// any screen says so.
///
/// Both bounds, because a cell is not a field. The form's writer is two
/// refusals -- `tui::planning::parse_percent` will not let a percentage
/// outside `0..=100` be typed at all, and `write_split` refuses the set it
/// would join -- but `import::cell::as_percent` reads whatever the sheet
/// carries and does not clamp, so an import stating only the set would let a
/// `150 / -60 / 5` totalling 95 through a rule written for `60 / 30 / 30`.
/// `Percent::of` does not clamp either, so what that reaches is
/// `calc::planning` handing a line a negative share: `transfer::plan` skips a
/// line at zero and nothing anywhere reads its sign, so it would be written
/// as a real transfer instruction moving money the wrong way.
///
/// The set is checked second because the per-field rule is what makes it mean
/// anything: three shares that sum inside 100 by cancelling each other out is
/// the arithmetic the set rule assumes cannot happen.
///
/// Read through [`settings_from_db`] so an unset key counts as the default the
/// waterfall will use rather than as nothing.
pub fn check_splits(db: &Db) -> Result<()> {
    let s = settings_from_db(db)?;
    let (fh, rt, inv) = (
        s.future_housing_pct.0,
        s.retirement_pct.0,
        s.investment_pct.0,
    );
    for (label, pct) in [
        ("Future Housing", fh),
        ("Retirement", rt),
        ("Investment", inv),
    ] {
        anyhow::ensure!(
            (0..=100).contains(&pct),
            "the {label} split is {pct}%, which is not a share of anything: \
             a share below zero allocates its line backwards, and one past a \
             hundred claims what the excess never had"
        );
    }
    anyhow::ensure!(
        fh + rt + inv <= 100,
        "the three splits total {}%, which leaves Goals nothing: \
         Future Housing {fh}%, Retirement {rt}%, Investment {inv}%",
        fh + rt + inv
    );
    Ok(())
}

/// Refuse a pinned excess the waterfall cannot run off.
///
/// Two bounds, and the waterfall breaks differently under each. A pin below
/// zero leaves `budget` clamped at nothing while `excess_used` is negative, so
/// every line floors to zero and `lines.total() <= excess_used` -- the one rule
/// the whole waterfall answers to -- is false with nothing spent. A pin
/// carrying cents has the drift line under the plan quoting a difference that
/// is not one, since what that line reads is the cents `excess_used`'s own
/// floor drops; the `Excess (Used)` row would then refuse its own prefill.
///
/// The stored figure, because that is the shape an import writes.
/// `tui::planning::parse_pinned_excess` holds the same two bounds on text
/// typed into the row, which is the shape the form writes; both exist because
/// both are writers, and the sheet's `Excess (Fixed)` cell would otherwise put
/// a database straight into the state the row refuses.
pub fn check_pinned_excess(db: &Db) -> Result<()> {
    let Some(pinned) = setting::get(db, key::PINNED_EXCESS)? else {
        return Ok(());
    };
    anyhow::ensure!(
        pinned >= Cents::ZERO,
        "the pinned excess is {pinned}, and a waterfall run off a figure below \
         zero allocates nothing while reporting it as spent"
    );
    anyhow::ensure!(
        pinned.0 % 100 == 0,
        "the pinned excess {pinned} is not a whole number of dollars, which \
         the drift line under the plan would read as drift"
    );
    Ok(())
}

/// Run the Planning waterfall against imported settings and balances.
///
/// `adhoc` is the date the checking balance is quoted at -- [`crate::projection::Dates::adhoc`]
/// as derived, or wherever the Overview's scrub has moved it to. The caller
/// supplies it rather than the waterfall re-deriving it, so a scrubbed screen
/// and the payday it writes cannot disagree about which day they mean.
///
/// `settings` is supplied too, for a plainer reason: both sinks that draw the
/// plan draw the constants beside it, so a waterfall reading them itself
/// would read every key twice per screen.
pub fn compute_from_db(
    db: &Db,
    settings: &PlanSettings,
    adhoc: NaiveDate,
) -> Result<planning::Plan> {
    let checking = account::checking(db)?.id;

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

    planning::compute(settings, &inputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::GoalId;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::{self, NewGoal};

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
                base_cents: Cents::from_dollars(5_500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
                floating: false,
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

    /// A gate over a taxed goal is not satisfied until the *taxed* figure is
    /// funded. Gating on the base would open the waterfall's next tier while
    /// the goal is still short of what the item costs.
    #[test]
    fn a_gate_over_a_taxed_goal_is_not_satisfied_until_the_taxed_figure_is_funded() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, crate::rate::BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &NewGoal {
                name: "Roth IRA".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
                floating: false,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
        setting::set(&db, Gate::Roth.key(), id).unwrap();

        assert_eq!(remaining(&db, Gate::Roth).unwrap(), Cents(6_500));
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
                base_cents: Cents::from_dollars(100_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
                floating: false,
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
                base_cents: Cents::from_dollars(5_500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
                floating: false,
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
                base_cents: Cents::from_dollars(500_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: false,
                sort: 1,
                taxed: false,
                floating: false,
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
        let plan = compute_from_db(&db, &settings, today).unwrap();

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

        let plan = compute_from_db(&db, &settings_from_db(&db).unwrap(), today).unwrap();

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

    /// The defaults total 60, so a fresh database is already a set the
    /// waterfall can divide.
    #[test]
    fn the_default_splits_leave_goals_a_share() {
        let db = db::open_in_memory().unwrap();
        check_splits(&db).unwrap();
    }

    /// A share below zero is the one the set rule cannot catch: it makes
    /// room for another past a hundred, and `Percent::of` hands the line the
    /// negative unclamped -- which `transfer::plan` would write as a real
    /// instruction, since nothing downstream reads a line's sign.
    #[test]
    fn a_split_below_zero_is_refused_even_when_the_three_total_under_a_hundred() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(150)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(-60)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(5)).unwrap();

        let err = check_splits(&db).unwrap_err().to_string();

        // The first one outside the bound, named as the sheet's own row.
        assert!(
            err.contains("Future Housing") && err.contains("150"),
            "the offending split is not named: {err}"
        );
    }

    /// The same bound the form holds with `parse_percent`, on the writer that
    /// has no field to hold it at: `cell::as_percent` reads whatever the
    /// sheet carries.
    #[test]
    fn a_split_past_a_hundred_is_refused_on_its_own() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(101)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(0)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(0)).unwrap();

        let err = check_splits(&db).unwrap_err().to_string();

        assert!(
            err.contains("101"),
            "the offending split is not named: {err}"
        );
    }

    /// A sheet whose three cells total over 100 would put a database into
    /// exactly the state the edit form refuses, and every discretionary
    /// dollar would go somewhere Goals was meant to have a share of.
    #[test]
    fn splits_totalling_over_one_hundred_are_refused_with_all_three_named() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(60)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(30)).unwrap();

        let err = check_splits(&db).unwrap_err().to_string();

        assert!(err.contains("120"), "the total is not named: {err}");
        for part in ["60", "30"] {
            assert!(err.contains(part), "{part} is not named: {err}");
        }
    }

    /// Exactly 100 leaves Goals nothing, which is a plan the owner may mean
    /// -- it is only *more* than the remainder that cannot be divided.
    #[test]
    fn splits_landing_exactly_on_one_hundred_are_accepted() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(50)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(20)).unwrap();

        check_splits(&db).unwrap();
    }

    /// An unset key is a waterfall running off the live balance, which is
    /// the ordinary case rather than a pin of zero.
    #[test]
    fn an_unpinned_excess_is_nothing_to_refuse() {
        let db = db::open_in_memory().unwrap();
        check_pinned_excess(&db).unwrap();
    }

    /// A sheet cell below zero leaves `budget` clamped at nothing while
    /// `excess_used` is negative, so every line floors to zero and
    /// `lines.total() <= excess_used` is false with not a dollar moved.
    #[test]
    fn a_negative_pinned_excess_is_refused() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::PINNED_EXCESS, Cents::from_dollars(-500)).unwrap();

        let err = check_pinned_excess(&db).unwrap_err().to_string();

        assert!(err.contains("-500.00"), "the figure is not named: {err}");
    }

    /// The drift line under the plan reads the cents `excess_used`'s floor
    /// drops, so a pin carrying its own would have it quoting a difference
    /// that is not one -- and the `Excess (Used)` row would then refuse the
    /// prefill it opened on.
    #[test]
    fn a_pinned_excess_carrying_cents_is_refused() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::PINNED_EXCESS, Cents(1_750_037)).unwrap();

        let err = check_pinned_excess(&db).unwrap_err().to_string();

        assert!(
            err.contains("whole number of dollars"),
            "the rule is not named: {err}"
        );
    }

    /// A payday whose excess is nothing is an ordinary payday, and holding
    /// the waterfall at it is what a pin is for.
    #[test]
    fn a_pinned_excess_of_zero_is_accepted() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::PINNED_EXCESS, Cents::ZERO).unwrap();

        check_pinned_excess(&db).unwrap();
    }
}
