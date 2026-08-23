//! A savings goal as it is shown: its derived columns, and what a container
//! has left unallocated.
//!
//! A peer of `plan` and `fund`. `tui::savings` adds the cursor, the filters
//! and the search on top of these rows; the report groups the same rows by
//! container.

use crate::account_label::Account;
use crate::db::account;
use crate::db::goal;
use crate::db::{AccountId, Db, GoalId};
use crate::goal::Funding;
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
    /// The target, which is what the `Goal` column shows.
    pub goal: Cents,
    /// What the table holds. Equal to `goal` unless the goal is taxed, and
    /// carried so `e` can prefill the form from the row rather than
    /// re-querying -- the reason `interest_eligible` is here too.
    pub base: Cents,
    pub taxed: bool,
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
/// `None` for a non-positive target: the base is user-editable and reaches a
/// divisor, which is the case `div_ceil` exists to refuse. The screen shows
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

/// What a container's unallocated remainder is *shown* as: the excess with
/// its cents dropped, truncated toward zero.
///
/// A container can sit a few cents out for months -- interest and rounding
/// collect there and nothing rounds them away -- and a line reporting `0.23`
/// beside a figure in the thousands is a digit of noise standing where a
/// reader looks for a real remainder. Truncating is what makes the line say
/// nothing when there is nothing to say: sub-dollar drift of either sign
/// reads `0`, and the figures that survive are the ones worth placing by
/// hand.
///
/// One function so the two sinks cannot drift: the Savings screen's
/// `Unallocated` footer and the report's `Unallocated` row show the same
/// remainder, and each decides its color from what this leaves rather than
/// from the cents behind it -- so a container sitting at `-0.23` draws a
/// plain `0` rather than a red `-0`.
pub fn unallocated(excess: Cents) -> Cents {
    excess.trunc_to_dollar()
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
///
/// The target, not the base: a goal due next paycheck asks for all it lacks,
/// and what a taxed one lacks includes the tax.
pub fn paycheck_ask(goal: &Funding, today: NaiveDate, period_days: i64) -> Result<Option<Cents>> {
    crate::calc::per_paycheck(
        goal.current,
        goal.target,
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
///
/// Every column that asks "how far along is this goal" reads `target`, so a
/// taxed goal is measured against what the item costs at the register.
pub fn rows(
    goals: Vec<Funding>,
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
            percent: percent_complete(g.current, g.target),
            expired: g.goal.goal_date.is_some_and(|d| d < today) && g.current < g.target,
            name: g.goal.name,
            current: g.current,
            goal: g.target,
            base: g.goal.base_cents,
            taxed: g.goal.taxed,
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
    ) -> Funding {
        Funding {
            goal: Goal {
                id: GoalId(id),
                name: name.to_string(),
                container_account_id: AccountId(container),
                base_cents: Cents(target),
                goal_date: date,
                recurring_goal_id: None,
                interest_eligible: true,
                closed: false,
                sort: id,
                favorite: false,
                taxed: false,
            },
            current: Cents(current),
            target: Cents(target),
        }
    }

    /// The same goal, marked. A helper rather than an eighth parameter on
    /// `goal`: every other fixture here is about a goal that is not
    /// favorited, and a `false` on each of them would say nothing.
    fn favorited(mut g: Funding) -> Funding {
        g.goal.favorite = true;
        g
    }

    /// The same goal with the base and the target pulled apart, which is the
    /// only thing a taxed goal is: every derived column must read the second.
    fn taxed(mut g: Funding, base: i64, target: i64) -> Funding {
        g.goal.base_cents = Cents(base);
        g.goal.taxed = true;
        g.target = Cents(target);
        g
    }

    /// The four rows of the design's screen mock, at its own today.
    fn goals() -> Vec<Funding> {
        vec![
            goal(1, 1, "Bill Payments", 1_300_000, 1_500_000, None),
            goal(2, 1, "Apple Watch", 48_500, 50_000, Some(day(2026, 9, 1))),
            goal(3, 1, "Dropbox", 0, 15_000, Some(day(2026, 9, 1))),
            goal(4, 2, "Emergency Savings", 10_600_195, 10_000_000, None),
        ]
    }

    /// The target -- derived from a user-editable base -- reaches a divisor.
    /// A zero target renders `--` rather than dividing.
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

    /// A goal funded to its base is 94% of the way to what the item costs, not
    /// 100% of the way to its sticker price. The screen's percentage is a
    /// target question.
    #[test]
    fn a_taxed_goals_percentage_is_measured_against_its_taxed_target() {
        let rows = rows(
            vec![taxed(
                goal(1, 1, "Couch", 100_000, 100_000, None),
                100_000,
                106_500,
            )],
            &accounts(),
            today(),
            14,
        )
        .unwrap();

        assert_eq!(rows[0].goal, Cents(106_500), "the column shows the target");
        assert_eq!(rows[0].base, Cents(100_000), "the base is carried for `e`");
        assert!(rows[0].taxed);
        assert_eq!(rows[0].percent, Some(Percent(94)));
    }

    /// Past its date and still short of the *taxed* figure is overdue. Reading
    /// the base would clear the mark on a goal that cannot yet buy the thing.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_expired_past_its_date() {
        let rows = rows(
            vec![taxed(
                goal(1, 1, "Couch", 100_000, 100_000, Some(day(2026, 1, 1))),
                100_000,
                106_500,
            )],
            &accounts(),
            today(),
            14,
        )
        .unwrap();

        assert!(rows[0].expired);
    }

    /// `$/Pay` is the third target question on this screen, and the payday
    /// worksheet prefills with the same figure through the same function. A
    /// goal dated one pay period out asks for the whole of what it lacks, and
    /// what a taxed one lacks includes the tax.
    #[test]
    fn a_taxed_goals_paycheck_ask_is_measured_against_its_taxed_target() {
        let rows = rows(
            vec![taxed(
                // today() is 2026-08-12 and the period is 14 days, so this is
                // exactly one paycheck away: one period to divide by.
                goal(1, 1, "Couch", 100_000, 100_000, Some(day(2026, 8, 26))),
                100_000,
                106_500,
            )],
            &accounts(),
            today(),
            14,
        )
        .unwrap();

        assert_eq!(rows[0].per_paycheck, Some(Cents(6_500)));
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

    /// $0.99 of drift is the workbook's steady state and reads as nothing;
    /// $1.00 is a figure the line reports. Either sign, and a negative
    /// sub-dollar remainder loses its sign along with its cents -- the whole
    /// reason the truncation happens before the color is chosen.
    #[test]
    fn sub_dollar_drift_shows_as_nothing_in_either_direction() {
        assert_eq!(unallocated(Cents(99)), Cents::ZERO);
        assert_eq!(unallocated(Cents(-99)), Cents::ZERO);
        assert_eq!(unallocated(Cents::ZERO), Cents::ZERO);
        assert_eq!(unallocated(Cents(100)), Cents(100));
        assert_eq!(unallocated(Cents(-100)), Cents(-100));
    }

    /// The cents go, the dollars stay: a real remainder is still reported in
    /// full.
    #[test]
    fn a_remainder_above_a_dollar_keeps_its_dollars() {
        assert_eq!(unallocated(Cents(250_023)), Cents(250_000));
        assert_eq!(unallocated(Cents(-250_023)), Cents(-250_000));
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
                    base_cents: Cents::from_dollars(1_000),
                    taxed: false,
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
