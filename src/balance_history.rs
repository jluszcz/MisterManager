//! What each account stood at, month by month: written by every `mm` run, and
//! read back as one series per account for the Retirement screen's charts.
//!
//! **A ledger account's month is its balance on the month's last day** --
//! the 31st, the 30th, the 28th or 29th -- counting every row dated inside
//! the month, the pre-entered ones included. This is deliberately not the
//! Overview's Month-End column, which is quoted on the first of the *next*
//! month and so counts a row dated that first toward the month before it.
//! Being derivable, it is recovered: any month from an account's first row through
//! today's that has no figure yet is filled in from the ledger, and a month
//! that has one keeps it, whatever the ledger under it has since become.
//!
//! **A fund's month can only be recorded.** `holding` carries a balance and
//! no date, so there is no asking what a fund stood at last March; a month the
//! app never ran in has no investment point, and the chart draws straight
//! across it. An investment account's balance is the sum of its funds'
//! snapshots, never stored beside them.
//!
//! **Today's month is rewritten on every run**, so it ends up holding the last
//! run's figures once the month is over -- and [`load`] reads it live rather
//! than from the table, so a holding edited mid-session moves the chart
//! without waiting for the next launch to write it.

use crate::account_label::Account;
use crate::calc::Month;
use crate::db::account::{self, Kind};
use crate::db::{AccountId, Db, balance_snapshot, holding, txn};
use crate::money::Cents;
use anyhow::Result;
use chrono::NaiveDate;
use std::collections::BTreeMap;

/// One account's balances, oldest month first.
#[derive(Clone, Debug)]
pub struct Series {
    pub account: Account,
    pub points: Vec<(Month, Cents)>,
}

/// Every account with any history, by kind, each in `account::list` order.
///
/// Balances are stored figures: a credit account's is the ledger's own sum,
/// positive as debt, the way `txn::balance_at` returns it.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub cash: Vec<Series>,
    pub credit: Vec<Series>,
    pub investment: Vec<Series>,
}

/// Records this month and fills every earlier one the ledger can answer for
/// and the table has no figure for. One transaction, so a run that fails
/// partway leaves no half-written month.
pub fn take(db: &Db, today: NaiveDate) -> Result<()> {
    let current = Month::of(today);
    let past = past_ledger_months(db, current)?;
    let now = txn::ledger_balances_at(db, current.last_day())?;
    let held = holding::list(db)?;
    db.transaction(|db| {
        for &(month, id, cents) in &past {
            balance_snapshot::fill_balance(db, month, id, cents)?;
        }
        balance_snapshot::set_balances(db, current, &now)?;
        let funds: Vec<_> = held
            .iter()
            .map(|h| (h.account_id, h.ticker.as_str(), h.balance))
            .collect();
        balance_snapshot::set_holdings(db, current, &funds)
    })
}

/// Every month recorded before today's, and today's as it stands now.
pub fn load(db: &Db, today: NaiveDate) -> Result<History> {
    let current = Month::of(today);
    let mut balances: BTreeMap<AccountId, BTreeMap<Month, Cents>> = BTreeMap::new();
    for row in balance_snapshot::balances(db)?
        .into_iter()
        .filter(|r| r.month < current)
    {
        balances
            .entry(row.account_id)
            .or_default()
            .insert(row.month, row.cents);
    }
    for (id, cents) in txn::ledger_balances_at(db, current.last_day())? {
        balances.entry(id).or_default().insert(current, cents);
    }
    for row in balance_snapshot::holdings(db)?
        .into_iter()
        .filter(|r| r.month < current)
    {
        *balances
            .entry(row.account_id)
            .or_default()
            .entry(row.month)
            .or_default() += row.cents;
    }
    for h in holding::list(db)? {
        *balances
            .entry(h.account_id)
            .or_default()
            .entry(current)
            .or_default() += h.balance;
    }

    let accounts = account::list(db)?;
    let mut history = History::default();
    for a in &accounts {
        let Some(points) = balances.remove(&a.id) else {
            continue;
        };
        let series = Series {
            account: Account::named(&accounts, a.id),
            points: points.into_iter().collect(),
        };
        match a.kind {
            Kind::Cash => history.cash.push(series),
            Kind::Credit => history.credit.push(series),
            Kind::Investment => history.investment.push(series),
        }
    }
    Ok(history)
}

/// Each ledger account's balance on the last day of every month from its
/// first row up to, not including, `current` -- a month it moved nothing in
/// carrying the balance before.
fn past_ledger_months(db: &Db, current: Month) -> Result<Vec<(Month, AccountId, Cents)>> {
    let last = current.shifted(-1);
    let nets = txn::monthly_nets(db, last.last_day())?;
    let mut out = Vec::new();
    let mut rows = nets.into_iter().peekable();
    while let Some((id, first, _)) = rows.peek().copied() {
        let mut balance = Cents::ZERO;
        let mut month = first;
        while month <= last {
            while let Some(&(row_id, row_month, net)) = rows.peek() {
                if row_id != id || row_month != month {
                    break;
                }
                balance += net;
                rows.next();
            }
            out.push((month, id, balance));
            month = month.next();
        }
        // Unreachable through the date bound on `monthly_nets`, but a row
        // left behind here would start this account over and never end.
        while rows.peek().is_some_and(|(row_id, ..)| *row_id == id) {
            rows.next();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::TaxTreatment;
    use crate::db::balance_snapshot::BalanceSnapshot;
    use crate::db::{self, txn::NewTxn};
    use crate::test_support::day;

    fn today() -> NaiveDate {
        day(2026, 8, 15)
    }

    fn row(db: &Db, account_id: AccountId, date: NaiveDate, dollars: i64) {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: Cents::from_dollars(dollars),
                account_id,
                description: String::new(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    fn cash(db: &Db) -> AccountId {
        account::insert(db, "CHK", "Everyday", Kind::Cash, 0, None).unwrap()
    }

    fn brokerage(db: &Db) -> AccountId {
        account::insert(
            db,
            "BRK",
            "Holdings",
            Kind::Investment,
            0,
            Some(TaxTreatment::Taxable),
        )
        .unwrap()
    }

    fn points(series: &Series) -> Vec<(NaiveDate, Cents)> {
        series
            .points
            .iter()
            .map(|(m, c)| (m.first_day(), *c))
            .collect()
    }

    fn stored(db: &Db) -> Vec<(NaiveDate, Cents)> {
        balance_snapshot::balances(db)
            .unwrap()
            .into_iter()
            .map(|BalanceSnapshot { month, cents, .. }| (month.first_day(), cents))
            .collect()
    }

    #[test]
    fn the_first_run_fills_every_month_since_the_ledger_began() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 5, 10), 1_000);
        row(&db, chk, day(2026, 7, 3), 200);
        take(&db, today()).unwrap();
        assert_eq!(
            stored(&db),
            vec![
                (day(2026, 5, 1), Cents::from_dollars(1_000)),
                // June moved nothing and carries May's balance.
                (day(2026, 6, 1), Cents::from_dollars(1_000)),
                (day(2026, 7, 1), Cents::from_dollars(1_200)),
                (day(2026, 8, 1), Cents::from_dollars(1_200)),
            ]
        );
    }

    /// Unlike the Overview's Month-End, which is quoted on the first of the
    /// next month: a paycheck dated the first belongs to the month it is in.
    #[test]
    fn a_row_dated_the_first_counts_toward_its_own_month() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 7, 10), 1_000);
        row(&db, chk, day(2026, 8, 1), 500);
        take(&db, today()).unwrap();
        assert_eq!(
            stored(&db),
            vec![
                (day(2026, 7, 1), Cents::from_dollars(1_000)),
                (day(2026, 8, 1), Cents::from_dollars(1_500)),
            ]
        );
    }

    #[test]
    fn a_leap_february_closes_on_the_twenty_ninth() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2028, 2, 29), 1_000);
        row(&db, chk, day(2028, 3, 1), 500);
        take(&db, day(2028, 3, 15)).unwrap();
        assert_eq!(
            stored(&db),
            vec![
                (day(2028, 2, 1), Cents::from_dollars(1_000)),
                (day(2028, 3, 1), Cents::from_dollars(1_500)),
            ]
        );
    }

    #[test]
    fn a_pre_entered_row_inside_this_month_counts_and_one_past_it_does_not() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 8, 10), 1_000);
        row(&db, chk, day(2026, 8, 31), 300);
        row(&db, chk, day(2026, 9, 1), 9_000);
        take(&db, today()).unwrap();
        assert_eq!(
            stored(&db),
            vec![(day(2026, 8, 1), Cents::from_dollars(1_300))]
        );
    }

    #[test]
    fn a_month_already_recorded_keeps_its_figure_when_the_ledger_under_it_changes() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 7, 10), 1_000);
        take(&db, day(2026, 7, 20)).unwrap();
        row(&db, chk, day(2026, 7, 12), 50);
        take(&db, today()).unwrap();
        assert_eq!(
            stored(&db),
            vec![
                (day(2026, 7, 1), Cents::from_dollars(1_000)),
                (day(2026, 8, 1), Cents::from_dollars(1_050)),
            ]
        );
    }

    #[test]
    fn this_month_is_rewritten_on_every_run() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 8, 10), 1_000);
        take(&db, today()).unwrap();
        row(&db, chk, day(2026, 8, 12), 50);
        take(&db, today()).unwrap();
        assert_eq!(
            stored(&db),
            vec![(day(2026, 8, 1), Cents::from_dollars(1_050))]
        );
    }

    #[test]
    fn a_fund_is_recorded_for_this_month_only() {
        let db = db::open_in_memory().unwrap();
        let brk = brokerage(&db);
        holding::insert(&db, brk, "USM", Cents::from_dollars(4_000)).unwrap();
        take(&db, today()).unwrap();
        let funds = balance_snapshot::holdings(&db).unwrap();
        assert_eq!(funds.len(), 1);
        assert_eq!(funds[0].month.first_day(), day(2026, 8, 1));
        assert_eq!(funds[0].cents, Cents::from_dollars(4_000));
        assert!(
            stored(&db).is_empty(),
            "an investment account took a ledger row"
        );
    }

    #[test]
    fn an_investment_account_reads_as_the_sum_of_its_funds_each_month() {
        let db = db::open_in_memory().unwrap();
        let brk = brokerage(&db);
        let usm = holding::insert(&db, brk, "USM", Cents::from_dollars(4_000)).unwrap();
        holding::insert(&db, brk, "USB", Cents::from_dollars(1_000)).unwrap();
        take(&db, day(2026, 7, 20)).unwrap();
        holding::update(&db, usm, brk, "USM", Cents::from_dollars(4_500)).unwrap();
        let history = load(&db, today()).unwrap();
        assert_eq!(history.investment.len(), 1);
        assert_eq!(
            points(&history.investment[0]),
            vec![
                (day(2026, 7, 1), Cents::from_dollars(5_000)),
                (day(2026, 8, 1), Cents::from_dollars(5_500)),
            ]
        );
    }

    /// The chart reads this month off the ledger rather than the table, so a
    /// row written mid-session moves it before the next launch records it.
    #[test]
    fn this_month_is_read_live_rather_than_from_the_last_snapshot() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        row(&db, chk, day(2026, 8, 10), 1_000);
        take(&db, today()).unwrap();
        row(&db, chk, day(2026, 8, 12), 50);
        let history = load(&db, today()).unwrap();
        assert_eq!(
            points(&history.cash[0]),
            vec![(day(2026, 8, 1), Cents::from_dollars(1_050))]
        );
    }

    #[test]
    fn credit_history_is_kept_apart_from_cash() {
        let db = db::open_in_memory().unwrap();
        let chk = cash(&db);
        let card = account::insert(&db, "CC1", "Card One", Kind::Credit, 0, None).unwrap();
        row(&db, chk, day(2026, 8, 10), 1_000);
        row(&db, card, day(2026, 8, 11), 80);
        take(&db, today()).unwrap();
        let history = load(&db, today()).unwrap();
        assert_eq!(history.cash.len(), 1);
        assert_eq!(history.credit.len(), 1);
        assert_eq!(
            points(&history.credit[0]),
            vec![(day(2026, 8, 1), Cents::from_dollars(80))]
        );
    }

    /// The quit path rewrites the report only when the run wrote a row, so a
    /// snapshot that finds nothing new must not count as one.
    #[test]
    fn a_second_snapshot_with_nothing_changed_writes_no_rows() {
        let path = std::env::temp_dir().join(format!(
            "mistermanager_test_snapshot_{}.sqlite",
            std::process::id()
        ));
        let remove = || {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
        };
        remove();
        {
            let db = db::open(&path).unwrap();
            let chk = cash(&db);
            row(&db, chk, day(2026, 6, 10), 1_000);
            let brk = brokerage(&db);
            holding::insert(&db, brk, "USM", Cents::from_dollars(4_000)).unwrap();
            take(&db, today()).unwrap();
        }
        let db = db::open(&path).unwrap();
        take(&db, today()).unwrap();
        assert!(!db.wrote_rows());
        drop(db);
        remove();
    }
}
