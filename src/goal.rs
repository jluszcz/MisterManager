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
use chrono::NaiveDate;

/// What a taxed goal with no rate on record reports, on the read side and on
/// the form that would otherwise write one.
///
/// Here rather than in `tui`, so the module that refuses to *derive* a target
/// and the form that refuses to *store* one say the same sentence.
pub const NO_TAX_RATE: &str = "no sales tax rate is configured; import Constants first";

/// The longest note a goal may carry, in characters.
///
/// Here rather than in `tui`, beside [`NO_TAX_RATE`] and for the same reason:
/// the form that refuses an over-long note and the modal that draws one in a
/// single line have to agree about one number, and a second copy of it is a
/// note that commits and then draws clipped.
///
/// The figure is what the **goal form** can show, which is the narrower of the
/// two readers: `tui::widget::FORM_WIDTH` less its two border columns, less the
/// label gutter every field line is padded to, and less one more for the column
/// the caret sits in past the last character. A text field has no horizontal
/// scrolling, so a longer limit would be an invitation to type blind -- the
/// characters past the edge invisible, or the caret gone while the text is
/// still there, which is the same field with no visible end to type at.
///
/// That last column is why the figure is odd. It is spent only while the field
/// has focus, which is exactly when a note is being typed, so it is not slack
/// to reclaim. Stated here rather than derived from those four, because `goal`
/// must not reach into `tui` for a number;
/// `tui::goal_form`'s `a_note_at_the_limit_is_drawn_whole_on_the_form` focuses
/// the field and is what holds the two together.
pub const NOTE_LIMIT: usize = 47;

/// A goal, its balance, and the target it is funded to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Funding {
    /// Carries `base_cents` and `taxed` -- what the table holds.
    pub goal: Goal,
    pub current: Cents,
    /// `base_cents`, `calc::tax` of it, or -- for a floating goal -- `current`.
    /// Derived on every read, the way a fund's target percentage is: a rate
    /// that changes must not leave a stored figure behind quoting the old
    /// one, and neither may a balance that moves.
    pub target: Cents,
}

/// What a goal is funded to.
///
/// **A floating goal is funded to whatever it holds**, which is why the
/// balance is an argument here rather than something the callers keep to
/// themselves. Read before `taxed`, and before the rate is asked for: a
/// target that follows the balance has no base for the tax lambda to
/// ceiling, so a row carrying both flags resolves rather than erroring.
///
/// **A taxed goal with no rate on record is an error, not a silent fallback
/// to the base.** An unset key normally means a feature is off, but the flag
/// on the row says tax is wanted, and quietly targeting the base would move
/// the Planning waterfall's plug on the strength of a missing setting -- the
/// same reasoning that makes a dangling gate key an error. The state is hard
/// to reach anyway: the rate arrives with `Constants`, and the goal form
/// refuses to create a taxed goal without one.
pub fn target(goal: &Goal, current: Cents, rate: Option<BasisPoints>) -> Result<Cents> {
    if goal.floating {
        return Ok(current);
    }
    if !goal.taxed {
        return Ok(goal.base_cents);
    }
    crate::calc::tax(goal.base_cents, taxed_rate(goal, rate)?)
}

/// The rate a taxed goal is priced at, or the [`NO_TAX_RATE`] refusal naming
/// the goal -- the one sentence [`target`] and [`spent_base`] both refuse with.
fn taxed_rate(goal: &Goal, rate: Option<BasisPoints>) -> Result<BasisPoints> {
    rate.with_context(|| {
        format!(
            "{:?} is taxed but {} is unset: {NO_TAX_RATE}",
            // The Planning screen renders this in place of the plan, so the
            // goal's name reaches a viewer as prose. Masked here, where the
            // message is built, rather than in each screen that draws it.
            crate::demo::text(&goal.name),
            key::TAX_RATE
        )
    })
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
            let derived = target(&g.goal, g.current, rate);
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
    let current = goal::balance(db, goal_id)?;
    let remaining = target(&found, current, setting::get(db, key::TAX_RATE)?)? - current;
    Ok(if remaining < Cents::ZERO {
        Cents::ZERO
    } else {
        remaining
    })
}

/// What spending `amount` towards a goal takes off its base: the amount
/// itself for an untaxed goal, and its pre-tax part for a taxed one.
///
/// A spend is paid at the register, tax included, so it is the taxed figure
/// the target has to fall by -- and a taxed target is `calc::tax` of the
/// base, which no base reaches exactly, so the base falls by what the spend
/// was before tax, in whole dollars, and the target lands within the
/// lambda's rounding of falling by the amount. A floating goal takes the same rule: its target
/// already follows its balance, and the base it keeps for when the flag is
/// turned off is a goal of which this much has now been bought.
///
/// Refuses a taxed goal with no rate on record with [`NO_TAX_RATE`], for
/// the reason [`target`] does: this is a figure that is about to be written.
pub fn spent_base(goal: &Goal, amount: Cents, rate: Option<BasisPoints>) -> Result<Cents> {
    if !goal.taxed {
        return Ok(amount);
    }
    // Floored to a whole dollar, because goal figures are whole dollars: a
    // base left carrying cents is one the goal form can no longer commit.
    // Down rather than to the nearest, so the target never falls further
    // than the spend did.
    Ok(crate::calc::pretax(amount, taxed_rate(goal, rate)?)?.floor_to_dollar())
}

/// Move `amount` from one goal to another, and -- when `spent` -- lower the
/// source's target by it as well: money spent towards a goal before it was
/// funded, which leaves that much less of it to fund.
///
/// The move and its refusals are `db::goal::transfer_value`'s; what this adds
/// is the figure the base falls by, which needs the rate `db` cannot reach.
/// Returns that figure, zero when nothing was spent.
///
/// **Must be called at top level, not inside another [`Db::transaction`].**
pub fn transfer_value(
    db: &Db,
    from: GoalId,
    to: GoalId,
    amount: Cents,
    date: NaiveDate,
    note: Option<&str>,
    spent: bool,
) -> Result<Cents> {
    let lowered = if spent {
        let source = goal::get(db, from)?.with_context(|| format!("no goal with id {from}"))?;
        spent_base(&source, amount, setting::get(db, key::TAX_RATE)?)?
    } else {
        Cents::ZERO
    };
    goal::transfer_value(db, from, to, amount, date, note, lowered)?;
    Ok(lowered)
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
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0, None).unwrap();
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
            floating: false,
            note: None,
        }
    }

    /// 1,000 of 1,200, 100 spent, leaves 900 of 1,100 -- and the 100 lands
    /// in the other goal.
    #[test]
    fn spending_from_an_untaxed_goal_lowers_its_balance_and_target_alike() {
        let (db, savings) = seeded();
        let couch = db::goal::insert(&db, &new_goal("Couch", savings, 1_200, false)).unwrap();
        let rug = db::goal::insert(&db, &new_goal("Rug", savings, 500, false)).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        db::goal::insert_allocation(&db, couch, date, Cents::from_dollars(1_000), None, None)
            .unwrap();

        let lowered =
            transfer_value(&db, couch, rug, Cents::from_dollars(100), date, None, true).unwrap();

        assert_eq!(lowered, Cents::from_dollars(100));
        let funded = list_with_balances(&db, savings).unwrap();
        let couch_row = funded.iter().find(|f| f.goal.id == couch).unwrap();
        assert_eq!(couch_row.current, Cents::from_dollars(900));
        assert_eq!(couch_row.target, Cents::from_dollars(1_100));
        let rug_row = funded.iter().find(|f| f.goal.id == rug).unwrap();
        assert_eq!(rug_row.current, Cents::from_dollars(100));
        assert_eq!(rug_row.target, Cents::from_dollars(500));
    }

    #[test]
    fn a_transfer_not_spent_leaves_the_target_alone() {
        let (db, savings) = seeded();
        let couch = db::goal::insert(&db, &new_goal("Couch", savings, 1_200, false)).unwrap();
        let rug = db::goal::insert(&db, &new_goal("Rug", savings, 500, false)).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

        let lowered =
            transfer_value(&db, couch, rug, Cents::from_dollars(100), date, None, false).unwrap();

        assert_eq!(lowered, Cents::ZERO);
        assert_eq!(
            shortfall(&db, couch).unwrap(),
            Cents::from_dollars(1_200 + 100)
        );
    }

    /// A spend is paid tax included, so a taxed base falls by the pre-tax
    /// part and the target lands within the lambda's rounding of falling by
    /// the amount: $1,000 at 6.25% is $1,065; $106.25 spent is $100 of base,
    /// and $900 taxes to $960 -- $105 lower, the $5 step's nearest.
    #[test]
    fn spending_from_a_taxed_goal_lowers_its_base_by_the_pre_tax_part() {
        let (db, savings) = seeded();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let couch = db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();
        let rug = db::goal::insert(&db, &new_goal("Rug", savings, 500, false)).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

        let lowered = transfer_value(&db, couch, rug, Cents(10_625), date, None, true).unwrap();

        assert_eq!(lowered, Cents(10_000));
        let funded = list_with_balances(&db, savings).unwrap();
        let couch_row = funded.iter().find(|f| f.goal.id == couch).unwrap();
        assert_eq!(couch_row.goal.base_cents, Cents::from_dollars(900));
        assert_eq!(couch_row.target, Cents::from_dollars(960));
    }

    /// $80 at 6.25% is $75.29 before tax; the base falls by $75, so it stays
    /// a whole-dollar figure the goal form can commit.
    #[test]
    fn spending_from_a_taxed_goal_leaves_its_base_in_whole_dollars() {
        let (db, savings) = seeded();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let couch = db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();
        let rug = db::goal::insert(&db, &new_goal("Rug", savings, 500, false)).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

        let lowered =
            transfer_value(&db, couch, rug, Cents::from_dollars(80), date, None, true).unwrap();

        assert_eq!(lowered, Cents::from_dollars(75));
        let base = db::goal::get(&db, couch).unwrap().unwrap().base_cents;
        assert_eq!(base, Cents::from_dollars(925));
    }

    /// The spend is about to be written, so a missing rate is the strict
    /// refusal rather than a guess -- and nothing moves.
    #[test]
    fn spending_from_a_taxed_goal_with_no_rate_is_refused_and_moves_nothing() {
        let (db, savings) = seeded();
        let couch = db::goal::insert(&db, &new_goal("Couch", savings, 1_000, true)).unwrap();
        let rug = db::goal::insert(&db, &new_goal("Rug", savings, 500, false)).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

        let err = transfer_value(&db, couch, rug, Cents(8_000), date, None, true).unwrap_err();

        assert!(err.to_string().contains(NO_TAX_RATE), "{err}");
        assert_eq!(db::goal::balance(&db, rug).unwrap(), Cents::ZERO);
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
            floating: false,
            note: None,
        };
        assert_eq!(
            target(&g, Cents::from_dollars(250), Some(BasisPoints(625))).unwrap(),
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
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0, None).unwrap();
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
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0, None).unwrap();
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
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0, None).unwrap();
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

    /// The flag is the whole of what a floating goal's target is: the base
    /// beside it is inert, so a goal switched to floating stops quoting the
    /// figure it was funded towards and starts quoting what it holds.
    #[test]
    fn a_floating_goals_target_is_its_balance_whatever_its_base() {
        let mut g = new_goal("Brokerage", crate::db::AccountId(1), 1_000, false);
        g.floating = true;
        let stored = db::goal::Goal {
            id: crate::db::GoalId(1),
            name: g.name,
            container_account_id: g.container_account_id,
            base_cents: g.base_cents,
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            closed: false,
            sort: 0,
            favorite: false,
            taxed: false,
            floating: true,
            note: None,
        };

        assert_eq!(
            target(&stored, Cents::from_dollars(250), None).unwrap(),
            Cents::from_dollars(250),
            "the balance, not the $1,000 base"
        );
    }

    /// Floating is read before `taxed`, so a row carrying both is not an
    /// error over a missing rate: there is no base for a lambda to ceiling
    /// once the target follows the balance. The form cannot write that pair,
    /// but a row can hold it, and a strict read must still resolve.
    #[test]
    fn a_floating_goal_targets_its_balance_even_when_it_is_also_flagged_taxed() {
        let (db, savings) = seeded();
        let mut g = new_goal("Brokerage", savings, 1_000, true);
        g.floating = true;
        let id = db::goal::insert(&db, &g).unwrap();
        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(250),
            None,
            None,
        )
        .unwrap();

        let funded = all_with_balances(&db, Reading::Strict).unwrap();
        assert_eq!(funded[0].target, Cents::from_dollars(250));
    }

    /// The point of the flag: a goal that is at its target by definition is
    /// never short, so no Planning gate behind one ever opens and the payday
    /// plug never offers it anything.
    #[test]
    fn shortfall_of_a_floating_goal_is_zero_at_any_balance() {
        let (db, savings) = seeded();
        let mut g = new_goal("Brokerage", savings, 100_000, false);
        g.floating = true;
        let id = db::goal::insert(&db, &g).unwrap();
        assert_eq!(
            shortfall(&db, id).unwrap(),
            Cents::ZERO,
            "with nothing in it"
        );

        db::goal::insert_allocation(
            &db,
            id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Cents::from_dollars(-40),
            None,
            None,
        )
        .unwrap();
        assert_eq!(shortfall(&db, id).unwrap(), Cents::ZERO, "and overspent");
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
