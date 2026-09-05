//! The edit side of the Planning constants: what a typed value must be
//! before it may become one, and where each one lands.
//!
//! [`Target`] is the constant a row *is* -- `plan_rows` owns it, since the
//! report reads the same list -- and its `write` is this screen's alone: the
//! page has no editor to reach it from.

use crate::calc::planning::PlanSettings;
use crate::db::Db;
use crate::db::bill;
use crate::db::setting::{self, key};
use crate::money::Cents;
use crate::plan_line::Line;
use crate::rate::Percent;
use crate::tui::form::{parse_amount, parse_whole_amount};
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;

pub use crate::plan_rows::Target;

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

/// The figure the whole waterfall then runs off.
///
/// Whole dollars, because `excess_used` is a whole-dollar figure however it is
/// arrived at -- `p` floors the live actual, and `compute` floors it again when
/// nothing is pinned. The drift line under the plan reads the cents that floor
/// drops, so a pin carrying cents of its own would have it quoting a difference
/// that is not one.
///
/// Refused below zero: `excess_actual` is clamped there and `p` can only ever
/// pin its floor, so no other path produces a negative pin -- and one typed
/// here would drive every line below it off a figure that means nothing. Zero
/// itself is an ordinary payday, and holding the waterfall at it is exactly
/// what a pin is for.
///
/// Text, because that is the shape this writer takes.
/// [`crate::plan::check_pinned_excess`] holds the same two bounds on the
/// stored figure, which is the shape an import writes.
fn parse_pinned_excess(raw: &str) -> Result<Cents> {
    let cents = parse_whole_amount(raw)?;
    ensure!(
        cents >= Cents::ZERO,
        "excess cannot be negative: {}",
        crate::demo::typed(raw.trim())
    );
    Ok(cents)
}

/// Write one of the three discretionary splits, refusing a set that leaves
/// Goals less than nothing.
///
/// Goals takes what the other three leave -- `100 - (fh + rt + inv)`,
/// saturating at zero -- so a set totalling over 100 hands every discretionary
/// dollar to the other three and Goals nothing. Bounded as a *set*, because
/// that is the only thing the bound is about: each of the three is already
/// inside `0..=100` on its own and still wrong between them.
///
/// This is what makes the clamp in `calc::planning` a backstop rather than the
/// rule. `others` reads through [`crate::plan::settings_from_db`] so the
/// default an unset key stands for is the one the waterfall will use, rather
/// than a second copy of it here.
///
/// The message quotes the headroom rather than the sum: what the owner needs
/// is the number they may type, and a set already at 100 means something else
/// must come down first.
fn write_split(
    db: &Db,
    key: crate::db::setting::Key<Percent>,
    line: Line,
    raw: &str,
    others: impl Fn(&PlanSettings) -> i64,
) -> Result<()> {
    let value = parse_percent(raw)?;
    let claimed = others(&crate::plan::settings_from_db(db)?);
    ensure!(
        value.0 + claimed <= 100,
        "{} leaves {}% at most: the other two splits already claim {claimed}%",
        line.label(),
        100 - claimed
    );
    setting::set(db, key, value)
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
    ///
    /// `today` is the date a pin is stamped with, and is what the one arm that
    /// writes two keys needs: the pair moves together, so dating the figure
    /// belongs beside writing it rather than at the call site. Every other arm
    /// ignores it.
    pub fn write(self, db: &Db, today: NaiveDate, raw: &str) -> Result<()> {
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
            Target::FutureHousingPct => write_split(
                db,
                key::SPLIT_FUTURE_HOUSING_PCT,
                Line::FutureHousing,
                raw,
                |s| s.retirement_pct.0 + s.investment_pct.0,
            ),
            Target::RetirementPct => {
                write_split(db, key::SPLIT_RETIREMENT_PCT, Line::Retirement, raw, |s| {
                    s.future_housing_pct.0 + s.investment_pct.0
                })
            }
            Target::InvestmentPct => {
                write_split(db, key::SPLIT_INVESTMENT_PCT, Line::Investment, raw, |s| {
                    s.future_housing_pct.0 + s.retirement_pct.0
                })
            }
            // Both keys, for the reason `p` moves both: a date with no amount
            // would render a line about a plan that is not pinned.
            Target::PinnedExcess => {
                setting::set(db, key::PINNED_EXCESS, parse_pinned_excess(raw)?)?;
                setting::set(db, key::PINNED_AT, today)
            }
            Target::Bill(id) => bill::set_amount(db, id, parse_amount(raw)?),
        }
    }

    /// Whether this constant is an amount of money.
    ///
    /// The other half of [`Target::write`]'s match, and written out beside it
    /// for that reason: a target is money exactly when its `write` arm parses
    /// with [`parse_amount`]. What reads it is the edit modal, which has one
    /// field and no idea what is in it -- a percentage and a count of pay
    /// periods go through the same form, and a demo must scramble the target's
    /// digits without scrambling those.
    pub fn is_money(self) -> bool {
        match self {
            Target::Target
            | Target::Buffer
            | Target::BillPaymentCap
            | Target::MomAndDadAnnual
            | Target::GoalsFloor
            | Target::PinnedExcess
            | Target::Bill(_) => true,
            Target::PeriodsPerYear
            | Target::BillPaymentPct
            | Target::FutureHousingPct
            | Target::RetirementPct
            | Target::InvestmentPct => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db;
    use crate::db::BillId;
    use crate::db::bill::Category;
    use crate::db::setting::{self, key};
    use crate::money::Cents;

    use crate::rate::Percent;
    use crate::test_support::day;

    use crate::tui::planning::test_support::*;
    use crate::tui::planning::{Editable, Planning};

    #[test]
    fn each_target_writes_its_own_setting() {
        let db = db::open_in_memory().unwrap();
        Target::Target
            .write(&db, day(2026, 8, 14), "1,000")
            .unwrap();
        Target::Buffer
            .write(&db, day(2026, 8, 14), "$2,000.50")
            .unwrap();
        Target::PeriodsPerYear
            .write(&db, day(2026, 8, 14), "24")
            .unwrap();
        Target::BillPaymentCap
            .write(&db, day(2026, 8, 14), "3000")
            .unwrap();
        Target::BillPaymentPct
            .write(&db, day(2026, 8, 14), "60")
            .unwrap();
        Target::MomAndDadAnnual
            .write(&db, day(2026, 8, 14), "4000")
            .unwrap();
        Target::GoalsFloor
            .write(&db, day(2026, 8, 14), "500")
            .unwrap();
        Target::FutureHousingPct
            .write(&db, day(2026, 8, 14), "30")
            .unwrap();
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "20")
            .unwrap();
        Target::InvestmentPct
            .write(&db, day(2026, 8, 14), "10")
            .unwrap();

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

        Target::Bill(id)
            .write(&db, day(2026, 8, 14), "3,100")
            .unwrap();

        let found = crate::db::bill::get(&db, id).unwrap();
        assert_eq!(found.cents, Cents::from_dollars(3_100));
        assert_eq!(found.label, "Mortgage");
    }

    /// The pin is what `Excess (Used)` *is* when the plan is pinned, so the
    /// row opens on the pin rather than on the live excess behind it.
    #[test]
    fn the_excess_used_row_opens_on_the_pin() {
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(12_000)), None))
            .unwrap();

        let r = row(&planning, "Excess (Used)");
        assert_eq!(r.editable, Some(Editable::Constant(Target::PinnedExcess)));
        assert_eq!(r.edit, "12,000.00");
    }

    /// Unpinned there is no pin to open on, and `Excess (Used)` is the live
    /// excess floored. Typing over that figure is what makes the first pin,
    /// so the figure it starts from is the one already on the row.
    #[test]
    fn an_unpinned_excess_used_row_opens_on_the_floored_actual() {
        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        let r = row(&planning, "Excess (Used)");
        assert_eq!(r.editable, Some(Editable::Constant(Target::PinnedExcess)));
        assert_eq!(r.edit, "17,500.00");
    }

    /// The sheet's hand-typed `Excess (Fixed)` cell, back as a row. Both pin
    /// keys move together here for the reason they do under `p`: a date with
    /// no amount would render a line about a plan that is not pinned.
    #[test]
    fn a_typed_excess_pins_the_figure_and_dates_it() {
        let db = db::open_in_memory().unwrap();

        Target::PinnedExcess
            .write(&db, day(2026, 8, 14), "12,000")
            .unwrap();

        assert_eq!(
            setting::get(&db, key::PINNED_EXCESS).unwrap(),
            Some(Cents::from_dollars(12_000))
        );
        assert_eq!(
            setting::get(&db, key::PINNED_AT).unwrap(),
            Some(day(2026, 8, 14))
        );
    }

    /// `excess_used` is a whole-dollar figure however it is arrived at: `p`
    /// floors the actual, and `compute` floors it again when nothing is
    /// pinned. The drift line reads the cents that floor drops, so a pin
    /// carrying cents of its own would have it quoting a difference that is
    /// not one.
    #[test]
    fn a_typed_excess_must_land_on_a_whole_dollar() {
        let db = db::open_in_memory().unwrap();

        let err = Target::PinnedExcess
            .write(&db, day(2026, 8, 14), "12,000.50")
            .unwrap_err();

        assert!(err.to_string().contains("whole number of dollars"), "{err}");
        assert_eq!(setting::get(&db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&db, key::PINNED_AT).unwrap(), None);
    }

    /// `excess_actual` is clamped at zero and `p` can only ever pin its
    /// floor, so no other path produces a negative pin -- and one typed here
    /// would drive every line below it off a figure that means nothing.
    #[test]
    fn a_negative_typed_excess_is_refused() {
        let db = db::open_in_memory().unwrap();

        let err = Target::PinnedExcess
            .write(&db, day(2026, 8, 14), "-500")
            .unwrap_err();

        assert!(err.to_string().contains("-500"), "{err}");
        assert_eq!(setting::get(&db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&db, key::PINNED_AT).unwrap(), None);
    }

    /// A payday whose excess is nothing is an ordinary payday, and holding
    /// the waterfall at it is what the pin is for.
    #[test]
    fn a_typed_excess_of_zero_is_pinned_like_any_other() {
        let db = db::open_in_memory().unwrap();

        Target::PinnedExcess
            .write(&db, day(2026, 8, 14), "0")
            .unwrap();

        assert_eq!(
            setting::get(&db, key::PINNED_EXCESS).unwrap(),
            Some(Cents::ZERO)
        );
    }

    /// `Percent` is whole percent. Accepting `0.35` would silently divide the
    /// split by a hundred and reroute every discretionary dollar.
    #[test]
    fn a_percentage_takes_a_bare_number_or_a_trailing_sign_and_nothing_else() {
        let db = db::open_in_memory().unwrap();
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), " 15% ")
            .unwrap();
        assert_eq!(
            setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(),
            Some(Percent(15))
        );

        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "0.35")
            .unwrap_err();
        assert!(err.to_string().contains("0.35"), "{err}");
        assert!(
            Target::RetirementPct
                .write(&db, day(2026, 8, 14), "fifteen")
                .is_err()
        );
    }

    /// Goals gets what the other three leave, so a combination over 100%
    /// leaves it nothing and sends every discretionary dollar elsewhere. The
    /// three are refused as a set rather than one at a time, which is the
    /// only place the sum is knowable.
    #[test]
    fn a_split_pushing_the_three_over_one_hundred_is_refused() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(10)).unwrap();

        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "70")
            .unwrap_err();

        assert!(err.to_string().contains("60"), "the headroom: {err}");
        assert_eq!(
            setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(),
            None,
            "the refused write must not have landed"
        );
    }

    /// The sum is what is bounded, not the change: landing exactly on 100
    /// leaves Goals nothing and is a plan the owner may well mean.
    #[test]
    fn a_split_landing_exactly_on_one_hundred_is_accepted() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(10)).unwrap();

        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "60")
            .unwrap();

        assert_eq!(
            setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(),
            Some(Percent(60))
        );
    }

    /// An unset key reads as its default rather than as nothing, so the
    /// headroom quoted is the headroom the waterfall will actually use.
    #[test]
    fn the_headroom_counts_the_defaults_the_unset_keys_stand_for() {
        let db = db::open_in_memory().unwrap();

        // Nothing is set: Future Housing 30 and Investment 10 by default, so
        // Retirement may have 60 and no more.
        assert!(
            Target::RetirementPct
                .write(&db, day(2026, 8, 14), "61")
                .is_err()
        );
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "60")
            .unwrap();
    }

    /// `Percent::of` does not clamp, so a percentage outside `0..=100` would
    /// write a negative or over-100 allocation straight into the waterfall
    /// with no error at any layer downstream.
    #[test]
    fn a_percentage_outside_zero_to_one_hundred_is_refused() {
        let db = db::open_in_memory().unwrap();
        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "-15")
            .unwrap_err();
        assert!(err.to_string().contains("-15"), "{err}");
        assert!(
            Target::RetirementPct
                .write(&db, day(2026, 8, 14), "101")
                .is_err()
        );
        assert_eq!(setting::get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(), None);
    }

    /// The `.max(1)` clamps downstream are the backstop; the form is where a
    /// nonsense count should be refused with a message that names it.
    #[test]
    fn a_non_positive_pay_period_count_is_refused() {
        let db = db::open_in_memory().unwrap();
        let err = Target::PeriodsPerYear
            .write(&db, day(2026, 8, 14), "0")
            .unwrap_err();
        assert!(err.to_string().contains('0'), "{err}");
        assert!(
            Target::PeriodsPerYear
                .write(&db, day(2026, 8, 14), "-4")
                .is_err()
        );
        assert!(
            Target::PeriodsPerYear
                .write(&db, day(2026, 8, 14), "26.5")
                .is_err()
        );
        assert_eq!(setting::get(&db, key::PAY_PERIODS_PER_YEAR).unwrap(), None);
    }

    /// A target is money exactly when its `write` arm parses an amount. The
    /// two matches are written out separately, so nothing but this holds them
    /// together -- and getting it wrong either publishes a figure in a demo
    /// or scrambles a percentage that was never private.
    #[test]
    fn a_money_target_is_the_one_whose_write_parses_an_amount() {
        for target in [
            Target::Target,
            Target::Buffer,
            Target::BillPaymentCap,
            Target::MomAndDadAnnual,
            Target::GoalsFloor,
            Target::PinnedExcess,
            Target::Bill(BillId(1)),
        ] {
            assert!(target.is_money(), "{target:?} writes an amount");
        }
        for target in [
            Target::PeriodsPerYear,
            Target::BillPaymentPct,
            Target::FutureHousingPct,
            Target::RetirementPct,
            Target::InvestmentPct,
        ] {
            assert!(!target.is_money(), "{target:?} writes no amount");
        }
    }
}
