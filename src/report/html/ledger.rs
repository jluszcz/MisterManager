//! The Cash and Credit tabs: every transaction the report carries, and the
//! month filter over it.

use crate::db::account::Kind;
use crate::description;
use crate::palette::Sense;
use crate::report::Ledger;
use crate::report::view::{Accounts, Block, Cell, Month, Row, TabView, money as money_cell};

/// Date, Account, Description, Amount -- the ledger screen's own columns, in
/// its order.
const HEADER: [&str; 4] = ["Date", "Account", "Description", "Amount"];

/// The tab's id: `Kind::as_str`, so the tab and its panel say `cash` and
/// `credit` because the column in the database does.
fn prefix(kind: Kind) -> &'static str {
    kind.as_str()
}

/// A ledger: the months in the order the snapshot carries them, each row
/// reduced to its day, since the month's key completes the date.
///
/// Oldest first, in the months and in the rows inside each, which is the
/// ledger screen's order: a running balance is read downward. The filter
/// lists the months newest first instead -- the month someone opens the page
/// to check is the one they are standing in, and it would otherwise be the
/// last entry of a long list -- and opens on it, or on `All months` when it
/// has no rows yet: a selection matching nothing would draw an empty list and
/// no reason for it, which is what the first of a month would look like.
/// A row dated past today is `future`, dimmed exactly as the screen draws it.
pub(super) fn view(l: &Ledger, acc: &mut Accounts) -> TabView {
    let sense = Sense::of_ledger(l.kind);
    let months: Vec<Month> = l
        .months
        .iter()
        .map(|m| Month {
            key: m.key.clone(),
            label: m.label.clone(),
            rows: m
                .rows
                .iter()
                .map(|r| Row {
                    c: vec![
                        Cell::Text(r.date.format("%d").to_string()),
                        acc.cell(&r.account),
                        Cell::Text(description::render(&r.description).into_owned()),
                        money_cell(sense, r.cents.to_string(), r.cents),
                    ],
                    k: r.future.then(|| "future".to_string()),
                    h: None,
                    d: None,
                    m: None,
                })
                .collect(),
        })
        .collect();
    let opens = l
        .months
        .iter()
        .any(|m| m.key == l.current)
        .then(|| l.current.clone());
    let empty = months.is_empty().then(|| "no transactions".to_string());
    TabView {
        id: prefix(l.kind),
        blocks: vec![Block::Ledger {
            head: HEADER.map(str::to_string).to_vec(),
            months,
            opens,
            empty,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, row, snapshot};
    use super::view;
    use crate::db::account::Kind;
    use crate::report::Ledger;
    use crate::report::view::Accounts;
    use serde_json::json;

    fn block(snapshot: &crate::report::Snapshot, kind: Kind) -> serde_json::Value {
        let ledger = match kind {
            Kind::Cash => &snapshot.cash,
            _ => &snapshot.credit,
        };
        json(&view(ledger, &mut Accounts::new()))["blocks"][0].clone()
    }

    fn fixture() -> crate::report::Snapshot {
        snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000)
    }

    #[test]
    fn the_filter_opens_on_the_current_month() {
        assert_eq!(block(&fixture(), Kind::Cash)["opens"], json!("2026-08"));
    }

    #[test]
    fn a_month_with_no_rows_yet_opens_on_all_months() {
        let mut snapshot = fixture();
        snapshot.cash.current = "2026-09".into();
        assert!(block(&snapshot, Kind::Cash).get("opens").unwrap().is_null());
    }

    #[test]
    fn months_keep_the_snapshots_order() {
        let b = block(&fixture(), Kind::Cash);
        let keys: Vec<_> = b["months"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["key"].clone())
            .collect();
        assert_eq!(keys, [json!("2026-07"), json!("2026-08")]);
        assert_eq!(b["months"][1]["label"], json!("Aug 2026"));
    }

    #[test]
    fn a_credit_payment_draws_positive() {
        let snapshot = fixture();
        let cents = |kind| {
            let b = block(&snapshot, kind);
            b["months"][0]["rows"][0]["c"][3].clone()
        };
        assert_eq!(cents(Kind::Credit), json!(["-80.00", "pos"]));
        assert_eq!(cents(Kind::Cash), json!(["-80.00", "neg"]));
    }

    #[test]
    fn a_future_row_is_dimmed() {
        let b = block(&fixture(), Kind::Cash);
        let rows = &b["months"][1]["rows"];
        assert!(rows[0].get("k").is_none());
        assert_eq!(rows[1]["k"], json!("future"));
    }

    #[test]
    fn rows_carry_only_their_day_and_the_month_key_completes_the_date() {
        let b = block(&fixture(), Kind::Cash);
        assert_eq!(b["months"][1]["key"], json!("2026-08"));
        assert_eq!(b["months"][1]["rows"][0]["c"][0], json!("03"));
        assert_eq!(b["months"][1]["rows"][0]["c"][2], json!("Paycheck"));
    }

    #[test]
    fn a_row_with_no_description_reads_as_an_em_dash() {
        let mut snapshot = fixture();
        snapshot.cash.months[0].rows[0].description = String::new();
        let b = block(&snapshot, Kind::Cash);
        assert_eq!(b["months"][0]["rows"][0]["c"][2], json!("\u{2014}"));
    }

    #[test]
    fn an_empty_ledger_says_no_transactions() {
        let mut snapshot = fixture();
        snapshot.cash = Ledger {
            kind: Kind::Cash,
            months: Vec::new(),
            current: "2026-08".into(),
        };
        let b = block(&snapshot, Kind::Cash);
        assert_eq!(b["empty"], json!("no transactions"));
        assert_eq!(
            b["head"],
            json!(["Date", "Account", "Description", "Amount"])
        );
        assert_eq!(b["months"], json!([]));
    }

    #[test]
    fn one_account_on_many_rows_is_one_accounts_entry() {
        let snapshot = fixture();
        let mut acc = Accounts::new();
        let v = json(&view(&snapshot.cash, &mut acc));
        let rows = &v["blocks"][0]["months"][1]["rows"];
        assert_eq!(rows[0]["c"][1], rows[1]["c"][1]);
        assert_eq!(acc.into_vec().len(), 1);
    }

    #[test]
    fn the_tab_is_named_for_its_kind() {
        let snapshot = fixture();
        let id = |l| json(&view(l, &mut Accounts::new()))["id"].clone();
        assert_eq!(id(&snapshot.cash), json!("cash"));
        assert_eq!(id(&snapshot.credit), json!("credit"));
    }

    /// A charge on a card is the ordinary case and takes no tone at all.
    #[test]
    fn a_charge_on_the_credit_tab_takes_no_color() {
        let b = block(&fixture(), Kind::Credit);
        assert_eq!(b["months"][1]["rows"][0]["c"][3], json!("2,500.00"));
    }
}
