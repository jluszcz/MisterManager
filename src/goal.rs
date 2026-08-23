//! The target every screen funds a goal to: the `goal` table and the sales
//! tax rate, fed to `calc::tax`.
//!
//! The shape of `fund.rs` -- reads rows and a setting out of `db`, hands
//! plain values to `calc`, hands the result back up. `db::goal` stays
//! queries, `calc::tax` stays pure, and this is the one place a goal's
//! target is derived from `key::TAX_RATE`.
//!
//! **The derived figure is the target everywhere, not a decoration.** A taxed
//! goal's shortfall, its percentage complete, its `$/Pay`, whether it counts
//! as still short for the payday plug, and whether it draws as overdue are
//! all computed against it. The stored number is the base those come from,
//! and the only places it is shown as itself are the two forms that edit it.

use crate::db::goal::{self, Goal, GoalWithBalance};
use crate::db::setting::{self, key};
use crate::db::{AccountId, Db, GoalId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use crate::reading::Reading;
use anyhow::{Context, Result};

/// What a taxed goal with no rate on record reports, on the read side and on
/// the form that would otherwise write one.
///
/// Here rather than in `tui`, so the module that refuses to *derive* a target
/// and the form that refuses to *store* one say the same sentence.
pub const NO_TAX_RATE: &str = "no sales tax rate is configured; import Constants first";

/// A goal, its balance, and the target it is funded to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Funding {
    /// Carries `base_cents` and `taxed` -- what the table holds.
    pub goal: Goal,
    pub current: Cents,
    /// `base_cents`, or `calc::tax` of it. Derived on every read, the way a
    /// fund's target percentage is: a rate that changes must not leave a
    /// stored figure behind quoting the old one.
    pub target: Cents,
}

/// What a goal is funded to.
///
/// **A taxed goal with no rate on record is an error, not a silent fallback
/// to the base.** An unset key normally means a feature is off, but the flag
/// on the row says tax is wanted, and quietly targeting the base would move
/// the Planning waterfall's plug on the strength of a missing setting -- the
/// same reasoning that makes a dangling gate key an error. The state is hard
/// to reach anyway: the rate arrives with `Constants`, and the goal form
/// refuses to create a taxed goal without one.
pub fn target(goal: &Goal, rate: Option<BasisPoints>) -> Result<Cents> {
    if !goal.taxed {
        return Ok(goal.base_cents);
    }
    let rate = rate.with_context(|| {
        format!(
            "{:?} is taxed but {} is unset: {NO_TAX_RATE}",
            goal.name,
            key::TAX_RATE
        )
    })?;
    crate::calc::tax(goal.base_cents, rate)
}

/// The rate once, for a whole list. Reading it per goal would be one query
/// per row for a figure that cannot change while the list is being built.
fn derive(
    rate: Option<BasisPoints>,
    reading: Reading,
    rows: Vec<GoalWithBalance>,
) -> Result<Vec<Funding>> {
    rows.into_iter()
        .map(|g| {
            let derived = target(&g.goal, rate);
            Ok(Funding {
                target: match reading {
                    Reading::Strict => derived?,
                    // Falls back on any failure to derive, not only on a
                    // missing rate: what a drawing path needs is a number,
                    // and the base is the one the table actually holds.
                    Reading::Tolerant => derived.unwrap_or(g.goal.base_cents),
                },
                goal: g.goal,
                current: g.current,
            })
        })
        .collect()
}

/// Every open goal in every container, with its balance and its target, in
/// `db::goal::all_with_balances` order.
///
/// [`Reading::Strict`] refuses a taxed goal with no rate on record, with
/// [`NO_TAX_RATE`]; [`Reading::Tolerant`] targets its base. Which one a
/// caller wants is [`Reading`]'s to explain -- the paths that spend a target
/// take the first, the Savings screen, `transfer::wiring` and the report take
/// the second.
pub fn all_with_balances(db: &Db, reading: Reading) -> Result<Vec<Funding>> {
    let rate = setting::get(db, key::TAX_RATE)?;
    derive(rate, reading, goal::all_with_balances(db)?)
}

/// One container's open goals, in the order every screen shows them.
///
/// Strict, with no reading to choose: its caller is the worksheet prefill,
/// which is about to price every goal in the container and write the result.
pub fn list_with_balances(db: &Db, container: AccountId) -> Result<Vec<Funding>> {
    let rate = setting::get(db, key::TAX_RATE)?;
    derive(
        rate,
        Reading::Strict,
        goal::list_with_balances(db, container)?,
    )
}

/// How much a goal still needs: its target less its balance, clamped at zero.
///
/// Here rather than in `db::goal` because it is a *target* reader, and the
/// rate cannot be reached from `db`. Leaving a second one behind would let
/// `plan::remaining` go on gating the waterfall against a base.
///
/// Errors if the goal does not exist. A caller holding an id for a goal that
/// is gone is looking at a corrupt database, not at an unfunded goal, and
/// reporting zero there would silently disable a Planning gate.
///
/// Clamped because goals overshoot: Emergency Savings sits above its target
/// in the live workbook, and a negative need would read as "needs funding"
/// at every call site that compares against zero.
///
/// Ignores `closed`, as `container_excess` does -- a closed goal's
/// allocations still count.
pub fn shortfall(db: &Db, goal_id: GoalId) -> Result<Cents> {
    let found = goal::get(db, goal_id)?.with_context(|| format!("no goal with id {goal_id}"))?;
    let target = target(&found, setting::get(db, key::TAX_RATE)?)?;
    let remaining = target - goal::balance(db, goal_id)?;
    Ok(if remaining < Cents::ZERO {
        Cents::ZERO
    } else {
        remaining
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Kind};
    use crate::db::goal::NewGoal;
    use crate::db::setting;
    use chrono::NaiveDate;

    fn seeded() -> (db::Db, crate::db::AccountId) {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        (db, savings)
    }

    fn new_goal(name: &str, container: crate::db::AccountId, base: i64, taxed: bool) -> NewGoal {
        NewGoal {
            name: name.to_string(),
            container_account_id: container,
            base_cents: Cents::from_dollars(base),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            sort: 0,
            taxed,
        }
    }

    /// Most goals are not taxed, and for those the stored figure is the
    /// answer: the derivation must not touch them.
    #[test]
    fn an_untaxed_goals_target_is_its_base() {
        let g = db::goal::Goal {
            id: crate::db::GoalId(1),
            name: "Couch".to_string(),
            container_account_id: crate::db::AccountId(1),
            base_cents: Cents::from_dollars(1_000),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            closed: false,
            sort: 0,
            favorite: false,
            taxed: false,
        };
        assert_eq!(
            target(&g, Some(BasisPoints(625))).unwrap(),
            Cents::from_dollars(1_000)
        );
    }

    /// 6.25% of $1,000 is $1,062.50, which the lambda's $5 increment carries
    /// up to $1,065. That is what the goal has to be funded to, because that
    /// is what the item costs at the register.
    #[test]
    fn a_taxed_goals_target_is_what_the_lambda_makes_of_its_base() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        let funded = list_with_balances(&db, savings).unwrap();
        assert_eq!(funded[0].target, Cents(106_500));
        assert_eq!(
            funded[0].goal.base_cents,
            Cents::from_dollars(1_000),
            "the stored figure is still the base"
        );
    }

    /// An unset key normally means a feature is off, but the flag on the row
    /// says tax is wanted. Quietly targeting the base would move the Planning
    /// waterfall's plug on the strength of a missing setting.
    #[test]
    fn a_taxed_goal_with_no_rate_on_record_is_an_error_naming_the_key() {
        let (db, savings) = seeded();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        let err = all_with_balances(&db, Reading::Strict)
            .unwrap_err()
            .to_string();
        assert!(err.contains(key::TAX_RATE.name()), "{err}");
        assert!(err.contains("Couch"), "{err}");
    }

    /// The other half of the rule the test above states: the refusal binds
    /// the paths that spend a target, and the drawing paths take the base so
    /// the owner still has an application to set the rate from.
    #[test]
    fn a_taxed_goal_with_no_rate_on_record_targets_its_base_when_read_tolerantly() {
        let (db, savings) = seeded();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        let funded = all_with_balances(&db, Reading::Tolerant).unwrap();
        assert_eq!(funded[0].target, Cents::from_dollars(1_000));
        assert!(funded[0].goal.taxed, "the flag is carried, not cleared");
        assert!(
            all_with_balances(&db, Reading::Strict).is_err(),
            "the strict reading still refuses"
        );
    }

    /// The tolerance is scoped to the goal that cannot resolve. A rate on
    /// record is spent exactly as the strict reader spends it, so a screen
    /// and a payday quote the same figure on every database but the corrupt
    /// one.
    #[test]
    fn the_tolerant_reader_taxes_a_goal_whose_rate_is_on_record() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();

        assert_eq!(
            all_with_balances(&db, Reading::Tolerant).unwrap()[0].target,
            Cents(106_500)
        );
    }

    /// The rate is only wanted by a goal that asked for tax, so a database no
    /// `Constants` sheet has reached still resolves every other goal.
    #[test]
    fn an_untaxed_goal_resolves_on_a_database_with_no_rate() {
        let (db, savings) = seeded();
        db::goal::insert(&db, &new_goal("Couch", savings, 1_000, false)).unwrap();

        let funded = all_with_balances(&db, Reading::Strict).unwrap();
        assert_eq!(funded[0].target, Cents::from_dollars(1_000));
    }

    #[test]
    fn shortfall_is_the_target_less_the_balance() {
        let (db, savings) = seeded();
        let id = db::goal::insert(&db, &new_goal("Roth IRA", savings, 5_500, false)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(2_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents::from_dollars(3_500));
    }

    #[test]
    fn shortfall_of_an_exactly_funded_goal_is_zero() {
        let (db, savings) = seeded();
        let id = db::goal::insert(&db, &new_goal("Roth IRA", savings, 5_500, false)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(5_500),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents::ZERO);
    }

    /// The shortfall of a taxed goal is measured against the taxed figure, or
    /// a goal funded to its base would come up short at the register -- which
    /// is the bug this whole feature exists to prevent.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_short_by_the_tax() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents(6_500));
    }

    /// Emergency Savings sits above its target in the live workbook, so an
    /// overfunded goal is a real state and not a hypothetical. Reporting a
    /// negative need would turn the emergency gate on and divert the entire
    /// discretionary split into a bucket that is already full.
    #[test]
    fn shortfall_of_an_overfunded_goal_is_zero_not_negative() {
        let (db, savings) = seeded();
        let id = db::goal::insert(&db, &new_goal("Emergency", savings, 100_000, false)).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(120_000),
            None,
            None,
        )
        .unwrap();

        assert_eq!(shortfall(&db, id).unwrap(), Cents::ZERO);
    }

    /// A dangling id is a corrupt database, not an unfunded goal. Returning
    /// zero here would silently switch a Planning gate off -- the exact
    /// failure this whole indirection exists to remove.
    #[test]
    fn shortfall_of_a_missing_goal_is_an_error() {
        let (db, _) = seeded();
        let err = shortfall(&db, crate::db::GoalId(999)).unwrap_err();
        assert!(err.to_string().contains("999"), "{err}");
    }
}
