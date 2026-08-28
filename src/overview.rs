//! The Overview's figures: every account's balance at each of the three dates,
//! banded and subtotalled.
//!
//! A peer of `plan` and `fund` -- it reads out of `db` and derives, and knows
//! nothing about what draws it. Two things do: the Overview screen and the
//! report.

use crate::db::account::{self, Group, Kind};
use crate::db::{AccountId, Db, txn};
use crate::money::Cents;
use crate::projection::Dates;
use anyhow::Result;
use chrono::NaiveDate;
use std::collections::HashMap;
use std::iter::Sum;
use std::ops::{Add, Neg};

/// One row's three columns: To-Date, Paycheck-Eve, Month-End.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Balances {
    pub to_date: Cents,
    pub adhoc: Cents,
    pub month_end: Cents,
}

impl Add for Balances {
    type Output = Balances;
    fn add(self, rhs: Balances) -> Balances {
        Balances {
            to_date: self.to_date + rhs.to_date,
            adhoc: self.adhoc + rhs.adhoc,
            month_end: self.month_end + rhs.month_end,
        }
    }
}

impl Neg for Balances {
    type Output = Balances;
    fn neg(self) -> Balances {
        Balances {
            to_date: -self.to_date,
            adhoc: -self.adhoc,
            month_end: -self.month_end,
        }
    }
}

impl Sum for Balances {
    fn sum<I: Iterator<Item = Balances>>(iter: I) -> Balances {
        iter.fold(Balances::default(), Add::add)
    }
}

/// One labelled row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The account this row is, or `None` for a subtotal, which names a band
    /// rather than an account and takes no tint.
    pub account: Option<crate::account_label::Account>,
    /// What a subtotal row is labelled. Empty when `account` is set.
    pub label: String,
    pub balances: Balances,
}

/// One band of accounts within a kind, and its subtotal.
#[derive(Clone, Debug)]
pub struct Band {
    pub group: Group,
    pub lines: Vec<Line>,
    pub total: Balances,
}

/// Every account of one kind, in bands, with the kind's own total.
#[derive(Clone, Debug)]
pub struct Section {
    pub kind: Kind,
    /// The kind's non-empty bands, in display order. Empty bands are dropped
    /// rather than drawn at zero: a band with no accounts is one the owner
    /// does not have, not one that holds nothing.
    pub bands: Vec<Band>,
    pub total: Balances,
}

impl Section {
    /// Whether the band subtotals are worth drawing. One band's subtotal and
    /// the section's own total are the same number under two names, and the
    /// section's is the one every reader is looking for.
    pub fn breaks_down(&self) -> bool {
        self.bands.len() > 1
    }
}

/// Everything the Overview screen shows, as numbers.
///
/// Built by three `balances_at` queries — one per column — rather than one
/// query per account per column.
#[derive(Clone, Debug)]
pub struct Overview {
    pub dates: Dates,
    pub cash: Section,
    pub credit: Section,
    pub net: Balances,
}

impl Overview {
    pub fn load(db: &Db, dates: Dates) -> Result<Overview> {
        let column = |date: NaiveDate| -> Result<HashMap<AccountId, Cents>> {
            Ok(txn::balances_at(db, date)?.into_iter().collect())
        };
        let to_date = column(dates.to_date)?;
        let adhoc = column(dates.adhoc)?;
        let month_end = column(dates.month_end)?;

        let accounts = account::list(db)?;
        let mut bands: Vec<Band> = Vec::new();
        for group in Group::ALL {
            let lines: Vec<Line> = accounts
                .iter()
                .filter(|a| a.group == group)
                .map(|account| {
                    let held = |c: &HashMap<AccountId, Cents>| {
                        c.get(&account.id).copied().unwrap_or(Cents::ZERO)
                    };
                    let balances = Balances {
                        to_date: held(&to_date),
                        adhoc: held(&adhoc),
                        month_end: held(&month_end),
                    };
                    Line {
                        account: Some(crate::account_label::Account::named(&accounts, account.id)),
                        label: String::new(),
                        // Credit is stored as debt, and this screen is the
                        // only one that negates it -- which is what makes Net
                        // a single addition rather than a subtraction with a
                        // sign to get wrong.
                        balances: match account.kind {
                            Kind::Cash => balances,
                            Kind::Credit => -balances,
                        },
                    }
                })
                .collect();
            if lines.is_empty() {
                continue;
            }
            let total = lines.iter().map(|l| l.balances).sum();
            bands.push(Band {
                group,
                lines,
                total,
            });
        }

        let section = |kind: Kind| {
            let bands: Vec<Band> = bands
                .iter()
                .filter(|b| b.group.kind() == kind)
                .cloned()
                .collect();
            Section {
                kind,
                total: bands.iter().map(|b| b.total).sum(),
                bands,
            }
        };
        let cash = section(Kind::Cash);
        let credit = section(Kind::Credit);

        Ok(Overview {
            dates,
            net: cash.total + credit.total,
            cash,
            credit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::txn::NewTxn;
    use crate::db::{self, account};
    use crate::test_support::day;

    fn dates() -> Dates {
        Dates::new(day(2026, 8, 12), day(2026, 8, 27))
    }

    fn add(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64) {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: Cents(cents),
                account_id,
                description: "row".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    /// Inserts an account exactly as the import does: named by its code, in
    /// its kind's default band, appended to whatever that kind holds.
    fn imported(db: &Db, code: &str, kind: Kind) -> AccountId {
        let sort = account::list_by_kind(db, kind).unwrap().len() as i64;
        account::insert(db, code, code, kind, sort).unwrap()
    }

    /// The same, then named and banded the way the owner would on the
    /// Accounts screen. Order comes from the insert order, so a fixture
    /// stacks in the order it is written.
    fn placed(db: &Db, code: &str, name: &str, kind: Kind, group: Group) -> AccountId {
        let id = imported(db, code, kind);
        account::set_name(db, id, name).unwrap();
        account::set_group(db, id, group).unwrap();
        id
    }

    /// A full set of accounts, named, banded and ordered as the owner would.
    fn full_db() -> Db {
        let db = db::open_in_memory().unwrap();
        placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        placed(&db, "SAV", "Rainy Day", Kind::Cash, Group::Savings);
        placed(&db, "BKR", "Brokerage", Kind::Cash, Group::Savings);
        for (code, name) in [
            ("CC1", "Card One"),
            ("CC2", "Card Two"),
            ("CC3", "Card Three"),
            ("CHK", "Everyday Card"),
        ] {
            placed(&db, code, name, Kind::Credit, Group::Credit);
        }
        db
    }

    fn line<'a>(section: &'a Section, name: &str) -> &'a Line {
        section
            .bands
            .iter()
            .flat_map(|b| &b.lines)
            .find(|l| l.account.as_ref().is_some_and(|a| a.text() == name))
            .unwrap_or_else(|| panic!("no line for account {name:?}"))
    }

    fn band(section: &Section, group: Group) -> &Band {
        section
            .bands
            .iter()
            .find(|b| b.group == group)
            .unwrap_or_else(|| panic!("no {group:?} band"))
    }

    /// Cash splits into the two bands, and the section total is their sum
    /// rather than a third independent figure.
    #[test]
    fn the_cash_section_splits_into_checking_and_savings() {
        let db = full_db();
        let everyday = account::by_code(&db, "CHK", Kind::Cash).unwrap().unwrap();
        let rainy = account::by_code(&db, "SAV", Kind::Cash).unwrap().unwrap();
        let broker = account::by_code(&db, "BKR", Kind::Cash).unwrap().unwrap();
        add(&db, everyday.id, day(2026, 1, 1), 100_000);
        add(&db, rainy.id, day(2026, 1, 1), 30_000);
        add(&db, broker.id, day(2026, 1, 1), 5_000);

        let overview = Overview::load(&db, dates()).unwrap();

        assert_eq!(
            overview
                .cash
                .bands
                .iter()
                .map(|b| b.group)
                .collect::<Vec<_>>(),
            vec![Group::Checking, Group::Savings]
        );
        assert_eq!(
            band(&overview.cash, Group::Checking).total.to_date,
            Cents(100_000)
        );
        assert_eq!(
            band(&overview.cash, Group::Savings).total.to_date,
            Cents(35_000)
        );
        assert_eq!(overview.cash.total.to_date, Cents(135_000));
    }

    #[test]
    fn an_account_with_no_transactions_still_gets_a_row_at_zero() {
        let db = db::open_in_memory().unwrap();
        let everyday = placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        placed(&db, "SAV", "Rainy Day", Kind::Cash, Group::Savings);
        add(&db, everyday, day(2026, 1, 1), 100_000);

        let overview = Overview::load(&db, dates()).unwrap();

        assert_eq!(
            overview
                .cash
                .bands
                .iter()
                .map(|b| b.lines.len())
                .sum::<usize>(),
            2,
            "a missing row reads as a missing account"
        );
        assert_eq!(
            line(&overview.cash, "Rainy Day").balances.to_date,
            Cents::ZERO
        );
    }

    /// A band nobody has an account in is dropped rather than drawn at zero:
    /// it is a band the owner does not have, not one holding nothing.
    #[test]
    fn a_band_with_no_accounts_is_not_drawn() {
        let db = db::open_in_memory().unwrap();
        placed(&db, "SAV", "Rainy Day", Kind::Cash, Group::Savings);

        let overview = Overview::load(&db, dates()).unwrap();

        assert_eq!(
            overview
                .cash
                .bands
                .iter()
                .map(|b| b.group)
                .collect::<Vec<_>>(),
            vec![Group::Savings]
        );
    }

    /// Credit is stored as debt. This screen is the one place that negates
    /// it, so Net is a single addition.
    #[test]
    fn credit_rows_and_the_credit_total_are_negated() {
        let db = db::open_in_memory().unwrap();
        let everyday = placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        let card = placed(&db, "CC1", "Card One", Kind::Credit, Group::Credit);
        add(&db, everyday, day(2026, 1, 1), 100_000);
        add(&db, card, day(2026, 1, 1), 7_000);

        let overview = Overview::load(&db, dates()).unwrap();

        assert_eq!(
            line(&overview.credit, "Card One").balances.to_date,
            Cents(-7_000)
        );
        assert_eq!(overview.credit.total.to_date, Cents(-7_000));
        assert_eq!(overview.cash.total.to_date, Cents(100_000));
        assert_eq!(overview.net.to_date, Cents(93_000));
    }

    #[test]
    fn each_column_is_quoted_at_its_own_date() {
        let db = db::open_in_memory().unwrap();
        let everyday = placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        add(&db, everyday, day(2026, 8, 12), 100_000);
        add(&db, everyday, day(2026, 8, 20), 5_000);
        add(&db, everyday, day(2026, 8, 31), 1_000);

        let overview = Overview::load(&db, dates()).unwrap();

        assert_eq!(overview.net.to_date, Cents(100_000));
        assert_eq!(overview.net.adhoc, Cents(105_000));
        assert_eq!(overview.net.month_end, Cents(106_000));
    }
}
