//! The edit side of the Planning constants: what a typed value must be
//! before it may become one, and where each one lands.
//!
//! [`Target`] is the constant a row *is* -- `plan_rows` owns it, since the
//! report reads the same list -- and its `write` is this screen's alone: the
//! page has no editor to reach it from.

use crate::db::Db;
use crate::db::setting::{self, key};
use crate::db::split_profile::{self, Shares};
use crate::money::Cents;
use crate::plan_line::Line;
use crate::rate::BasisPoints;
use crate::tui::form::{parse_amount, parse_percent, parse_whole_amount};
use anyhow::{Context, Result, bail, ensure};
use chrono::NaiveDate;

pub use crate::plan_rows::Target;

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
/// rule. The share lands in the active split profile, which is the set the
/// waterfall divides and the one the screen is showing; `share` picks which
/// of its three this is.
///
/// The message quotes the headroom rather than the sum: what the owner needs
/// is the number they may type, and a set already at 100 means something else
/// must come down first.
fn write_split(
    db: &Db,
    line: Line,
    raw: &str,
    share: fn(&mut Shares) -> &mut BasisPoints,
) -> Result<()> {
    let value = parse_percent(raw)?;
    let profile = split_profile::active(db)?;
    let mut shares = profile.shares;
    *share(&mut shares) = BasisPoints::ZERO;
    let claimed = shares.total();
    ensure!(
        value + claimed <= BasisPoints::ONE,
        "{} leaves {}% at most: the other two splits already claim {}%",
        line.label(),
        BasisPoints::ONE.saturating_sub(claimed).exact_percent(),
        claimed.exact_percent()
    );
    *share(&mut shares) = value;
    split_profile::set_shares(db, profile.id, shares)
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
            Target::FutureHousingPct => {
                write_split(db, Line::FutureHousing, raw, |s| &mut s.future_housing)
            }
            Target::RetirementPct => write_split(db, Line::Retirement, raw, |s| &mut s.retirement),
            Target::InvestmentPct => write_split(db, Line::Investment, raw, |s| &mut s.investment),
            // Both keys, for the reason `p` moves both: a date with no amount
            // would render a line about a plan that is not pinned.
            Target::PinnedExcess => {
                setting::set(db, key::PINNED_EXCESS, parse_pinned_excess(raw)?)?;
                setting::set(db, key::PINNED_AT, today)
            }
            // `e` on a bill opens the bill's own form, which writes the whole
            // row through `bill::update`; no one-field editor reaches here.
            Target::Bill(_) => bail!("a bill is edited through its form"),
            // Its name: the goal it fills is chosen from a list, not typed.
            Target::Gate(gate) => crate::gate::rename(db, gate, raw),
            Target::SplitProfile => split_profile::rename(db, split_profile::active(db)?.id, raw),
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
            | Target::InvestmentPct
            | Target::Gate(_)
            | Target::SplitProfile => false,
        }
    }

    /// Whether this row belongs to the Split block, where `←`/`→`, `a` and
    /// `d` act on the split profile rather than on what they mean elsewhere.
    pub fn in_split_block(self) -> bool {
        matches!(
            self,
            Target::SplitProfile
                | Target::FutureHousingPct
                | Target::RetirementPct
                | Target::InvestmentPct
        )
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

    use crate::rate::BasisPoints;
    use crate::test_support::day;

    use crate::db::split_profile::{self, Shares};
    use crate::tui::planning::test_support::*;
    use crate::tui::planning::{Editable, Planning};

    fn shares(fh: i64, rt: i64, inv: i64) -> Shares {
        Shares {
            future_housing: BasisPoints(fh),
            retirement: BasisPoints(rt),
            investment: BasisPoints(inv),
        }
    }

    fn split(db: &Db) -> Shares {
        split_profile::active(db).unwrap().shares
    }

    fn set_split(db: &Db, set: Shares) {
        split_profile::set_shares(db, split_profile::active(db).unwrap().id, set).unwrap();
    }

    /// A split edit lands in the profile the screen is showing, and the
    /// owner's other profiles keep what they were set to.
    #[test]
    fn a_split_edit_writes_only_the_active_profile() {
        let db = db::open_in_memory().unwrap();
        let first = split_profile::active(&db).unwrap().id;
        let maxed = split_profile::insert(&db, "Maxed", shares(1_000, 4_000, 2_000)).unwrap();
        split_profile::set_active(&db, maxed).unwrap();

        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "45")
            .unwrap();

        assert_eq!(split(&db), shares(1_000, 4_500, 2_000));
        assert_eq!(
            split_profile::get(&db, first).unwrap().shares,
            shares(3_000, 2_000, 1_000)
        );
    }

    #[test]
    fn writing_the_profile_target_renames_the_active_profile() {
        let db = db::open_in_memory().unwrap();
        Target::SplitProfile
            .write(&db, day(2026, 8, 14), " Contributing ")
            .unwrap();
        assert_eq!(split_profile::active(&db).unwrap().name, "Contributing");
        assert!(!Target::SplitProfile.is_money());
    }

    #[test]
    fn only_the_profile_and_its_three_shares_are_in_the_split_block() {
        for t in [
            Target::SplitProfile,
            Target::FutureHousingPct,
            Target::RetirementPct,
            Target::InvestmentPct,
        ] {
            assert!(t.in_split_block(), "{t:?}");
        }
        for t in [
            Target::BillPaymentPct,
            Target::GoalsFloor,
            Target::Bill(BillId(1)),
        ] {
            assert!(!t.in_split_block(), "{t:?}");
        }
    }

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
        assert_eq!(pct(key::BILL_PAYMENT_PCT), BasisPoints(6_000));
        assert_eq!(cents(key::MOM_AND_DAD_ANNUAL), Cents::from_dollars(4_000));
        assert_eq!(cents(key::GOALS_FLOOR), Cents::from_dollars(500));
        assert_eq!(split(&db), shares(3_000, 2_000, 1_000));
    }

    #[test]
    fn a_bill_target_is_edited_through_its_form_rather_than_written_as_a_value() {
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

        let err = Target::Bill(id)
            .write(&db, day(2026, 8, 14), "3,100")
            .unwrap_err();

        assert!(err.to_string().contains("form"), "{err}");
        let found = crate::db::bill::get(&db, id).unwrap();
        assert_eq!(found.cents, Cents::from_dollars(1_200));
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

    #[test]
    fn a_percentage_takes_a_bare_number_or_a_trailing_sign_and_nothing_else() {
        let db = db::open_in_memory().unwrap();
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), " 15% ")
            .unwrap();
        assert_eq!(split(&db).retirement, BasisPoints(1_500));

        for junk in ["fifteen", "", "%", ".", "1.2.3", "1e2", "+5", "-"] {
            assert!(
                Target::RetirementPct
                    .write(&db, day(2026, 8, 14), junk)
                    .is_err(),
                "{junk:?} was accepted"
            );
        }
    }

    /// The split the owner asked for: `10.5 / 20.5 / 69` is a whole payday,
    /// and each share is stored to the hundredth it was typed at.
    #[test]
    fn a_split_takes_a_fraction_of_a_percent() {
        let db = db::open_in_memory().unwrap();
        let write = |target: Target, raw| target.write(&db, day(2026, 8, 14), raw).unwrap();
        write(Target::FutureHousingPct, "10.5");
        write(Target::RetirementPct, "20.5%");
        write(Target::InvestmentPct, "69");

        assert_eq!(split(&db), shares(1_050, 2_050, 6_900));
        assert_eq!(
            crate::plan::settings_from_db(&db).unwrap().goals_pct(),
            BasisPoints::ZERO
        );
    }

    /// `5` after the point is fifty hundredths and `05` is five, and a point
    /// with nothing before it is a fraction of one percent -- typed as a
    /// percentage, so `0.35` is 0.35% and draws back as exactly that.
    #[test]
    fn a_fraction_is_read_to_the_hundredth_it_was_typed_at() {
        let db = db::open_in_memory().unwrap();
        for (raw, expected) in [
            ("12.5", 1_250),
            ("12.05", 1_205),
            ("12.", 1_200),
            (".5", 50),
            ("0.35", 35),
            ("100.00", 10_000),
        ] {
            Target::BillPaymentPct
                .write(&db, day(2026, 8, 14), raw)
                .unwrap();
            assert_eq!(
                setting::get(&db, key::BILL_PAYMENT_PCT).unwrap(),
                Some(BasisPoints(expected)),
                "{raw:?}"
            );
        }
    }

    /// A hundredth is the finest a share is stored at, and rounding a third
    /// decimal away would store a split nobody typed.
    #[test]
    fn a_third_decimal_is_refused_rather_than_rounded() {
        let db = db::open_in_memory().unwrap();
        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "10.125")
            .unwrap_err();
        assert!(err.to_string().contains("10.125"), "{err}");
        assert_eq!(split(&db).retirement, BasisPoints(2_000), "unchanged");
    }

    /// The headroom is quoted to the hundredth too, or a refusal over a
    /// fractional set would name a figure the owner cannot type to land on.
    #[test]
    fn the_headroom_is_quoted_to_the_hundredth() {
        let db = db::open_in_memory().unwrap();
        set_split(&db, shares(1_050, 0, 6_900));

        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "20.51")
            .unwrap_err();

        assert!(err.to_string().contains("20.5%"), "the headroom: {err}");
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "20.5")
            .unwrap();
    }

    /// Goals gets what the other three leave, so a combination over 100%
    /// leaves it nothing and sends every discretionary dollar elsewhere. The
    /// three are refused as a set rather than one at a time, which is the
    /// only place the sum is knowable.
    #[test]
    fn a_split_pushing_the_three_over_one_hundred_is_refused() {
        let db = db::open_in_memory().unwrap();
        set_split(&db, shares(3_000, 0, 1_000));

        let err = Target::RetirementPct
            .write(&db, day(2026, 8, 14), "70")
            .unwrap_err();

        assert!(err.to_string().contains("60"), "the headroom: {err}");
        assert_eq!(
            split(&db).retirement,
            BasisPoints::ZERO,
            "the refused write must not have landed"
        );
    }

    /// The sum is what is bounded, not the change: landing exactly on 100
    /// leaves Goals nothing and is a plan the owner may well mean.
    #[test]
    fn a_split_landing_exactly_on_one_hundred_is_accepted() {
        let db = db::open_in_memory().unwrap();
        set_split(&db, shares(3_000, 0, 1_000));

        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "60")
            .unwrap();

        assert_eq!(split(&db).retirement, BasisPoints(6_000));
    }

    /// The headroom is measured against the profile's own other two shares,
    /// which on a fresh database are the seeded defaults.
    #[test]
    fn the_headroom_counts_the_active_profiles_other_two_shares() {
        let db = db::open_in_memory().unwrap();

        // Future Housing 30 and Investment 10, so Retirement may have 60 and
        // no more.
        assert!(
            Target::RetirementPct
                .write(&db, day(2026, 8, 14), "61")
                .is_err()
        );
        Target::RetirementPct
            .write(&db, day(2026, 8, 14), "60")
            .unwrap();
    }

    /// `BasisPoints::of` does not clamp, so a percentage outside `0..=100` would
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
        assert_eq!(split(&db).retirement, BasisPoints(2_000), "unchanged");
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
