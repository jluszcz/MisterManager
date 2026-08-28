use crate::calc::month_end_projection;
use crate::db::Db;
use crate::recurring_txn;
use anyhow::{Context, Result};
use chrono::{Days, NaiveDate};

/// The dates every balance is quoted at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Dates {
    /// Today.
    pub to_date: NaiveDate,
    /// The eve of a paycheck: the first day-before-a-paycheck strictly after
    /// today, derived from the paycheck recurring transaction. Today when
    /// there is no such recurring transaction.
    pub adhoc: NaiveDate,
    /// `EOMONTH(max(adhoc, to_date), 0) + 1` — the first of the month after
    /// the last one the columns to its left reach into.
    pub month_end: NaiveDate,
}

impl Dates {
    pub fn new(to_date: NaiveDate, adhoc: NaiveDate) -> Dates {
        Dates {
            to_date,
            adhoc,
            // The later of the two, not the eve alone: the scrub runs
            // backwards as well, and an eve dragged into last month would put
            // the third column *before* the first -- the same inversion the
            // forward case fixes, mirrored. Month-End therefore never retreats
            // behind the one today derives, which is where it has always sat.
            month_end: month_end_projection(adhoc.max(to_date)),
        }
    }

    /// The same three dates with the ad-hoc column moved -- what the
    /// Overview's scrub does.
    ///
    /// Month-End moves with it rather than being pinned to today, because the
    /// three columns are one widening horizon and the month a paycheck eve
    /// falls in is the month the owner is being asked to plan through. Derived
    /// from today, the third column names a date *before* the second as soon
    /// as the eve crosses a month boundary -- 8/28, 9/10, 9/1 -- quoting a
    /// nearer balance under the name of the furthest column. A scrub back past
    /// the first of the month is that same inversion against To-Date, which is
    /// what [`Dates::new`] takes the later of the two dates for.
    pub fn with_adhoc(self, adhoc: NaiveDate) -> Dates {
        Dates::new(self.to_date, adhoc)
    }
}

/// Deriving `adhoc` here rather than in `recurring_txn` keeps the one
/// `Cadence -> Step` match in a single place: `projection` never learns what a
/// cadence is.
pub fn dates(db: &Db, today: NaiveDate) -> Result<Dates> {
    // Asked from tomorrow rather than from today, so that the eve is the day
    // the column *rolls over*. Asked from today, paycheck eve derives itself:
    // the next paycheck is tomorrow and the day before it is today, so the
    // column spends its last day quoting the To-Date balance beside it under
    // another name -- exactly when the runway it exists to show is shortest.
    let from = today
        .checked_add_days(Days::new(1))
        .context("the day after today ran off the calendar")?;
    let adhoc = match recurring_txn::next_paycheck(db, from)? {
        // A database with no paycheck recurring transaction is a state to
        // correct, not to guess at.
        None => today,
        Some(next) => next
            .checked_sub_days(Days::new(1))
            .context("the day before the next paycheck ran off the calendar")?,
    };
    Ok(Dates::new(today, adhoc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Kind};
    use crate::db::recurring_txn::{self, Cadence, NewRecurringTxn};
    use crate::money::Cents;
    use crate::test_support::day;

    fn with_paycheck(anchor: NaiveDate) -> Db {
        let db = crate::db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let id = recurring_txn::insert(
            &db,
            &NewRecurringTxn {
                description: "Salary".to_string(),
                cents: Cents(500_000),
                account_id: checking,
                cadence: Cadence::Biweekly,
                anchor_date: anchor,
                horizon: None,
            },
        )
        .unwrap();
        recurring_txn::set_paycheck(&db, id).unwrap();
        db
    }

    /// `Overview!E2` was a hand-typed cell holding the day before the next
    /// paycheck. This is the same figure, derived.
    #[test]
    fn the_adhoc_date_is_the_day_before_the_next_paycheck() {
        let db = with_paycheck(day(2026, 8, 28));
        let dates = dates(&db, day(2026, 8, 14)).unwrap();
        assert_eq!(dates.adhoc, day(2026, 8, 27));
        assert_eq!(dates.to_date, day(2026, 8, 14));
        assert_eq!(dates.month_end, day(2026, 9, 1));
    }

    /// The eve is what Month-End is derived from, so an eve that has crossed
    /// into September puts the column at the end of *that* month. Derived from
    /// today it would read 9/1: a third column naming an earlier date than the
    /// second one.
    #[test]
    fn month_end_follows_the_eve_into_the_next_month() {
        let db = with_paycheck(day(2026, 8, 28));
        let dates = dates(&db, day(2026, 8, 28)).unwrap();
        assert_eq!(dates.adhoc, day(2026, 9, 10));
        assert_eq!(dates.month_end, day(2026, 10, 1));
    }

    /// The scrub moves Month-End the same way, however far it is pushed: the
    /// column is a property of the eve rather than of how it got there.
    #[test]
    fn scrubbing_the_eve_carries_month_end_with_it() {
        let dates = dates(&with_paycheck(day(2026, 8, 28)), day(2026, 8, 28)).unwrap();

        let scrubbed = dates.with_adhoc(day(2026, 10, 2));

        assert_eq!(scrubbed.to_date, day(2026, 8, 28));
        assert_eq!(scrubbed.month_end, day(2026, 11, 1));
    }

    /// Backwards, the horizon stops widening rather than reversing: an eve
    /// scrubbed into July would put Month-End before To-Date, which is the
    /// forward inversion mirrored.
    #[test]
    fn scrubbing_the_eve_back_past_the_month_leaves_month_end_where_today_puts_it() {
        let dates = dates(&with_paycheck(day(2026, 8, 28)), day(2026, 8, 28)).unwrap();

        let scrubbed = dates.with_adhoc(day(2026, 7, 10));

        assert_eq!(scrubbed.month_end, day(2026, 9, 1));
    }

    /// A freshly imported database quotes Paycheck-Eve at today until the
    /// paycheck recurring transaction is entered: a visible, correctable state
    /// rather than a wrong number presented as right.
    #[test]
    fn a_database_with_no_paycheck_rule_quotes_paycheck_eve_at_today() {
        let db = crate::db::open_in_memory().unwrap();
        let dates = dates(&db, day(2026, 8, 14)).unwrap();
        assert_eq!(dates.adhoc, day(2026, 8, 14));
    }

    /// The eve is where the column turns over, not where it collapses. Derived
    /// from today, the day before the next paycheck *is* today on the eve, and
    /// the column would repeat the To-Date balance beside it for a day; the
    /// runway the owner is asked to plan against is the one to the paycheck
    /// after.
    #[test]
    fn on_paycheck_eve_the_adhoc_date_rolls_over_to_the_next_eve() {
        let db = with_paycheck(day(2026, 8, 28));
        let dates = dates(&db, day(2026, 8, 27)).unwrap();
        assert_eq!(dates.adhoc, day(2026, 9, 10));
        assert_eq!(dates.to_date, day(2026, 8, 27));
    }

    /// Payday itself quotes the same eve the day before it did: the rollover
    /// has already happened, so the paycheck landing changes nothing.
    #[test]
    fn on_payday_itself_the_adhoc_date_is_the_eve_of_the_paycheck_after() {
        let db = with_paycheck(day(2026, 8, 28));
        assert_eq!(
            dates(&db, day(2026, 8, 28)).unwrap().adhoc,
            day(2026, 9, 10)
        );
    }

    /// Two days out is the last day the column still names the coming eve.
    #[test]
    fn the_day_before_paycheck_eve_still_names_that_eve() {
        let db = with_paycheck(day(2026, 8, 28));
        assert_eq!(
            dates(&db, day(2026, 8, 26)).unwrap().adhoc,
            day(2026, 8, 27)
        );
    }
}
