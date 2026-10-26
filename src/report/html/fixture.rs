//! One snapshot every tab's tests draw from.
//!
//! Shared rather than one per module, because six builders would drift into
//! six different databases and a page-wide assertion could then hold on none
//! of them. Every figure here is invented; see the no-real-data rule in
//! `AGENTS.md`.

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

/// The next `<td`/`<th` that opens a cell, rather than merely starting with
/// those three bytes.
///
/// `<thead>` does too, and taking it for a cell swallows the first header
/// cell of every table: the entry runs from `<tr>` to the first `</th>`, so
/// the real cell inside it is never returned and never checked. A cell's tag
/// name ends at the byte after it.
fn next_cell(html: &str) -> Option<usize> {
    let mut from = 0;
    loop {
        let start = from
            + match (html[from..].find("<td"), html[from..].find("<th")) {
                (Some(d), Some(h)) => d.min(h),
                (Some(d), None) => d,
                (None, Some(h)) => h,
                (None, None) => return None,
            };
        match html.as_bytes().get(start + 3) {
            Some(&b) if b == b'>' || b.is_ascii_whitespace() => return Some(start),
            _ => from = start + 3,
        }
    }
}

/// Whether a cell's attributes name `class` among its classes -- `n` is
/// still `n` in `class="n neg"`.
pub(super) fn has_class(attrs: &str, class: &str) -> bool {
    attrs
        .split("class=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .is_some_and(|list| list.split(' ').any(|c| c == class))
}

/// Every `<td>` and `<th>` in some html, as its opening tag's attributes and
/// the text between the tags. The attributes are what a test checks a cell's
/// class against; the text is what the cell holds, tags and all.
pub(super) fn cells(html: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = next_cell(rest) {
        let (tag, after) = rest[start..].split_at(3);
        let open = after.find('>').expect("an unclosed cell");
        let body = &after[open + 1..];
        let close = format!("</{}>", &tag[1..]);
        let end = body.find(&close).expect("a cell with no closing tag");
        out.push((&after[..open], &body[..end]));
        rest = &body[end..];
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

/// The headers in one table set differently from the column under them: a
/// header that is not `n` over a column whose every body cell is, or the
/// reverse. A spanning cell is in no single column, so a row carrying one is
/// left out of the reading; an empty header labels nothing to align.
pub(super) fn misaligned_headers(table: &str) -> Vec<String> {
    let is_n = |attrs: &str| has_class(attrs, "n");
    let mut rows = Vec::new();
    let mut rest = table;
    while let Some(start) = rest.find("<tr") {
        let after = &rest[start..];
        let end = after.find("</tr>").expect("a <tr> with no </tr>");
        if !after[..end].contains("colspan") {
            rows.push(cells(&after[..end]));
        }
        rest = &after[end + "</tr>".len()..];
    }
    let Some((header, body)) = rows.split_first() else {
        return Vec::new();
    };
    if body.is_empty() {
        return Vec::new();
    }
    header
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| !text.is_empty())
        .filter(|(i, (attrs, _))| {
            let column_n = body
                .iter()
                .all(|row| row.get(*i).is_some_and(|(a, _)| is_n(a)));
            let column_left = body
                .iter()
                .all(|row| row.get(*i).is_some_and(|(a, _)| !is_n(a)));
            (column_n && !is_n(attrs)) || (column_left && is_n(attrs))
        })
        .map(|(_, (_, text))| text.to_string())
        .collect()
}

/// The page with every ledger table and goal history cut out: what is left
/// is the markup whose cells say what they may do with their width through
/// their own classes, rather than through the stylesheet's positional rules.
pub(super) fn without_positional_tables(page: &str) -> String {
    let mut rest = page;
    let mut kept = String::new();
    loop {
        let next = [
            ("<table class=\"ledger\">", "</table>"),
            // A history's rows are a `div` of their own inside it.
            ("<div class=\"history\"", "</div></div>"),
        ]
        .into_iter()
        .filter_map(|(open, close)| rest.find(open).map(|at| (at, close)))
        .min_by_key(|(at, _)| *at);
        let Some((at, close)) = next else {
            kept.push_str(rest);
            return kept;
        };
        kept.push_str(&rest[..at]);
        let end = rest[at..]
            .find(close)
            .expect("an unclosed positional table");
        rest = &rest[at + end + close.len()..];
    }
}
