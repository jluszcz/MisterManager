//! Recurring transactions: what a recurring transaction generates, and when.
//!
//! A top-level module beside `plan.rs`, and for the same reason -- it reads
//! from `db`, applies policy, and calls into a pure `calc`.
//! `db::recurring_txn` stays CRUD plus the queries regeneration needs; the
//! horizon, the adoption order, and what a cadence *is* all live here.

use crate::calc::schedule::{self, Step};
use crate::db::recurring_txn::{self, Cadence, RecurringTxn};
use crate::db::setting::{self, key};
use crate::db::{Db, RecurringTxnId, txn};
use anyhow::{Context, Result};
use chrono::{Months, NaiveDate};
use std::collections::HashSet;
use std::fmt;

/// What one regeneration did.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Regenerated {
    pub removed: usize,
    /// Hand-corrected rows handed back to the ledger because their date is no
    /// longer an occurrence. Reported rather than left to be inferred from a
    /// changed row count: the owner's correction is now an ordinary row, and
    /// only they can decide whether to keep it.
    pub released: usize,
    pub adopted: usize,
    pub inserted: usize,
}

/// The four counts as a status line reads them, in the order regeneration
/// does them. On the type rather than beside a screen, so the three keys that
/// report a regeneration -- `g`, `G` and `x` -- cannot come to count the same
/// work in three wordings.
impl fmt::Display for Regenerated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "removed {} · released {} · adopted {} · inserted {}",
            self.removed, self.released, self.adopted, self.inserted
        )
    }
}

impl Regenerated {
    fn add(self, other: Regenerated) -> Regenerated {
        Regenerated {
            removed: self.removed + other.removed,
            released: self.released + other.released,
            adopted: self.adopted + other.adopted,
            inserted: self.inserted + other.inserted,
        }
    }
}

/// The one place a persisted cadence becomes a schedule.
///
/// It lives here rather than on `Cadence`, so `calc` never learns that
/// `biweekly` is a string in a column and `db` never learns what the string
/// means.
fn step(cadence: Cadence) -> Step {
    match cadence {
        Cadence::Biweekly => Step::Days(14),
        Cadence::Monthly => Step::Months(1),
    }
}

/// The furthest ahead the rolling horizon may reach, ten years.
///
/// The upper end matters as much as the lower: every occurrence inside the
/// horizon is a row written in one transaction, so a fat-fingered 36000 is
/// thousands of inserts and a ledger nobody wants. Ten years is already far
/// past any plan the screen shows and keeps a biweekly recurring transaction
/// under three hundred rows.
const MAX_HORIZON_MONTHS: i64 = 120;

/// The last date `recurring_txn` may generate today: the rolling horizon,
/// raised by any extension, then capped by the recurring transaction's own
/// end.
///
/// The two dates on the row pull in opposite directions and neither can
/// stand in for the other. `generate_through` is a **floor**: `x` asks for
/// rows the rolling window does not reach, and an extension the window has
/// since overtaken is spent rather than pulling generation back in.
/// `horizon` is a **cap**: the recurring transaction ends there, and no
/// extension outlives it.
///
/// Both are clamped to ten years out, from the same date: a floor nobody
/// bounded would write a decade of rows per press.
fn horizon(db: &Db, recurring_txn: &RecurringTxn, today: NaiveDate) -> Result<NaiveDate> {
    let reach = reach(db, recurring_txn, today)?.min(furthest(today)?);
    Ok(match recurring_txn.horizon {
        Some(end) => reach.min(end),
        None => reach,
    })
}

/// How many months of rows the rolling window holds. User-editable and
/// reaching a date computation, so it is clamped at both ends exactly as a
/// divisor would be.
fn horizon_months(db: &Db) -> Result<i64> {
    Ok(setting::get_or(db, key::RECURRING_TXN_HORIZON_MONTHS, 3)?.clamp(1, MAX_HORIZON_MONTHS))
}

/// How far generation reaches before either cap: the rolling window, or the
/// floor an extension has written past it.
///
/// [`horizon`] then caps it -- by ten years and by the recurring
/// transaction's own end -- while [`extend`] reads it uncapped, because the
/// caps are precisely what it has to report having hit.
fn reach(db: &Db, recurring_txn: &RecurringTxn, today: NaiveDate) -> Result<NaiveDate> {
    let rolling = add_months(today, horizon_months(db)?)?;
    Ok(match recurring_txn.generate_through {
        Some(extended) => rolling.max(extended),
        None => rolling,
    })
}

/// The furthest date any recurring transaction may generate to, extension or
/// not: ten years out.
fn furthest(today: NaiveDate) -> Result<NaiveDate> {
    add_months(today, MAX_HORIZON_MONTHS)
}

fn add_months(date: NaiveDate, months: i64) -> Result<NaiveDate> {
    let months = u32::try_from(months).unwrap_or(u32::MAX);
    date.checked_add_months(Months::new(months))
        .context("the recurring transaction horizon ran off the end of the calendar")
}

/// What one press of `x` did.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Extended {
    /// The rows now reach `through` -- the extended floor, cut short by the
    /// recurring transaction's own end, which is the date the ledger shows.
    /// The floor itself is stored uncapped, so moving that end out later
    /// reaches further without a second press.
    Through {
        through: NaiveDate,
        report: Regenerated,
    },
    /// Refused: the recurring transaction ends on this date, and extending
    /// how far ahead it is written out cannot outlive it. Moving the end is
    /// the form's decision, not `x`'s.
    Ends(NaiveDate),
    /// Refused: generation already reaches the ten-year ceiling.
    Ceiling(NaiveDate),
}

/// Push one recurring transaction one rolling horizon further out, and
/// regenerate it.
///
/// The step is [`key::RECURRING_TXN_HORIZON_MONTHS`] rather than a fixed
/// three months: it is already the answer to "how far ahead do I look", so a
/// stride of the same size keeps one press meaning one window.
///
/// The write and the regeneration share a transaction, so a failed
/// regeneration cannot leave the row claiming a reach its ledger has not got.
///
/// **Must be called at top level, not nested inside another transaction.**
pub fn extend(db: &Db, id: RecurringTxnId, today: NaiveDate) -> Result<Extended> {
    let recurring_txn = recurring_txn::get(db, id)?;
    let reach = reach(db, &recurring_txn, today)?;
    if let Some(end) = recurring_txn.horizon
        && end <= reach
    {
        return Ok(Extended::Ends(end));
    }
    let ceiling = furthest(today)?;
    if reach >= ceiling {
        return Ok(Extended::Ceiling(reach));
    }
    let floor = add_months(reach, horizon_months(db)?)?.min(ceiling);
    let extended = RecurringTxn {
        generate_through: Some(floor),
        ..recurring_txn
    };
    // What the rows will actually reach, which is the floor cut short by the
    // recurring transaction's own end. Read from `horizon` rather than
    // recomputed, so the message and the generation cannot drift: quoting the
    // floor would overstate the reach right up until the next press refused.
    let through = horizon(db, &extended, today)?;
    let report = db.transaction(|db| {
        recurring_txn::set_generate_through(db, id, floor)?;
        regenerate_within(db, &extended, today)
    })?;
    Ok(Extended::Through { through, report })
}

/// Regenerate one recurring transaction's rows, inside one transaction.
///
/// **Must be called at top level, not nested inside another transaction.**
pub fn regenerate(db: &Db, id: RecurringTxnId, today: NaiveDate) -> Result<Regenerated> {
    let recurring_txn = recurring_txn::get(db, id)?;
    db.transaction(|db| regenerate_within(db, &recurring_txn, today))
}

/// Regenerate every recurring transaction, inside one transaction.
///
/// **Must be called at top level, not nested inside another transaction.**
/// It cannot delegate to [`regenerate`], which opens one of its own --
/// `Db::transaction` is not reentrant, the same shape
/// `db::clear_imported_data` already relies on.
pub fn regenerate_all(db: &Db, today: NaiveDate) -> Result<Regenerated> {
    let recurring_txn = recurring_txn::list(db)?;
    db.transaction(|db| {
        let mut total = Regenerated::default();
        for recurring_txn in &recurring_txn {
            total = total.add(regenerate_within(db, recurring_txn, today)?);
        }
        Ok(total)
    })
}

/// Release, adopt, insert -- and open no transaction, because both public
/// forms already have.
fn regenerate_within(
    db: &Db,
    recurring_txn: &RecurringTxn,
    today: NaiveDate,
) -> Result<Regenerated> {
    // 1. Release what it owns, forward only. History is never rewritten, and
    //    a corrected row is never clobbered.
    let removed = recurring_txn::release_generated(db, recurring_txn.id, today)?;

    // Whatever the recurring transaction still owns is exactly what step 1 was
    // told not to touch: a hand-corrected row that already accounts for its
    // occurrence. `adopt`'s `recurring_txn_id IS NULL` guard would never
    // reclaim it anyway, so asking would only end in a duplicate insert for
    // the same date.
    let owned = recurring_txn::owned_dates(db, recurring_txn.id, today)?;
    let to = horizon(db, recurring_txn, today)?;
    let occurrences = schedule::occurrences(
        recurring_txn.anchor_date,
        step(recurring_txn.cadence),
        today,
        to,
    )?;
    let scheduled: HashSet<NaiveDate> = occurrences.iter().copied().collect();

    // 1b. A correction the schedule no longer produces accounts for nothing,
    //     so it goes back to the ledger unowned rather than blocking the
    //     insert of the occurrence it was moved off. Bounded by `to`: a row
    //     owned beyond a shrunken horizon is out of this run's reach, not
    //     stranded.
    let stranded: Vec<NaiveDate> = owned
        .iter()
        .copied()
        .filter(|date| *date <= to && !scheduled.contains(date))
        .collect();
    let released = recurring_txn::release_dates(db, recurring_txn.id, &stranded)?;

    let mut report = Regenerated {
        removed,
        released,
        ..Regenerated::default()
    };
    // What is left after 1b: the occurrences this recurring transaction
    // already accounts for.
    let already_owned: HashSet<NaiveDate> = owned
        .into_iter()
        .filter(|date| scheduled.contains(date))
        .collect();
    for date in occurrences {
        if already_owned.contains(&date) {
            continue;
        }
        // 2. Adopt a matching unclaimed row -- the workbook already holds
        //    every future occurrence as an ordinary row -- before
        // 3. inserting, when there was nothing to adopt.
        let adopted = recurring_txn::adopt(
            db,
            recurring_txn.id,
            date,
            recurring_txn.account_id,
            &recurring_txn.description,
            recurring_txn.cents,
        )?;
        if adopted {
            report.adopted += 1;
        } else {
            txn::insert(
                db,
                &txn::NewTxn {
                    date,
                    cents: recurring_txn.cents,
                    account_id: recurring_txn.account_id,
                    description: recurring_txn.description.clone(),
                    recurring_txn_id: Some(recurring_txn.id),
                },
            )?;
            report.inserted += 1;
        }
    }
    Ok(report)
}

/// The next paycheck after `today`, from whichever recurring transaction
/// carries the flag.
///
/// `None` when no recurring transaction does. A freshly imported database is
/// in that state until the owner enters one, which is visible and correctable
/// rather than a wrong number presented as right.
///
/// **`recurring_txn.horizon` is deliberately ignored**, so a paycheck
/// recurring transaction past its end date still yields a date beyond it. The
/// horizon governs how far ahead rows are generated, not the pay rhythm;
/// honouring it here would collapse Paycheck-Eve onto To-Date the day the
/// recurring transaction lapsed, which is a wrong number where the fallback at
/// least announces itself.
pub fn next_paycheck(db: &Db, today: NaiveDate) -> Result<Option<NaiveDate>> {
    let Some(recurring_txn) = recurring_txn::paycheck(db)? else {
        return Ok(None);
    };
    Ok(schedule::next_after(
        recurring_txn.anchor_date,
        step(recurring_txn.cadence),
        today,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Kind};
    use crate::db::recurring_txn::{Cadence, NewRecurringTxn};
    use crate::db::txn::{self, Filter, NewTxn, Txn};
    use crate::db::{self, AccountId};
    use crate::money::Cents;
    use crate::test_support::day;

    fn today() -> NaiveDate {
        day(2026, 8, 16)
    }

    fn all_cash() -> Filter {
        Filter {
            kind: Kind::Cash,
            account_id: None,
            from: day(2000, 1, 1),
            to: day(2100, 1, 1),
        }
    }

    fn rows(db: &Db) -> Vec<Txn> {
        txn::list(db, &all_cash()).unwrap()
    }

    fn mortgage(account_id: AccountId) -> NewRecurringTxn {
        NewRecurringTxn {
            description: "Mortgage".to_string(),
            cents: Cents(-120_000),
            account_id,
            cadence: Cadence::Monthly,
            anchor_date: day(2026, 9, 1),
            horizon: Some(day(2026, 12, 1)),
        }
    }

    /// A database with one recurring transaction and no ledger rows at all.
    fn fresh() -> (Db, AccountId, RecurringTxnId) {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let id = recurring_txn::insert(&db, &mortgage(checking)).unwrap();
        (db, checking, id)
    }

    fn write(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64, what: &str) {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: Cents(cents),
                account_id,
                description: what.to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    /// The rolling horizon is three months, so from 2026-08-16 the mortgage
    /// generates 09-01, 10-01 and 11-01 -- and not 12-01, which the recurring
    /// transaction's own horizon still permits.
    #[test]
    fn a_first_regenerate_inserts_every_occurrence_inside_the_rolling_horizon() {
        let (db, _, id) = fresh();

        let report = regenerate(&db, id, today()).unwrap();

        assert_eq!(
            report,
            Regenerated {
                removed: 0,
                released: 0,
                adopted: 0,
                inserted: 3
            }
        );
        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(
            dates,
            vec![day(2026, 9, 1), day(2026, 10, 1), day(2026, 11, 1)]
        );
        assert!(rows(&db).iter().all(|t| t.recurring_txn_id == Some(id)));
        assert!(rows(&db).iter().all(|t| !t.edited));
    }

    /// The database is seeded by importing the workbook, which already holds
    /// every future occurrence as an ordinary row with a null
    /// `recurring_txn_id`. Without adoption the first `g` writes a second
    /// 2026-09-01 Mortgage and Overview's Month-End column is quietly wrong by
    /// 1,200.00.
    #[test]
    fn regenerate_adopts_an_unclaimed_row_instead_of_inserting_a_duplicate() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 10, 1), -120_000, "Mortgage");

        let report = regenerate(&db, id, today()).unwrap();

        assert_eq!(report.adopted, 2);
        assert_eq!(report.inserted, 1);
        assert_eq!(rows(&db).len(), 3, "nothing was duplicated");
    }

    /// A pre-entered 2026-08-14 paycheck of 4,999.99 against the recurring
    /// transaction's 5,000.00. Adopting it as `edited` is what keeps step 1
    /// from taking it back on the next run.
    #[test]
    fn an_adopted_row_whose_amount_differs_is_flagged_edited() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -294_400, "Mortgage");
        write(&db, checking, day(2026, 10, 1), -120_000, "Mortgage");

        regenerate(&db, id, today()).unwrap();

        let found = rows(&db);
        let corrected = found.iter().find(|t| t.date == day(2026, 9, 1)).unwrap();
        let exact = found.iter().find(|t| t.date == day(2026, 10, 1)).unwrap();
        assert!(corrected.edited);
        assert!(!exact.edited);
    }

    #[test]
    fn a_hand_edited_row_survives_a_regenerate_unchanged() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -300_000, "Mortgage");
        regenerate(&db, id, today()).unwrap();

        regenerate(&db, id, today()).unwrap();

        let found = rows(&db);
        let kept = found.iter().find(|t| t.date == day(2026, 9, 1)).unwrap();
        assert_eq!(kept.cents, Cents(-300_000), "the correction was clobbered");
        assert!(kept.edited);
        assert_eq!(found.len(), 3, "and it was not duplicated either");
    }

    /// The property every other guarantee here reduces to.
    #[test]
    fn regenerating_twice_in_a_row_produces_identical_rows() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");
        regenerate(&db, id, today()).unwrap();
        let once: Vec<(NaiveDate, Cents, bool)> = rows(&db)
            .iter()
            .map(|t| (t.date, t.cents, t.edited))
            .collect();

        let second = regenerate(&db, id, today()).unwrap();

        let twice: Vec<(NaiveDate, Cents, bool)> = rows(&db)
            .iter()
            .map(|t| (t.date, t.cents, t.edited))
            .collect();
        assert_eq!(once, twice);
        assert_eq!(second.adopted, 0, "there is nothing unclaimed left");
    }

    /// The idempotence guarantee holds in the presence of a correction, not
    /// only in its absence: a third consecutive regenerate must not
    /// duplicate the hand-edited row either.
    #[test]
    fn regenerating_three_times_with_a_hand_edited_row_still_produces_identical_rows() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -300_000, "Mortgage");
        regenerate(&db, id, today()).unwrap();
        regenerate(&db, id, today()).unwrap();
        let twice: Vec<(NaiveDate, Cents, bool)> = rows(&db)
            .iter()
            .map(|t| (t.date, t.cents, t.edited))
            .collect();

        let third = regenerate(&db, id, today()).unwrap();

        let thrice: Vec<(NaiveDate, Cents, bool)> = rows(&db)
            .iter()
            .map(|t| (t.date, t.cents, t.edited))
            .collect();
        assert_eq!(twice, thrice);
        assert_eq!(third.adopted, 0, "there is nothing unclaimed left");
        assert_eq!(rows(&db).len(), 3, "the hand-edited row was not duplicated");
    }

    /// Moving a recurring transaction-owned row through the ledger -- the
    /// mortgage paid on the 5th rather than the 1st -- flags it `edited` at
    /// its new date. The recurring transaction still generates the 1st, so
    /// without step 1b the row is exempt from `release_generated`, matches no
    /// occurrence, and a fresh row appears at the original date: four rows for
    /// three occurrences, and every projected balance from there one payment
    /// too low.
    #[test]
    fn a_rule_owned_row_moved_off_its_occurrence_is_released_rather_than_duplicated() {
        let (db, _, id) = fresh();
        regenerate(&db, id, today()).unwrap();
        let moved = rows(&db)
            .into_iter()
            .find(|t| t.date == day(2026, 9, 1))
            .unwrap();
        let mut edit = moved.clone().into_new();
        edit.date = day(2026, 9, 5);
        txn::update(&db, moved.id, &edit).unwrap();

        let report = regenerate(&db, id, today()).unwrap();

        assert_eq!(
            report,
            Regenerated {
                // 10-01 and 11-01 are unedited, so they are rewritten as
                // always; 09-05 is the released one, and 09-01 comes back.
                removed: 2,
                released: 1,
                adopted: 0,
                inserted: 3
            }
        );
        let found = rows(&db);
        let given_back = found.iter().find(|t| t.date == day(2026, 9, 5)).unwrap();
        assert_eq!(
            given_back.recurring_txn_id, None,
            "the moved row is nobody's now"
        );
        assert!(!given_back.edited);
        assert_eq!(
            given_back.cents,
            Cents(-120_000),
            "the row itself is untouched"
        );
        let owned: Vec<NaiveDate> = found
            .iter()
            .filter(|t| t.recurring_txn_id == Some(id))
            .map(|t| t.date)
            .collect();
        assert_eq!(
            owned,
            vec![day(2026, 9, 1), day(2026, 10, 1), day(2026, 11, 1)],
            "the recurring transaction owns its three occurrences and nothing else"
        );
        assert_eq!(
            regenerate(&db, id, today()).unwrap().released,
            0,
            "and there is nothing left to release"
        );
    }

    /// The other way in: `e` on the Recurring Transactions screen moves the
    /// whole schedule, and the correction from the old one is released the
    /// next time `g` runs.
    #[test]
    fn moving_a_rules_anchor_releases_the_correction_left_on_the_old_schedule() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 9, 1), -300_000, "Mortgage");
        regenerate(&db, id, today()).unwrap();
        let mut moved = mortgage(checking);
        moved.anchor_date = day(2026, 9, 15);
        recurring_txn::update(&db, id, &moved).unwrap();

        let report = regenerate(&db, id, today()).unwrap();

        assert_eq!(report.released, 1);
        let stranded = rows(&db)
            .into_iter()
            .find(|t| t.date == day(2026, 9, 1))
            .expect("the correction is released, never deleted");
        assert_eq!(stranded.recurring_txn_id, None);
        assert_eq!(stranded.cents, Cents(-300_000));
    }

    /// Step 1b is bounded by the run's own horizon, so shrinking the horizon
    /// does not sweep up rows beyond it as a side effect. They are out of
    /// this run's reach, not stranded.
    #[test]
    fn a_correction_beyond_a_shrunken_horizon_is_left_owned() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 11, 1), -300_000, "Mortgage");
        regenerate(&db, id, today()).unwrap();
        setting::set(&db, key::RECURRING_TXN_HORIZON_MONTHS, 1).unwrap();

        let report = regenerate(&db, id, today()).unwrap();

        assert_eq!(report.released, 0);
        let beyond = rows(&db)
            .into_iter()
            .find(|t| t.date == day(2026, 11, 1))
            .unwrap();
        assert_eq!(beyond.recurring_txn_id, Some(id));
        assert!(beyond.edited);
    }

    /// Rows the workbook pre-entered beyond the horizon stay unclaimed and
    /// untouched: step 1 only ever deletes rows the recurring transaction
    /// owns, and the recurring transaction never owns anything past its
    /// horizon.
    #[test]
    fn nothing_is_generated_past_the_horizon_or_before_today() {
        let (db, checking, id) = fresh();
        write(&db, checking, day(2026, 12, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 7, 1), -120_000, "Mortgage");

        regenerate(&db, id, today()).unwrap();

        let untouched: Vec<NaiveDate> = rows(&db)
            .iter()
            .filter(|t| t.recurring_txn_id.is_none())
            .map(|t| t.date)
            .collect();
        assert_eq!(untouched, vec![day(2026, 7, 1), day(2026, 12, 1)]);
    }

    /// The recurring transaction's own horizon caps the rolling one, not the
    /// other way round.
    #[test]
    fn a_rules_own_horizon_cuts_the_run_short() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = Some(day(2026, 10, 1));
        let id = recurring_txn::insert(&db, &new).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates, vec![day(2026, 9, 1), day(2026, 10, 1)]);
    }

    /// A recurring transaction that does not end still stops at the rolling
    /// horizon.
    #[test]
    fn a_rule_with_no_horizon_stops_at_the_rolling_one() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();

        regenerate(&db, id, today()).unwrap();

        assert_eq!(rows(&db).len(), 3);
    }

    /// A user-editable setting reaching a date computation gets the same
    /// treatment as one reaching a divisor.
    #[test]
    fn a_non_positive_horizon_setting_is_clamped_to_one_month() {
        let (db, _, id) = fresh();
        setting::set(&db, key::RECURRING_TXN_HORIZON_MONTHS, 0).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates, vec![day(2026, 9, 1)]);
    }

    /// The upper clamp matters as much as the lower one: every occurrence
    /// inside the horizon is a row written in one transaction.
    #[test]
    fn a_horizon_setting_past_ten_years_is_clamped_to_ten_years() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();
        setting::set(&db, key::RECURRING_TXN_HORIZON_MONTHS, 12_000).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.len(), 120, "one a month for ten years, and no more");
        assert_eq!(dates.last(), Some(&day(2036, 8, 1)));
    }

    /// The extension is a floor on the rolling horizon: the point of `x` is
    /// to write out rows the three-month window does not reach.
    #[test]
    fn an_extension_past_the_rolling_horizon_generates_out_to_it() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();
        recurring_txn::set_generate_through(&db, id, day(2027, 2, 16)).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.last(), Some(&day(2027, 2, 1)));
        assert_eq!(dates.len(), 6);
    }

    /// A floor only ever raises. An extension the rolling horizon has since
    /// overtaken is spent, not a cap that pulls generation back in.
    #[test]
    fn an_extension_the_rolling_horizon_has_overtaken_changes_nothing() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();
        recurring_txn::set_generate_through(&db, id, day(2026, 9, 16)).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(
            dates,
            vec![day(2026, 9, 1), day(2026, 10, 1), day(2026, 11, 1)]
        );
    }

    /// Extending says how far ahead to look, never that the recurring
    /// transaction outlives its own end.
    #[test]
    fn a_rules_own_horizon_still_cuts_an_extension_short() {
        let (db, _, id) = fresh();
        recurring_txn::set_generate_through(&db, id, day(2027, 2, 16)).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.last(), Some(&day(2026, 12, 1)));
    }

    /// The extension reaches the same date computation the setting does, and
    /// takes the same upper clamp: ten years of rows, written in one
    /// transaction, is already more than any screen shows.
    #[test]
    fn an_extension_past_ten_years_is_clamped_to_ten_years() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();
        recurring_txn::set_generate_through(&db, id, day(2100, 1, 1)).unwrap();

        regenerate(&db, id, today()).unwrap();

        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.len(), 120, "one a month for ten years, and no more");
        assert_eq!(dates.last(), Some(&day(2036, 8, 1)));
    }

    /// One press buys one rolling horizon's worth past where generation
    /// already reaches: from 2026-08-16 the window ends 2026-11-16, so
    /// extending carries it to 2027-02-16 and the rows out to 2027-02-01.
    #[test]
    fn extending_generates_one_more_horizon_of_rows() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();

        let extended = extend(&db, id, today()).unwrap();

        let Extended::Through { through, report } = extended else {
            panic!("extending an endless recurring transaction is not refused: {extended:?}");
        };
        assert_eq!(through, day(2027, 2, 16));
        assert_eq!(
            report.inserted, 6,
            "the counts are a whole regeneration's, as `g`'s are -- release then insert"
        );
        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.last(), Some(&day(2027, 2, 1)));
    }

    /// The extension is a date, not a flag: pressing again moves it on from
    /// where it already stands rather than recomputing the same date.
    #[test]
    fn extending_twice_reaches_two_horizons_out() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();

        extend(&db, id, today()).unwrap();
        let extended = extend(&db, id, today()).unwrap();

        let Extended::Through { through, .. } = extended else {
            panic!("the second extension is not refused: {extended:?}");
        };
        assert_eq!(through, day(2027, 5, 16));
        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.last(), Some(&day(2027, 5, 1)));
    }

    /// The mortgage ends 2026-12-01, which the rolling window has not reached
    /// yet: the first press is what writes that last occurrence out.
    #[test]
    fn extending_reaches_a_rules_own_end_and_then_refuses() {
        let (db, _, id) = fresh();

        let first = extend(&db, id, today()).unwrap();
        let second = extend(&db, id, today()).unwrap();

        assert!(
            matches!(first, Extended::Through { .. }),
            "the end was still ahead of the window: {first:?}"
        );
        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(dates.last(), Some(&day(2026, 12, 1)));
        assert_eq!(
            second,
            Extended::Ends(day(2026, 12, 1)),
            "nothing is left to generate past the date it ends"
        );
    }

    /// The status line quotes where the rows now stop, not where the floor
    /// landed. The two part company whenever the recurring transaction's own
    /// end binds first, and a message overstating the reach by two months is
    /// contradicted by the very next press refusing.
    #[test]
    fn the_reported_reach_is_where_the_rows_stop() {
        let (db, _, id) = fresh();

        let extended = extend(&db, id, today()).unwrap();

        let Extended::Through { through, .. } = extended else {
            panic!("the end was still ahead of the window: {extended:?}");
        };
        let dates: Vec<NaiveDate> = rows(&db).iter().map(|t| t.date).collect();
        assert_eq!(through, day(2026, 12, 1));
        assert_eq!(dates.last(), Some(&through));
    }

    /// Refusing writes nothing at all -- neither the extension nor a round of
    /// regeneration that would report zeroes.
    #[test]
    fn a_refused_extension_leaves_the_rule_untouched() {
        let (db, _, id) = fresh();
        extend(&db, id, today()).unwrap();
        let before = recurring_txn::get(&db, id).unwrap();

        extend(&db, id, today()).unwrap();

        assert_eq!(recurring_txn::get(&db, id).unwrap(), before);
    }

    /// Ten years is the ceiling on generation however it is reached, so the
    /// press that would cross it is refused rather than clamped into a report
    /// of nothing inserted.
    #[test]
    fn extending_past_ten_years_is_refused() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let mut new = mortgage(checking);
        new.horizon = None;
        let id = recurring_txn::insert(&db, &new).unwrap();
        recurring_txn::set_generate_through(&db, id, day(2036, 8, 16)).unwrap();

        let extended = extend(&db, id, today()).unwrap();

        assert_eq!(extended, Extended::Ceiling(day(2036, 8, 16)));
    }

    /// `G` is one action and reports one line.
    #[test]
    fn regenerate_all_sums_its_counters_across_every_rule() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let first = recurring_txn::insert(&db, &mortgage(checking)).unwrap();
        let mut hoa = mortgage(checking);
        hoa.description = "HOA".to_string();
        hoa.cents = Cents(-30_000);
        recurring_txn::insert(&db, &hoa).unwrap();
        write(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");

        let report = regenerate_all(&db, today()).unwrap();

        assert_eq!(report.adopted, 1);
        assert_eq!(report.inserted, 5);
        assert_eq!(report.removed, 0);
        assert_eq!(rows(&db).len(), 6);
        assert_eq!(
            recurring_txn::owned_counts(&db).unwrap().get(&first),
            Some(&3)
        );
    }

    #[test]
    fn regenerating_a_rule_that_does_not_exist_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(regenerate(&db, RecurringTxnId(999), today()).is_err());
    }

    #[test]
    fn the_next_paycheck_comes_from_the_flagged_rule() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        recurring_txn::insert(&db, &mortgage(checking)).unwrap();
        let pay = recurring_txn::insert(
            &db,
            &NewRecurringTxn {
                description: "Salary".to_string(),
                cents: Cents(500_000),
                account_id: checking,
                cadence: Cadence::Biweekly,
                anchor_date: day(2026, 8, 28),
                horizon: None,
            },
        )
        .unwrap();
        recurring_txn::set_paycheck(&db, pay).unwrap();

        assert_eq!(next_paycheck(&db, today()).unwrap(), Some(day(2026, 8, 28)));
        assert_eq!(
            next_paycheck(&db, day(2026, 8, 28)).unwrap(),
            Some(day(2026, 9, 11)),
            "on payday itself the runway is to the next one"
        );
    }

    /// A freshly imported database has no paycheck recurring transaction until
    /// the owner enters one. That is a visible, correctable state rather than
    /// a wrong number presented as right.
    #[test]
    fn there_is_no_next_paycheck_without_a_paycheck_rule() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        recurring_txn::insert(&db, &mortgage(checking)).unwrap();
        assert_eq!(next_paycheck(&db, today()).unwrap(), None);
    }
}
