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
    /// The day before the next paycheck, derived from the paycheck recurring
    /// transaction. Today when there is no such recurring transaction.
    pub adhoc: NaiveDate,
    /// `EOMONTH(today, 0) + 1`.
    pub month_end: NaiveDate,
}

/// Deriving `adhoc` here rather than in `recurring_txn` keeps the one
/// `Cadence -> Step` match in a single place: `projection` never learns what a
/// cadence is.
pub fn dates(db: &Db, today: NaiveDate) -> Result<Dates> {
    let adhoc = match recurring_txn::next_paycheck(db, today)? {
        // A database with no paycheck recurring transaction is a state to
        // correct, not to guess at.
        None => today,
        Some(next) => next
            .checked_sub_days(Days::new(1))
            .context("the day before the next paycheck ran off the calendar")?,
    };
    Ok(Dates {
        to_date: today,
        adhoc,
        month_end: month_end_projection(today),
    })
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

    /// A freshly imported database quotes Paycheck-Eve at today until the
    /// paycheck recurring transaction is entered: a visible, correctable state
    /// rather than a wrong number presented as right.
    #[test]
    fn a_database_with_no_paycheck_rule_quotes_paycheck_eve_at_today() {
        let db = crate::db::open_in_memory().unwrap();
        let dates = dates(&db, day(2026, 8, 14)).unwrap();
        assert_eq!(dates.adhoc, day(2026, 8, 14));
    }

    #[test]
    fn on_payday_itself_the_adhoc_date_is_the_eve_of_the_paycheck_after() {
        let db = with_paycheck(day(2026, 8, 28));
        assert_eq!(
            dates(&db, day(2026, 8, 28)).unwrap().adhoc,
            day(2026, 9, 10)
        );
    }
}
