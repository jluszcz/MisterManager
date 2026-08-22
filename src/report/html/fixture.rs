//! One snapshot every tab's tests draw from.
//!
//! Shared rather than one per module, because six builders would drift into
//! six different databases and a page-wide assertion could then hold on none
//! of them. Every figure here is invented; see the no-real-data rule in
//! `CLAUDE.md`.

use crate::calc::planning::{PlanInputs, PlanSettings};
use crate::db::account::{Account, AccountColor, Group, Kind};
use crate::db::{AccountId, FundId, GoalId};
use crate::fund::{Allocation, FundRow};
use crate::money::Cents;
use crate::overview::{Balances, Band, Line, Overview, Section};
use crate::projection::Dates;
use crate::rate::{BasisPoints, Percent};
use crate::report::{
    BillRow, Container, Ledger, LedgerMonth, LedgerRow, PlanView, Planning, Snapshot, Transfer,
};
use chrono::{Local, NaiveDate, TimeZone};

pub(super) fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

pub(super) fn accounts() -> Vec<Account> {
    vec![Account {
        id: AccountId(1),
        code: "SAV".into(),
        name: "Rainy Day".into(),
        kind: Kind::Cash,
        sort: 0,
        group: Group::Checking,
        color: Some(AccountColor::Teal),
    }]
}

fn labelled() -> crate::account_label::Account {
    crate::account_label::Account::named(&accounts(), AccountId(1))
}

fn balances(c: i64) -> Balances {
    Balances {
        to_date: Cents::from_dollars(c),
        adhoc: Cents::from_dollars(c),
        month_end: Cents::from_dollars(c),
    }
}

pub(super) fn row(name: &str, current: i64, goal: i64) -> crate::savings::Row {
    crate::savings::Row {
        goal_id: GoalId(1),
        container: labelled(),
        name: name.to_string(),
        current: Cents::from_dollars(current),
        goal: Cents::from_dollars(goal),
        base: Cents::from_dollars(goal),
        taxed: false,
        percent: Some(Percent(50)),
        goal_date: Some(day(2027, 1, 1)),
        expired: false,
        per_paycheck: Some(Cents::from_dollars(25)),
        interest_eligible: false,
        favorite: false,
    }
}

/// A transaction on a given day, in whole dollars.
pub(super) fn txn(date: NaiveDate, description: &str, dollars: i64, future: bool) -> LedgerRow {
    LedgerRow {
        date,
        account: labelled(),
        description: description.to_string(),
        cents: Cents::from_dollars(dollars),
        future,
    }
}

/// Two months of one ledger, so the filter has something to switch between.
/// The later of them is the month the fixture's `today` falls in.
pub(super) fn ledger(kind: Kind) -> Ledger {
    Ledger {
        kind,
        current: "2026-08".into(),
        months: vec![
            LedgerMonth {
                key: "2026-07".into(),
                label: "Jul 2026".into(),
                rows: vec![txn(day(2026, 7, 14), "Groceries", -80, false)],
            },
            LedgerMonth {
                key: "2026-08".into(),
                label: "Aug 2026".into(),
                rows: vec![
                    txn(day(2026, 8, 3), "Paycheck", 2_500, false),
                    txn(day(2026, 8, 28), "Rent", -1_200, true),
                ],
            },
        ],
    }
}

/// A waterfall over invented inputs, computed rather than written down: the
/// figures a hand-built `Plan` carried would be a set of numbers nothing had
/// to agree with.
pub(super) fn plan_view() -> PlanView {
    let dollars = Cents::from_dollars;
    let settings = PlanSettings {
        target: dollars(10_000),
        buffer: dollars(5_000),
        periods_per_year: 26,
        bill_payment_cap: dollars(1_800),
        bill_payment_pct: Percent(40),
        mom_and_dad_annual: dollars(12_000),
        goals_floor: dollars(400),
        future_housing_pct: Percent(30),
        retirement_pct: Percent(20),
        investment_pct: Percent(10),
    };
    let inputs = PlanInputs {
        checking_at_adhoc: dollars(20_000),
        pinned_excess: None,
        housing_monthly: vec![dollars(2_000)],
        other_bills_monthly: vec![dollars(300)],
        remaining_emergency: Cents::ZERO,
        remaining_roth: Cents::ZERO,
    };
    let plan = crate::calc::planning::compute(&settings, &inputs).unwrap();
    PlanView {
        settings,
        housing_monthly: dollars(2_000),
        housing: vec![BillRow {
            label: "Mortgage".into(),
            monthly: dollars(2_000),
            biweekly: dollars(923),
        }],
        other_bills: vec![BillRow {
            label: "Internet".into(),
            monthly: dollars(300),
            biweekly: dollars(139),
        }],
        transfers: Ok(vec![
            Transfer {
                to: Some(labelled()),
                label: "Rainy Day".into(),
                cents: dollars(1_000),
                lines: vec![("Goals", dollars(1_000))],
            },
            Transfer {
                to: None,
                label: "Withdrawal".into(),
                cents: dollars(500),
                lines: vec![("Retirement", dollars(500))],
            },
        ]),
        plan,
    }
}

pub(super) fn funds() -> Allocation {
    let fund = |id, name: &str, actual: i64, target, share| FundRow {
        id: FundId(id),
        name: name.into(),
        actual: Cents::from_dollars(actual),
        target,
        actual_share: BasisPoints(share),
        delta: target.map(|t: BasisPoints| BasisPoints((t.0 - share).max(0))),
    };
    Allocation {
        rows: vec![
            fund(1, "Stocks", 6_000, Some(BasisPoints(7_000)), 6_000),
            fund(2, "Bonds", 4_000, Some(BasisPoints(3_000)), 4_000),
        ],
        total: Cents::from_dollars(10_000),
        target_total: BasisPoints(10_000),
        furthest_down: Some(0),
        age: Some(36),
    }
}

/// The whole page's worth: `rows` are the one container's goals and `net` is
/// what the Overview's Net line reads.
pub(super) fn snapshot(rows: Vec<crate::savings::Row>, net: i64) -> Snapshot {
    let accounts = accounts();
    let section = |kind: Kind, group: Group, cents: i64| Section {
        kind,
        bands: vec![Band {
            group,
            lines: vec![Line {
                account: Some(crate::account_label::Account::named(
                    &accounts,
                    AccountId(1),
                )),
                label: String::new(),
                balances: balances(cents),
            }],
            total: balances(cents),
        }],
        total: balances(cents),
    };
    Snapshot {
        generated_at: Local.with_ymd_and_hms(2026, 8, 21, 14, 30, 0).unwrap(),
        overview: Overview {
            dates: Dates {
                to_date: day(2026, 8, 21),
                adhoc: day(2026, 8, 27),
                month_end: day(2026, 9, 1),
            },
            cash: section(Kind::Cash, Group::Checking, net),
            credit: section(Kind::Credit, Group::Credit, 0),
            net: balances(net),
        },
        cash: ledger(Kind::Cash),
        credit: ledger(Kind::Credit),
        containers: vec![Container {
            account: labelled(),
            rows,
            excess: Cents(23),
        }],
        planning: Planning::Resolved(Box::new(plan_view())),
        funds: funds(),
    }
}

/// One tab's panel, between its own tags.
pub(super) fn panel<'a>(page: &'a str, id: &str) -> &'a str {
    let open = format!("<section class=\"panel\" id=\"{id}-panel\">");
    let start = page.find(&open).expect("no such panel") + open.len();
    let rest = &page[start..];
    &rest[..rest.find("</section>").expect("an unclosed panel")]
}

/// Every `<table ...>...</table>` span in some html, in order.
pub(super) fn tables(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<table") {
        let after = &rest[start..];
        let open = after.find('>').expect("an unclosed <table") + 1;
        let end = after.find("</table>").expect("a <table> with no </table>");
        out.push(&after[open..end]);
        rest = &after[end + "</table>".len()..];
    }
    out
}

/// The column count of every `<tr>...</tr>` in one table, `<td>` and `<th>`
/// counted together so a header row is checked against the body rows below
/// it, and a `colspan` counted as the columns it actually spans.
pub(super) fn row_column_counts(table: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = table;
    while let Some(start) = rest.find("<tr") {
        let after = &rest[start..];
        let end = after.find("</tr>").expect("a <tr> with no </tr>");
        let row = &after[..end];
        let cells = row.matches("<td").count() + row.matches("<th").count();
        let spanned: usize = row
            .match_indices("colspan=\"")
            .map(|(i, m)| {
                let rest = &row[i + m.len()..];
                let digits = &rest[..rest.find('"').expect("an unclosed colspan")];
                digits.parse::<usize>().expect("a non-numeric colspan")
            })
            // Each spanning cell was already counted once above.
            .map(|span| span - 1)
            .sum();
        out.push(cells + spanned);
        rest = &after[end + "</tr>".len()..];
    }
    out
}
