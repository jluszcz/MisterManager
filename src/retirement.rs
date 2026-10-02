//! Retirement savings against the age rule: what the retirement-marked
//! accounts hold, as a multiple of salary and as a tax-free share, and the
//! milestone table. In neither medium -- the Retirement screen and the
//! report's tab both draw this, the way both draw `plan_rows`.

use crate::account_label::Account;
use crate::calc::{
    self,
    retirement::{Band, Multiple, Status},
};
use crate::db::account::{self, TaxTreatment};
use crate::db::setting::{self, key};
use crate::db::{Db, holding};
use crate::money::Cents;
use crate::rate::{BasisPoints, Percent};
use anyhow::Result;
use chrono::NaiveDate;

#[derive(Clone, Debug)]
pub struct Held {
    pub account: Account,
    pub treatment: TaxTreatment,
    /// The part of `balance` held tax-free, through
    /// [`account::Account::tax_free`] -- all of it under the `tax_free`
    /// treatment.
    pub tax_free: Percent,
    pub balance: Cents,
}

impl Held {
    /// What of `balance` counts as tax-free, truncated to the cent.
    pub fn tax_free_balance(&self) -> Cents {
        self.tax_free.of(self.balance)
    }

    /// What a Tax column prints for this account.
    pub fn tax_label(&self) -> String {
        account::tax_label(
            self.treatment,
            (self.tax_free != Percent::ZERO).then_some(self.tax_free),
        )
    }

    /// Floored to the basis point, so a column of these may sum a hundredth
    /// short of 100.00 -- drawn beside the balances it came from, which do
    /// foot.
    pub fn share_of(&self, saved: Cents) -> Option<BasisPoints> {
        share(self.balance, saved)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub age: i64,
    /// The owner's own age -- the row the standing is measured against.
    pub now: bool,
    pub saved: Band<Multiple>,
    /// `saved` in dollars, which needs a salary.
    pub saved_dollars: Option<Band<Cents>>,
    pub tax_free: Band<BasisPoints>,
}

#[derive(Clone, Debug, Default)]
pub struct Retirement {
    /// Whole years, through `calc::fund::whole_years` -- the age the Funds
    /// target uses, so the two screens agree on it.
    pub age: Option<i64>,
    pub salary: Option<Cents>,
    pub saved: Cents,
    pub tax_free: Cents,
    pub held: Vec<Held>,
    pub rows: Vec<Row>,
}

impl Retirement {
    pub fn multiple(&self) -> Option<Multiple> {
        Multiple::between(self.saved, self.salary?)
    }

    pub fn tax_free_share(&self) -> Option<BasisPoints> {
        share(self.tax_free, self.saved)
    }

    pub fn now(&self) -> Option<&Row> {
        self.rows.iter().find(|r| r.now)
    }

    /// Compared in cents rather than as a floored multiple, so a balance a
    /// fraction of a hundredth under the band is not called on track.
    pub fn saved_status(&self) -> Option<(Status, Option<Cents>)> {
        let band = self.now()?.saved_dollars?;
        let status = calc::retirement::status(self.saved, band);
        let short_by = (status == Status::Short).then(|| band.low - self.saved);
        Some((status, short_by))
    }

    /// Short by the tax-free dollars today's total would hold at the band's
    /// low end -- what would have to move across, not what new money would
    /// have to be added, which grows the total it is a share of. Rounded up
    /// to the cent, so moving it is enough; a share exists only over a
    /// positive total, so the numerator is never negative.
    pub fn tax_free_status(&self) -> Option<(Status, Option<Cents>)> {
        let low = self.now()?.tax_free.low;
        let status = calc::retirement::status(self.tax_free_share()?, self.now()?.tax_free);
        let short_by = (status == Status::Short)
            .then(|| Cents((self.saved.0 * low.0 + 9_999) / 10_000) - self.tax_free);
        Some((status, short_by))
    }
}

/// `None` over nothing, never a divide by zero.
fn share(part: Cents, whole: Cents) -> Option<BasisPoints> {
    (whole.0 > 0).then(|| BasisPoints(part.0 * 10_000 / whole.0))
}

fn row(age: i64, now: bool, salary: Option<Cents>) -> Row {
    let saved = calc::retirement::saved_band(age);
    Row {
        age,
        now,
        saved,
        saved_dollars: salary.map(|s| saved.map(|m| m.of(s))),
        tax_free: calc::retirement::tax_free_band(age),
    }
}

/// Now, then the milestones still ahead. A passed one is left out: with no
/// balance history there is no saying whether it was met *at* that age, and
/// a past target says nothing about today. With no age there is no Now and
/// nothing has passed, so every milestone is drawn.
pub fn rows(age: Option<i64>, salary: Option<Cents>) -> Vec<Row> {
    let mut rows: Vec<Row> = age.map(|a| row(a, true, salary)).into_iter().collect();
    rows.extend(
        calc::retirement::MILESTONES
            .into_iter()
            .filter(|m| age.is_none_or(|a| *m > a))
            .map(|m| row(m, false, salary)),
    );
    rows
}

pub fn load(db: &Db, today: NaiveDate) -> Result<Retirement> {
    let age = setting::get(db, key::BIRTH_DATE)?.map(|birth| calc::fund::whole_years(birth, today));
    let salary = setting::get(db, key::ANNUAL_SALARY)?;
    let accounts = account::list(db)?;
    let mut held = Vec::new();
    for a in accounts.iter().filter(|a| a.retirement) {
        held.push(Held {
            account: Account::named(&accounts, a.id),
            treatment: a.tax_treatment.expect(
                "schema CHECK pairs retirement with an investment kind, which has a treatment",
            ),
            tax_free: a.tax_free(),
            balance: holding::list_for_account(db, a.id)?
                .iter()
                .map(|h| h.balance)
                .sum(),
        });
    }
    let saved = held.iter().map(|h| h.balance).sum();
    let tax_free = held.iter().map(Held::tax_free_balance).sum();
    Ok(Retirement {
        age,
        salary,
        saved,
        tax_free,
        held,
        rows: rows(age, salary),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, AccountId, account, holding, setting};
    use crate::test_support::day;
    use chrono::Datelike;

    fn today() -> NaiveDate {
        day(2026, 8, 15)
    }

    /// Thirty-seven today, derived from the fixture's day rather than
    /// written out -- a literal year is a plausible real birth date in a
    /// tracked file.
    fn born_37_years_ago() -> NaiveDate {
        today().with_year(today().year() - 37).unwrap()
    }

    fn retirement_account(db: &Db, code: &str, name: &str, treatment: TaxTreatment) -> AccountId {
        let id = account::insert(
            db,
            code,
            name,
            account::Kind::Investment,
            0,
            Some(treatment),
        )
        .unwrap();
        account::set_retirement(db, id, true).unwrap();
        id
    }

    #[test]
    fn only_holdings_under_a_retirement_account_count_toward_saved() {
        let db = db::open_in_memory().unwrap();
        let long_haul = retirement_account(&db, "RET", "Long Haul", TaxTreatment::TaxDeferred);
        let pot = retirement_account(&db, "ROTH", "Untaxed Pot", TaxTreatment::TaxFree);
        let brokerage = account::insert(
            &db,
            "BRK",
            "Holdings",
            account::Kind::Investment,
            0,
            Some(TaxTreatment::Taxable),
        )
        .unwrap();
        holding::insert(&db, long_haul, "TDF45", Cents::from_dollars(300_000)).unwrap();
        holding::insert(&db, pot, "USM", Cents::from_dollars(30_000)).unwrap();
        holding::insert(&db, pot, "ISM", Cents::from_dollars(10_000)).unwrap();
        holding::insert(&db, brokerage, "USM", Cents::from_dollars(50_000)).unwrap();

        let r = load(&db, today()).unwrap();
        assert_eq!(r.saved, Cents::from_dollars(340_000));
        assert_eq!(r.tax_free, Cents::from_dollars(40_000));
        assert_eq!(r.held.len(), 2, "the unmarked brokerage is not listed");
        // 40,000 / 340,000 = 11.76...%, floored to the basis point.
        assert_eq!(r.tax_free_share(), Some(BasisPoints(1_176)));
    }

    #[test]
    fn a_tax_free_part_of_a_mixed_account_counts_toward_tax_free() {
        let db = db::open_in_memory().unwrap();
        let long_haul = retirement_account(&db, "RET", "Long Haul", TaxTreatment::TaxDeferred);
        account::set_tax_free_percent(&db, long_haul, Some(Percent(20))).unwrap();
        holding::insert(&db, long_haul, "TDF45", Cents::from_dollars(300_000)).unwrap();
        let pot = retirement_account(&db, "ROTH", "Untaxed Pot", TaxTreatment::TaxFree);
        holding::insert(&db, pot, "USM", Cents::from_dollars(40_000)).unwrap();

        let r = load(&db, today()).unwrap();
        // 20% of 300,000, and all of 40,000.
        assert_eq!(r.tax_free, Cents::from_dollars(100_000));
        assert_eq!(r.held[0].tax_label(), "20% tax-free");
        assert_eq!(r.held[1].tax_label(), "Tax-free");
    }

    /// No `fund_mix` row is written anywhere in this file: a balance needs
    /// no composition, which is what keeps an unfetched fund in the total.
    #[test]
    fn a_fund_never_fetched_still_counts() {
        let db = db::open_in_memory().unwrap();
        let pot = retirement_account(&db, "ROTH", "Untaxed Pot", TaxTreatment::TaxFree);
        holding::insert(&db, pot, "UNC", Cents::from_dollars(1_000)).unwrap();
        assert_eq!(
            load(&db, today()).unwrap().saved,
            Cents::from_dollars(1_000)
        );
    }

    #[test]
    fn an_account_holding_nothing_is_listed_and_saved_nothing_has_no_tax_free_share() {
        let db = db::open_in_memory().unwrap();
        retirement_account(&db, "ROTH", "Untaxed Pot", TaxTreatment::TaxFree);
        let r = load(&db, today()).unwrap();
        assert_eq!(r.held.len(), 1);
        assert_eq!(r.saved, Cents::ZERO);
        assert_eq!(r.tax_free_share(), None);
        assert_eq!(r.tax_free_status(), None);
    }

    #[test]
    fn short_of_the_tax_free_band_states_the_dollars_to_move_across_at_todays_total() {
        let db = db::open_in_memory().unwrap();
        let ret = retirement_account(&db, "RET", "Long Haul", TaxTreatment::TaxDeferred);
        let pot = retirement_account(&db, "ROTH", "Untaxed Pot", TaxTreatment::TaxFree);
        holding::insert(&db, ret, "TDF45", Cents::from_dollars(370_000)).unwrap();
        holding::insert(&db, pot, "USM", Cents::from_dollars(30_000)).unwrap();
        setting::set(&db, key::BIRTH_DATE, born_37_years_ago()).unwrap();
        let r = load(&db, today()).unwrap();
        // 37's band starts at 11.00%; 11% of 400,000 is 44,000, and 30,000
        // of it is held.
        assert_eq!(r.now().unwrap().tax_free.low, BasisPoints(1_100));
        assert_eq!(
            r.tax_free_status(),
            Some((Status::Short, Some(Cents::from_dollars(14_000))))
        );
    }

    #[test]
    fn the_table_starts_at_now_and_drops_every_milestone_already_passed() {
        let ages: Vec<(i64, bool)> = rows(Some(37), None)
            .iter()
            .map(|r| (r.age, r.now))
            .collect();
        assert_eq!(
            ages,
            vec![
                (37, true),
                (40, false),
                (45, false),
                (50, false),
                (55, false)
            ]
        );
    }

    #[test]
    fn an_age_on_a_milestone_is_one_row_marked_now() {
        let ages: Vec<(i64, bool)> = rows(Some(40), None)
            .iter()
            .map(|r| (r.age, r.now))
            .collect();
        assert_eq!(
            ages,
            vec![(40, true), (45, false), (50, false), (55, false)]
        );
    }

    #[test]
    fn past_the_last_milestone_the_table_is_now_alone_and_with_no_age_it_is_every_milestone() {
        assert_eq!(rows(Some(60), None).len(), 1);
        let ages: Vec<i64> = rows(None, None).iter().map(|r| r.age).collect();
        assert_eq!(ages, calc::retirement::MILESTONES.to_vec());
        assert!(rows(None, None).iter().all(|r| !r.now));
    }

    #[test]
    fn a_dollar_target_is_stated_only_beside_a_salary() {
        assert!(
            rows(Some(37), None)
                .iter()
                .all(|r| r.saved_dollars.is_none())
        );
        let now = &rows(Some(37), Some(Cents::from_dollars(100_000)))[0];
        assert_eq!(
            now.saved_dollars,
            Some(Band {
                low: Cents::from_dollars(340_000),
                high: Cents::from_dollars(360_000)
            })
        );
    }

    #[test]
    fn short_of_the_band_states_the_dollars_to_its_low_end() {
        let db = db::open_in_memory().unwrap();
        let ret = retirement_account(&db, "RET", "Long Haul", TaxTreatment::TaxDeferred);
        holding::insert(&db, ret, "TDF45", Cents::from_dollars(330_000)).unwrap();
        setting::set(&db, key::BIRTH_DATE, born_37_years_ago()).unwrap();
        setting::set(&db, key::ANNUAL_SALARY, Cents::from_dollars(100_000)).unwrap();
        let r = load(&db, today()).unwrap();
        assert_eq!(r.age, Some(37));
        assert_eq!(r.multiple(), Some(Multiple(330)));
        assert_eq!(
            r.saved_status(),
            Some((Status::Short, Some(Cents::from_dollars(10_000))))
        );
    }

    #[test]
    fn with_no_salary_or_no_age_there_is_no_saved_status() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::BIRTH_DATE, born_37_years_ago()).unwrap();
        assert_eq!(
            load(&db, today()).unwrap().saved_status(),
            None,
            "no salary"
        );
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::ANNUAL_SALARY, Cents::from_dollars(100_000)).unwrap();
        assert_eq!(load(&db, today()).unwrap().saved_status(), None, "no age");
    }
}
