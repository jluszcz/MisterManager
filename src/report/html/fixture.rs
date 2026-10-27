//! One snapshot every tab's tests draw from.
//!
//! Shared rather than one per module, because six builders would drift into
//! six different databases and a page-wide assertion could then hold on none
//! of them. Every figure here is invented; see the no-real-data rule in
//! `AGENTS.md`.

use crate::balance_history::Charts;
use crate::calc::Month;
use crate::calc::fund::Targets;
use crate::calc::planning::{PlanInputs, PlanSettings};
use crate::db::account::{Account, AccountColor, Group, Kind, TaxTreatment};
use crate::db::fund_mix::{AssetClass, Slice};
use crate::db::{AccountId, GoalId};
use crate::money::Cents;
use crate::overview::{Balances, Band, Line, Overview, Section};
use crate::plan_rows::{Bill, Transfer};
use crate::projection::Dates;
use crate::rate::{BasisPoints, Percent};
use crate::report::{
    AccountAllocation, Allocation, Container, Goal, Holding, Ledger, LedgerMonth, LedgerRow,
    PlanView, Planning, Snapshot,
};
use crate::retirement::{Retirement, rows};
use crate::test_support::day;
use chrono::{Local, NaiveDate, TimeZone};

pub(super) fn accounts() -> Vec<Account> {
    vec![
        Account {
            group: Group::Checking,
            color: Some(AccountColor::Teal),
            ..crate::test_support::cash(1, "SAV")
        },
        // The two accounts the Funds tab sections its holdings under, in two
        // colors: a section heading names its account the way a container's
        // does, and one color across both would let a section drawn under
        // the wrong name still pass for the right one.
        Account {
            color: Some(AccountColor::Copper),
            ..crate::test_support::investment(2, "BRK")
        },
        // Tax-deferred where the other is taxable, so the summary's tax
        // columns have two treatments to divide the portfolio between --
        // one treatment everywhere would let a column drawn over the wrong
        // treatment still pass.
        Account {
            color: Some(AccountColor::Violet),
            tax_treatment: Some(TaxTreatment::TaxDeferred),
            ..crate::test_support::investment(3, "RET")
        },
    ]
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
        floating: false,
        percent: Some(Percent(50)),
        goal_date: Some(day(2027, 1, 1)),
        expired: false,
        per_paycheck: Some(Cents::from_dollars(25)),
        interest_eligible: false,
        favorite: false,
        // No column on the row draws it: a note is read above the goal's
        // history, which the tests that want one set it for.
        note: None,
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
        bill_payment_pct: BasisPoints(4_000),
        mom_and_dad_annual: dollars(12_000),
        goals_floor: dollars(400),
        future_housing_pct: BasisPoints(3_000),
        retirement_pct: BasisPoints(2_000),
        investment_pct: BasisPoints(1_000),
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
        expense_constants: Vec::new(),
        housing: vec![Bill {
            id: crate::db::BillId(1),
            label: "Mortgage".into(),
            monthly: dollars(2_000),
            biweekly: dollars(923),
            counts_as_expense: false,
        }],
        other_bills: vec![Bill {
            id: crate::db::BillId(2),
            label: "Internet".into(),
            monthly: dollars(300),
            biweekly: dollars(139),
            counts_as_expense: false,
        }],
        transfers: Ok(vec![
            Transfer {
                to: Some(labelled()),
                label: "Rainy Day".into(),
                cents: dollars(1_000),
                lines: vec![(crate::plan_line::Line::Goals, dollars(1_000))],
            },
            Transfer {
                to: None,
                label: "Withdrawal".into(),
                cents: dollars(500),
                lines: vec![(crate::plan_line::Line::Retirement, dollars(500))],
            },
        ]),
        plan,
        // Nothing asked of the plug, so the Goals line covers it -- the
        // tests that are about the gap set this themselves.
        spread_ask_total: Cents::ZERO,
    }
}

/// The age rule as the fixture's portfolio is read against it: 18% bonds,
/// and 40% of what that leaves international -- 32.80% and 49.20%.
///
/// An age rather than a birth date, and an invented one: no birth year
/// appears anywhere in the crate.
pub(super) fn targets() -> Targets {
    crate::calc::fund::targets(Some(48), BasisPoints(4_000))
}

fn slices(pairs: &[(AssetClass, i64)]) -> Vec<Slice> {
    pairs
        .iter()
        .map(|(class, weight)| Slice {
            class: *class,
            weight: BasisPoints(*weight),
        })
        .collect()
}

/// One holding as the fixture writes it, before anything is apportioned over
/// it: which account it sits in, what the owner typed, and the filing behind
/// it where there is one.
struct Held<'a> {
    account: AccountId,
    ticker: &'a str,
    dollars: i64,
    filed: Option<(&'a [Slice], NaiveDate)>,
}

/// The portfolio the Funds tab draws: two accounts, four holdings, and a
/// filing for three of them.
///
/// $6,000 of a domestic index fund and $2,000 of an international one in the
/// first account, $2,000 of a bond index fund in the second, and $1,000 of a
/// fund nobody has fetched a filing for -- which is what leaves the coverage
/// line something to say and the holdings table a row to say it on. Over the
/// $10,000 the three priced holdings come to: 60% U.S. stock, 19%
/// international stock, 20% U.S. bond and 1% cash.
///
/// Apportioned rather than written down, here as in the waterfall above: the
/// shares a hand-built look-through carried would be a set of numbers nothing
/// had to agree with.
pub(super) fn funds(targets: Targets) -> Allocation {
    let (us, intl, bond) = (
        slices(&[(AssetClass::UsStock, 10_000)]),
        slices(&[(AssetClass::IntlStock, 9_500), (AssetClass::Cash, 500)]),
        slices(&[(AssetClass::UsBond, 10_000)]),
    );
    let holding = |account: i64, ticker, dollars, filed| Held {
        account: AccountId(account),
        ticker,
        dollars,
        filed,
    };
    let held = [
        holding(2, "USM", 6_000, Some((us.as_slice(), day(2026, 6, 30)))),
        holding(2, "ISM", 2_000, Some((intl.as_slice(), day(2026, 6, 30)))),
        holding(2, "UNC", 1_000, None),
        holding(3, "USB", 2_000, Some((bond.as_slice(), day(2026, 3, 31)))),
    ];
    let treatment = |id: AccountId| {
        accounts()
            .into_iter()
            .find(|a| a.id == id)
            .and_then(|a| a.tax_treatment)
    };
    let apportion = |rows: &[&Held]| {
        let mix: Vec<crate::allocation::Held<'_>> = rows
            .iter()
            .map(|h| crate::allocation::Held {
                balance: Cents::from_dollars(h.dollars),
                treatment: treatment(h.account),
                tax_free: BasisPoints::ZERO,
                mix: h.filed.map(|(mix, _)| mix),
            })
            .collect();
        crate::allocation::apportion(&mix)
    };

    let accounts = accounts();
    let section = |id: AccountId| {
        let mine: Vec<&Held> = held.iter().filter(|h| h.account == id).collect();
        let lookthrough = apportion(&mine);
        AccountAllocation {
            account: crate::account_label::Account::named(&accounts, id),
            // The loader's own rule, rather than a second copy of "empty
            // when there is nothing to look through".
            summary: crate::report::summary(&lookthrough, targets),
            lookthrough,
            holdings: mine
                .iter()
                .map(|h| Holding {
                    ticker: h.ticker.to_string(),
                    balance: Cents::from_dollars(h.dollars),
                    as_of: h.filed.map(|(_, date)| date),
                })
                .collect(),
        }
    };

    let whole: Vec<&Held> = held.iter().collect();
    let lookthrough = apportion(&whole);
    Allocation {
        summary: crate::report::summary(&lookthrough, targets),
        lookthrough,
        accounts: vec![section(AccountId(2)), section(AccountId(3))],
    }
}

/// Two allocations every fixture goal carries, one noted and one not: the
/// note column draws both of its cases on every page the tests read. They
/// come to $500 whatever the row's own `current` says, since no test here
/// asserts the two against each other.
pub(super) fn history(goal_id: GoalId) -> Vec<crate::db::goal::Allocation> {
    let allocation = |id, date, dollars, note: Option<&str>| crate::db::goal::Allocation {
        id: crate::db::AllocationId(id),
        goal_id,
        date,
        cents: Cents::from_dollars(dollars),
        note: note.map(str::to_string),
    };
    vec![
        allocation(1, day(2026, 7, 10), 300, Some("Paycheck")),
        allocation(2, day(2026, 8, 7), 200, None),
    ]
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
            dates: Dates::new(day(2026, 8, 21), day(2026, 8, 27)),
            cash: section(Kind::Cash, Group::Checking, net),
            credit: section(Kind::Credit, Group::Credit, 0),
            net: balances(net),
        },
        cash: ledger(Kind::Cash),
        credit: ledger(Kind::Credit),
        containers: vec![Container {
            account: labelled(),
            rows: rows
                .into_iter()
                .map(|row| Goal {
                    earlier: None,
                    history: history(row.goal_id),
                    row,
                })
                .collect(),
            excess: Cents(23),
        }],
        planning: Planning::Resolved(Box::new(plan_view())),
        allocation: funds(targets()),
        retirement: crate::retirement::Retirement::default(),
        charts: None,
    }
}

/// Thirty-seven, $330,000 saved against a $100,000 salary: short of
/// 3.40× by $10,000, with nothing held tax-free.
pub(super) fn retirement() -> Retirement {
    let accounts = accounts();
    let saved = Cents::from_dollars(330_000);
    let salary = Cents::from_dollars(100_000);
    Retirement {
        age: Some(37),
        salary: Some(salary),
        saved,
        tax_free: Cents::ZERO,
        held: vec![crate::retirement::Held {
            account: crate::account_label::Account::named(&accounts, AccountId(3)),
            treatment: TaxTreatment::TaxDeferred,
            tax_free: Cents::ZERO,
            tax_free_share: crate::rate::BasisPoints::ZERO,
            balance: saved,
        }],
        rows: rows(Some(37), Some(salary)),
    }
}

fn month(m: u32) -> Month {
    Month::of(crate::test_support::day(2026, m, 1))
}

/// Rainy Day alone from March through August, and two investment
/// accounts: Holdings from July, Long Haul recorded only in August. The
/// investment Total is $10K in July and $20K in August.
pub(super) fn charts() -> Charts {
    use crate::balance_history::{History, Series};
    let accounts = accounts();
    let named = |id| crate::account_label::Account::named(&accounts, AccountId(id));
    let history = History {
        cash: vec![Series {
            account: named(1),
            points: (3..=8)
                .map(|m| (month(m), Cents::from_dollars(1_000 * i64::from(m))))
                .collect(),
        }],
        credit: Vec::new(),
        investment: vec![
            Series {
                account: named(2),
                points: vec![
                    (month(7), Cents::from_dollars(10_000)),
                    (month(8), Cents::from_dollars(12_000)),
                ],
            },
            Series {
                account: named(3),
                points: vec![(month(8), Cents::from_dollars(8_000))],
            },
        ],
    };
    Charts::whole(&history).unwrap()
}

/// The page as a reader would meet it with every tab populated: a long goal
/// name and a long description to wrap, two containers, a favorite, an
/// expired goal, a dated history with an `Earlier` line, both charts and the
/// retirement standings. For looking at in a browser, so every figure is
/// invented and round.
pub(super) fn long_names_snapshot() -> Snapshot {
    let mut first = row(
        "Replace the roof, gutters and downspouts before the winter storms",
        6_500,
        12_000,
    );
    first.note = Some("Three quotes in hand; the middle one is the one to beat.".into());
    let mut favorite = row("Rainy Day", 4_000, 4_000);
    favorite.goal_id = GoalId(2);
    favorite.favorite = true;
    favorite.percent = Some(Percent(100));
    favorite.goal_date = None;
    let mut late = row("Bike", 150, 900);
    late.goal_id = GoalId(3);
    late.expired = true;
    late.percent = Some(Percent(16));
    late.goal_date = Some(day(2026, 6, 1));
    let mut floating = row("Holiday Gifts", 0, 0);
    floating.goal_id = GoalId(4);
    floating.percent = None;
    floating.goal_date = None;
    let mut snapshot = snapshot(vec![first, favorite, late, floating], 18_000);
    snapshot.containers[0].rows[0].earlier = Some(crate::report::Earlier {
        before: day(2026, 5, 1),
        count: 6,
        cents: Cents::from_dollars(6_000),
    });
    let nest = vec![Account {
        color: Some(AccountColor::Rose),
        ..crate::test_support::cash(4, "NST")
    }];
    let nest = crate::account_label::Account::named(&nest, AccountId(4));
    let mut car = row("Car", 2_500, 10_000);
    car.goal_id = GoalId(5);
    car.container = nest.clone();
    car.percent = Some(Percent(25));
    snapshot.containers.push(Container {
        account: nest,
        rows: vec![Goal {
            earlier: None,
            history: history(GoalId(5)),
            row: car,
        }],
        excess: Cents::from_dollars(-3),
    });
    snapshot.cash.months[1].rows.insert(
        1,
        txn(
            day(2026, 8, 14),
            "Hardware store, garden supplies, replacement hose and two bags of mulch",
            -70,
            false,
        ),
    );
    // A gap and a year wide enough to test the Planning tab's last column.
    if let Planning::Resolved(view) = &mut snapshot.planning {
        view.plan.shortfall.bills = Cents::from_dollars(12_345);
        view.expense_constants = vec![crate::plan_rows::Target::GoalsFloor];
        if let Ok(transfers) = &mut view.transfers {
            transfers[0]
                .lines
                .push((crate::plan_line::Line::Bills, Cents::from_dollars(300)));
        }
    }
    snapshot.retirement = retirement();
    snapshot.charts = Some(charts());
    snapshot
}

/// A view type as the JSON the script is handed, for assertions on its shape.
pub(super) fn json(v: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}
