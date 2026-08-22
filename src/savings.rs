//! A savings goal as it is shown: its derived columns, and what a container
//! has left unallocated.
//!
//! A peer of `plan` and `fund`. `tui::savings` adds the cursor, the filters
//! and the search on top of these rows; the report groups the same rows by
//! container.

use crate::db::account;
use crate::db::goal::{self, GoalWithBalance};
use crate::db::{AccountId, Db, GoalId};
use crate::label::Account;
use crate::money::Cents;
use crate::rate::Percent;
use anyhow::Result;
use chrono::NaiveDate;

/// One goal's derived columns. Computed once, so filtering and paging cannot
/// be a source of arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub goal_id: GoalId,
    /// The container this goal sits in, as the Account column shows it. One
    /// value rather than an id, a name and a color kept in step by hand.
    pub container: Account,
    pub name: String,
    pub current: Cents,
    pub goal: Cents,
    pub percent: Option<Percent>,
    pub goal_date: Option<NaiveDate>,
    /// Past its goal date and still short. What the screen offers there is a
    /// close-out, not a roll forward.
    pub expired: bool,
    pub per_paycheck: Option<Cents>,
    /// Whether an interest posting weights this goal. Carried so `e` can
    /// prefill the form from the row it already has.
    pub interest_eligible: bool,
    /// The owner's mark, drawn as a band behind the whole row. It changes
    /// nothing else: the row keeps its place under every filter and every
    /// sort, because standing out and coming first are different requests
    /// and only the first one was made.
    pub favorite: bool,
}

/// `current / goal`, as a whole percent rounded to nearest.
///
/// `None` for a non-positive target: `goal_cents` is user-editable and reaches
/// a divisor, which is the case `div_ceil` exists to refuse. The screen shows
/// `--` there rather than dividing.
///
/// `i128` because `current * 100` overflows an `i64` only above ~$922
/// trillion, but the doubling below halves even that headroom for no
/// benefit.
///
/// Uses `div_euclid`, not `/`, on the halfway-adjusted ratio: `goal` is
/// always positive here, so Euclidean division is floor division, which is
/// what "round to nearest" needs. Rust's `/` truncates toward zero instead,
/// which would round an overspent goal's negative `current` the wrong way.
pub fn percent_complete(current: Cents, goal: Cents) -> Option<Percent> {
    if goal <= Cents::ZERO {
        return None;
    }
    let goal = goal.0 as i128;
    let scaled = current.0 as i128 * 100;
    Some(Percent((scaled * 2 + goal).div_euclid(goal * 2) as i64))
}

/// How far a container's unallocated remainder may drift before it is worth
/// flagging. A container can sit at $0.23 for months; a warning that is always
/// on is a warning nobody reads.
///
/// A documented constant rather than a setting: nothing yet suggests it needs
/// to vary, and a setting would need a fallback, which is a second place for
/// it to be wrong.
pub const RECONCILED_WITHIN: Cents = Cents(100);

/// Whether a container's excess is close enough to zero to stop flagging. The
/// figure still renders either way -- this only decides the marker.
///
/// Called by the Savings screen's `Unallocated` footer, which is the one place
/// the reconciliation is shown.
pub fn is_reconciled(excess: Cents) -> bool {
    excess.0.abs() < RECONCILED_WITHIN.0
}

/// What one goal asks of this paycheck, or `None` when it asks nothing.
///
/// The one place a goal is unpacked into [`crate::calc::per_paycheck`]'s four
/// arguments. Three callers want the same number and would otherwise each
/// unpack it themselves: the Savings screen's `$/Pay` column, the payday
/// worksheet `A` prefills, and the one Planning's `t` prefills. A figure the
/// screen shows and a figure the prefill writes must be the same figure, and
/// three copies of the unpacking is how they stop being.
///
/// `None` for an undated goal -- there is no runway to divide -- and for one
/// already at its target. Both mean "nothing this paycheck", which a prefill
/// reads as zero and a column leaves blank.
pub fn paycheck_ask(
    goal: &GoalWithBalance,
    today: NaiveDate,
    period_days: i64,
) -> Result<Option<Cents>> {
    crate::calc::per_paycheck(
        goal.current,
        goal.goal.goal_cents,
        goal.goal.goal_date,
        today,
        period_days,
    )
}

/// Every open goal's derived columns, computed once.
///
/// Taken as a whole `Vec` rather than one goal at a time: the columns are
/// derived per goal, but a caller that wanted them one at a time would be a
/// caller filtering before deriving, and then two screens would disagree
/// about what a filtered-out goal's `$/Pay` was.
pub fn rows(
    goals: Vec<GoalWithBalance>,
    accounts: &[account::Account],
    today: NaiveDate,
    period_days: i64,
) -> Result<Vec<Row>> {
    let mut rows = Vec::with_capacity(goals.len());
    for g in goals {
        let per_paycheck = paycheck_ask(&g, today, period_days)?;
        rows.push(Row {
            goal_id: g.goal.id,
            container: Account::named(accounts, g.goal.container_account_id),
            percent: percent_complete(g.current, g.goal.goal_cents),
            expired: g.goal.goal_date.is_some_and(|d| d < today) && g.current < g.goal.goal_cents,
            name: g.goal.name,
            current: g.current,
            goal: g.goal.goal_cents,
            goal_date: g.goal.goal_date,
            per_paycheck,
            interest_eligible: g.goal.interest_eligible,
            favorite: g.goal.favorite,
        });
    }
    Ok(rows)
}

/// Every container holding goals, and what each has left unallocated.
///
/// In `goal::containers` order, which is the accounts' own order -- the same
/// order the Savings screen cycles its filter through.
pub fn containers_with_excess(db: &Db) -> Result<Vec<(AccountId, Cents)>> {
    let containers = goal::containers(db)?;
    let mut out = Vec::with_capacity(containers.len());
    for id in containers {
        out.push((id, goal::container_excess(db, id)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::Goal;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn today() -> NaiveDate {
        day(2026, 8, 12)
    }

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "SAV".into(),
                name: "Rainy Day".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            account::Account {
                id: AccountId(2),
                code: "BKR".into(),
                name: "Brokerage".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: None,
            },
        ]
    }

    /// `id` doubles as the sort key, so goals arrive in the order
    /// `all_with_balances` would return them.
    fn goal(
        id: i64,
        container: i64,
        name: &str,
        current: i64,
        target: i64,
        date: Option<NaiveDate>,
    ) -> GoalWithBalance {
        GoalWithBalance {
            goal: Goal {
                id: GoalId(id),
                name: name.to_string(),
                container_account_id: AccountId(container),
                goal_cents: Cents(target),
                goal_date: date,
                recurring_goal_id: None,
                interest_eligible: true,
                closed: false,
                sort: id,
                favorite: false,
            },
            current: Cents(current),
        }
    }

    /// The same goal, marked. A helper rather than an eighth parameter on
    /// `goal`: every other fixture here is about a goal that is not
    /// favorited, and a `false` on each of them would say nothing.
    fn favorited(mut g: GoalWithBalance) -> GoalWithBalance {
        g.goal.favorite = true;
        g
    }

    /// The four rows of the design's screen mock, at its own today.
    fn goals() -> Vec<GoalWithBalance> {
        vec![
            goal(1, 1, "Bill Payments", 1_300_000, 1_500_000, None),
            goal(2, 1, "Apple Watch", 48_500, 50_000, Some(day(2026, 9, 1))),
            goal(3, 1, "Dropbox", 0, 15_000, Some(day(2026, 9, 1))),
            goal(4, 2, "Emergency Savings", 10_600_195, 10_000_000, None),
        ]
    }

    /// `goal_cents` is user-editable and reaches a divisor. A zero target
    /// renders `--` rather than dividing.
    #[test]
    fn a_goal_with_a_zero_target_has_no_percent_rather_than_dividing() {
        assert_eq!(percent_complete(Cents(500), Cents::ZERO), None);
        assert_eq!(percent_complete(Cents(500), Cents(-100)), None);
        assert_eq!(
            percent_complete(Cents::ZERO, Cents(100)),
            Some(Percent::ZERO)
        );
    }

    /// An overspent goal has a negative `current`. Truncating division would
    /// round -14.9% to -14; the nearest whole percent is -15.
    #[test]
    fn an_overspent_goal_rounds_its_negative_percent_to_the_nearest_whole_percent() {
        assert_eq!(
            percent_complete(Cents(-149), Cents(1000)),
            Some(Percent(-15))
        );
    }

    /// 87%, 97%, 0%, 106% -- rounded to the nearest whole percent, not
    /// truncated, or 13,000/15,000 would read 86%.
    #[test]
    fn the_percent_column_rounds_to_the_nearest_whole_percent() {
        let rows = rows(goals(), &accounts(), today(), 14).unwrap();
        let percents: Vec<Option<Percent>> = rows.iter().map(|r| r.percent).collect();
        assert_eq!(
            percents,
            vec![
                Some(Percent(87)),
                Some(Percent(97)),
                Some(Percent::ZERO),
                Some(Percent(106))
            ]
        );
    }

    /// Column F of the sheet goes blank for undated and already-met goals.
    #[test]
    fn per_paycheck_is_blank_for_undated_and_met_goals() {
        let rows = rows(goals(), &accounts(), today(), 14).unwrap();
        assert_eq!(rows[0].per_paycheck, None, "Bill Payments is undated");
        assert_eq!(rows[1].per_paycheck, Some(Cents(800)), "Apple Watch");
        assert_eq!(rows[2].per_paycheck, Some(Cents(7_500)), "Dropbox");
        assert_eq!(rows[3].per_paycheck, None, "Emergency Savings is undated");
    }

    #[test]
    fn a_met_goal_with_a_future_date_has_no_per_paycheck_figure() {
        let rows = rows(
            vec![goal(
                1,
                1,
                "Couch",
                100_000,
                100_000,
                Some(day(2026, 12, 1)),
            )],
            &accounts(),
            today(),
            14,
        )
        .unwrap();
        assert_eq!(rows[0].per_paycheck, None);
    }

    /// A goal past its date and still short is the parent design's "flags
    /// expired goals". A goal past its date that is funded is simply done.
    #[test]
    fn only_a_past_dated_goal_that_is_still_short_is_marked_expired() {
        let rows = rows(
            vec![
                goal(1, 1, "Late", 5_000, 10_000, Some(day(2026, 7, 1))),
                goal(2, 1, "Met", 10_000, 10_000, Some(day(2026, 7, 1))),
                goal(3, 1, "Ahead", 0, 10_000, Some(day(2027, 7, 1))),
                goal(4, 1, "Undated", 0, 10_000, None),
            ],
            &accounts(),
            today(),
            14,
        )
        .unwrap();
        let expired: Vec<bool> = rows.iter().map(|r| r.expired).collect();
        assert_eq!(expired, vec![true, false, false, false]);
    }

    #[test]
    fn each_row_names_its_own_goals_container() {
        let rows = rows(goals(), &accounts(), today(), 14).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.container.text()).collect();
        assert_eq!(names, ["Rainy Day", "Rainy Day", "Rainy Day", "Brokerage"]);
    }

    /// The flag is carried on the row so the screen's cursor and the `f`
    /// toggle read the same value the database holds, rather than either
    /// re-querying.
    #[test]
    fn a_favorited_goal_arrives_on_the_row_it_belongs_to() {
        let rows = rows(
            vec![
                goal(1, 1, "Plain", 0, 10_000, None),
                favorited(goal(2, 1, "Marked", 0, 10_000, None)),
            ],
            &accounts(),
            today(),
            14,
        )
        .unwrap();
        let marked: Vec<bool> = rows.iter().map(|r| r.favorite).collect();
        assert_eq!(marked, vec![false, true]);
    }

    /// The threshold is exclusive: $0.99 of drift is the workbook's steady
    /// state, $1.00 is worth looking at.
    #[test]
    fn sub_dollar_drift_reconciles_in_either_direction() {
        assert!(is_reconciled(Cents(99)));
        assert!(is_reconciled(Cents(-99)));
        assert!(is_reconciled(Cents::ZERO));
        assert!(!is_reconciled(Cents(100)));
        assert!(!is_reconciled(Cents(-100)));
    }

    /// The screen's own order, so the report and the screen list a container's
    /// goals the same way.
    #[test]
    fn containers_come_back_in_the_accounts_own_order() {
        let db = db::open_in_memory().unwrap();
        let sav = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let nst = account::insert(&db, "NST", "Nest Egg", Kind::Cash, 1).unwrap();
        for (container, name) in [(nst, "Nest Egg goal"), (sav, "Rainy Day goal")] {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: container,
                    goal_cents: Cents::from_dollars(1_000),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: false,
                    sort: 0,
                },
            )
            .unwrap();
        }
        let containers = containers_with_excess(&db).unwrap();
        assert_eq!(
            containers.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![sav, nst]
        );
    }
}
