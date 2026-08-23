//! The Funds screen: the asset-allocation rows, and the birth date the age
//! rule needs before it can state a target.
//!
//! Nothing here stores a percentage. The table holds the rule and
//! `calc::fund` turns it into a target on every read, so what these keys
//! write is only ever a rule or the date one is measured from.

use super::{App, NOTHING_SELECTED, Screen};
use crate::db::fund;
use crate::db::setting::{self, key};
use crate::fund as fund_engine;
use crate::tui::cursor;
use crate::tui::form::{self, ValueForm};
use crate::tui::fund::FundForm;
use crate::tui::modal::{Confirm, Modal};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// Entering the screen is what asks for a birth date, so the question
    /// comes back the next time it is entered and never once the setting
    /// exists.
    pub(super) fn open_funds(&mut self) {
        self.screen = Screen::Funds;
        if self.funds.needs_birth_date() {
            self.modal = Some(Modal::BirthDate(ValueForm::date("Birth Date", "")));
        }
    }

    pub(super) fn funds_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.funds, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.modal = Some(Modal::Fund(FundForm::add())),
            KeyCode::Char('e') => self.open_fund_value_edit(),
            KeyCode::Char('E') => self.open_fund_edit()?,
            KeyCode::Char('d') => self.open_fund_delete(),
            _ => {}
        }
        Ok(())
    }

    fn open_fund_value_edit(&mut self) {
        let Some(row) = self.funds.selected() else {
            self.status = NOTHING_SELECTED.to_string();
            return;
        };
        // The stored cents, not the whole dollars the row prints: opening a
        // value and pressing Enter must not quietly round it.
        self.modal = Some(Modal::FundValue(
            row.fund_id,
            ValueForm::money("Actual Value", &row.actual.to_string()),
        ));
    }

    fn open_fund_edit(&mut self) -> Result<()> {
        let Some(row) = self.funds.selected() else {
            return self.nothing_selected();
        };
        let found = fund::get(&self.db, row.fund_id)?;
        self.modal = Some(Modal::Fund(FundForm::edit(&found)));
        Ok(())
    }

    fn open_fund_delete(&mut self) {
        let Some(row) = self.funds.selected() else {
            self.status = NOTHING_SELECTED.to_string();
            return;
        };
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteFund(row.fund_id),
            label: format!("{} — {}", row.name, crate::demo::whole_figure(row.actual)),
        });
    }

    pub(super) fn commit_fund(&mut self) -> Result<()> {
        let Some(Modal::Fund(form)) = &self.modal else {
            return Ok(());
        };
        let edit = form.commit()?;
        match form.editing {
            Some(id) => fund::update(&self.db, id, &edit)?,
            None => {
                let ord = fund::next_ord(&self.db)?;
                fund::insert(
                    &self.db,
                    &fund::NewFund {
                        name: edit.name.clone(),
                        ord,
                        target: edit.target,
                        actual: edit.actual,
                    },
                )?;
            }
        }
        self.status = format!("{} saved", edit.name);
        self.close_modal();
        self.reload()
    }

    pub(super) fn commit_fund_value(&mut self) -> Result<()> {
        let Some(Modal::FundValue(id, form)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected edit keeps the form
        // and everything typed into it.
        let actual = form::parse_whole_amount(form.value())?;
        fund::set_actual(&self.db, *id, actual)?;
        self.status = "value saved".to_string();
        self.close_modal();
        self.reload()
    }

    pub(super) fn commit_birth_date(&mut self) -> Result<()> {
        let Some(Modal::BirthDate(form)) = &self.modal else {
            return Ok(());
        };
        let birth = form::parse_date(form.value())?;
        setting::set(&self.db, key::BIRTH_DATE, birth)?;
        self.status = "birth date saved".to_string();
        self.close_modal();
        self.reload()
    }

    pub(super) fn reload_funds(&mut self) -> Result<()> {
        self.funds
            .set_allocation(fund_engine::compute_from_db(&self.db, self.today)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fund;
    use crate::money::Cents;
    use crate::rate::BasisPoints;
    use crate::tui::app::test_support::*;
    use crate::tui::modal::Modal;
    use chrono::Datelike;
    use ratatui::crossterm::event::KeyCode;

    /// The birth date is a setting with no screen of its own, so the screen
    /// that needs it asks -- every time it is entered, until it is answered.
    #[test]
    fn entering_funds_with_an_age_row_and_no_birth_date_asks_for_one() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Bonds".to_string(),
                ord: 0,
                target: fund::Target::AgeOver30,
                actual: Cents::from_dollars(30_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        assert!(matches!(app.modal, Some(Modal::BirthDate(_))));

        // Esc dismisses it, and the screen still draws.
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert_eq!(app.screen, Screen::Funds);
        assert!(
            footer(&app).contains("birth date unset"),
            "{}",
            footer(&app)
        );

        // And it comes back on the next visit.
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('6'));
        assert!(matches!(app.modal, Some(Modal::BirthDate(_))));
    }

    /// Answering it is the last time it is asked.
    #[test]
    fn saving_a_birth_date_settles_the_question_and_gives_the_age_row_a_target() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Bonds".to_string(),
                ord: 0,
                target: fund::Target::AgeOver30,
                actual: Cents::from_dollars(30_000),
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('6'));

        let birth = app.today.with_year(app.today.year() - 44).unwrap();
        for c in birth.format("%Y-%m-%d").to_string().chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.funds.rows()[0].target, Some(BasisPoints(1_400)));

        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('6'));
        assert!(app.modal.is_none(), "the question is settled");
    }

    /// A fund table with no age row is never asked for a birth date.
    #[test]
    fn entering_funds_with_only_share_rows_asks_nothing() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(10_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));

        assert!(app.modal.is_none());
        assert_eq!(app.screen, Screen::Funds);
    }

    /// `commit_fund`'s insert branch, exercised through the keys rather than
    /// called directly -- this is what would catch `commit_fund` and
    /// `commit_fund_value` swapped on their `Modal` arms, which every test up
    /// to here still passes with.
    #[test]
    fn a_on_the_funds_screen_writes_a_fund() {
        let mut app = app();
        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "International");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "40");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "60,000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1);
        assert_eq!(funds[0].name, "International");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(4_000))
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(60_000));
    }

    /// `e` prefills the stored cents, not the whole dollars the row prints, so
    /// pressing Enter straight away must not quietly round it -- the claim
    /// `open_fund_value_edit`'s own comment makes.
    #[test]
    fn e_on_the_funds_screen_round_trips_the_value_unchanged() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1, "e must not add a second row");
        assert_eq!(funds[0].name, "Domestic");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(6_000))
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(90_000));
    }

    /// `commit_fund`'s update branch, and `open_fund_edit`'s prefill: `E`
    /// rewrites the selected row rather than adding a second, the same
    /// property `committing_capital_e_rewrites_the_bill_rather_than_adding_one`
    /// pins for Planning's bills.
    #[test]
    fn capital_e_on_the_funds_screen_rewrites_the_selected_fund() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('E'));
        for _ in 0.."Domestic".len() {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "Total Market");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let funds = fund::list(&app.db).unwrap();
        assert_eq!(funds.len(), 1, "E updates the row rather than adding one");
        assert_eq!(funds[0].name, "Total Market");
        assert_eq!(
            funds[0].target,
            fund::Target::RemainderShare(BasisPoints(6_000)),
            "fields left untouched by the edit survive it"
        );
        assert_eq!(funds[0].actual, Cents::from_dollars(90_000));
    }

    #[test]
    fn d_then_y_deletes_the_selected_fund() {
        let mut app = app();
        fund::insert(
            &app.db,
            &fund::NewFund {
                name: "Domestic".to_string(),
                ord: 0,
                target: fund::Target::RemainderShare(BasisPoints(6_000)),
                actual: Cents::from_dollars(90_000),
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(fund::list(&app.db).unwrap().is_empty());
        assert!(app.status.contains("fund deleted"), "{}", app.status);
    }
}
