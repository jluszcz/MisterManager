//! Screen 0: retirement savings against the age rule. Nothing on it is
//! selectable, so it has no cursor and no scroll keys.

use super::App;
use crate::db::setting::{self, key};
use crate::tui::modal::Modal;
use crate::tui::retirement::RetirementForm;
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn retirement_key(&mut self, event: KeyEvent) -> Result<()> {
        if event.code == KeyCode::Char('e') {
            self.modal = Some(Modal::Retirement(RetirementForm::new(
                self.today,
                setting::get(&self.db, key::ANNUAL_SALARY)?,
                setting::get(&self.db, key::BIRTH_DATE)?,
            )));
        }
        Ok(())
    }

    /// Both settings in one transaction, so a refusal of the second cannot
    /// leave the first written.
    pub(super) fn commit_retirement_form(&mut self) -> Result<()> {
        let Some(Modal::Retirement(form)) = &self.modal else {
            return Ok(());
        };
        let (salary, birth) = form.commit()?;
        self.db.transaction(|db| {
            match salary {
                Some(s) => setting::set(db, key::ANNUAL_SALARY, s)?,
                None => setting::clear(db, key::ANNUAL_SALARY)?,
            }
            match birth {
                Some(b) => setting::set(db, key::BIRTH_DATE, b)?,
                None => setting::clear(db, key::BIRTH_DATE)?,
            }
            Ok(())
        })?;
        self.status = "retirement saved".to_string();
        self.close_modal();
        // A full reload rather than this screen's: the birth date is also
        // what the Funds screen's target is set by.
        self.reload()
    }
}

#[cfg(test)]
mod tests {
    use crate::db::account::{self, Kind, TaxTreatment};
    use crate::db::holding;
    use crate::db::setting::{self, key};
    use crate::money::Cents;
    use crate::tui::app::App;
    use crate::tui::app::test_support::*;
    use chrono::Datelike;
    use ratatui::crossterm::event::KeyCode;

    /// Thirty-seven, $330,000 saved against a $100,000 salary: 3.30× against
    /// a band of 3.40–3.60×, so $10,000 short.
    fn retirement_app() -> App {
        let mut app = app();
        let ret = account::insert(
            &app.db,
            "RET",
            "Long Haul",
            Kind::Investment,
            0,
            Some(TaxTreatment::TaxDeferred),
        )
        .unwrap();
        account::set_retirement(&app.db, ret, true).unwrap();
        holding::insert(&app.db, ret, "TDF45", Cents::from_dollars(330_000)).unwrap();
        let birth = today().with_year(today().year() - 37).unwrap();
        setting::set(&app.db, key::BIRTH_DATE, birth).unwrap();
        setting::set(&app.db, key::ANNUAL_SALARY, Cents::from_dollars(100_000)).unwrap();
        app.reload().unwrap();
        app
    }

    #[test]
    fn zero_opens_the_retirement_screen_and_it_draws_where_the_owner_stands() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(screen.contains("age 37"), "{screen}");
        assert!(screen.contains("3.30×"), "{screen}");
        assert!(screen.contains("Short $10,000"), "{screen}");
        assert!(screen.contains("Now (37)"), "{screen}");
        assert!(
            !screen.contains("By 35"),
            "a passed milestone is drawn:\n{screen}"
        );
        assert!(screen.contains("Long Haul"), "{screen}");
        for title in ["Where you stand", "Milestones", "Accounts"] {
            assert!(
                screen.lines().any(|l| l.contains('┌') && l.contains(title)),
                "no {title} box:\n{screen}"
            );
        }
    }

    #[test]
    fn with_nothing_configured_the_screen_says_what_to_mark() {
        let mut app = app();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(
            screen.contains("Mark investment accounts as Retirement on Accounts (0)"),
            "{screen}"
        );
    }

    #[test]
    fn with_no_birth_date_the_screen_asks_for_one() {
        let mut app = retirement_app();
        setting::clear(&app.db, key::BIRTH_DATE).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(screen.contains("Press e to set a birth date"), "{screen}");
        assert!(
            screen.contains("By 35"),
            "with no age every milestone is drawn:\n{screen}"
        );
    }

    #[test]
    fn with_no_salary_the_multiples_read_as_absent_and_the_prompt_asks_for_one() {
        let mut app = retirement_app();
        setting::clear(&app.db, key::ANNUAL_SALARY).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(screen.contains("Press e to set a salary"), "{screen}");
        assert!(!screen.contains("3.30×"), "{screen}");
    }

    #[test]
    fn e_saves_a_salary_and_the_multiple_moves_with_it() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "110000");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert_eq!(
            setting::get(&app.db, key::ANNUAL_SALARY).unwrap(),
            Some(Cents::from_dollars(110_000))
        );
        // 330,000 / 110,000 = 3.00×.
        assert!(drawn(&mut app).contains("3.00×"));
    }

    #[test]
    fn an_emptied_salary_clears_the_setting() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert_eq!(setting::get(&app.db, key::ANNUAL_SALARY).unwrap(), None);
    }

    #[test]
    fn a_salary_of_nothing_is_refused_and_the_form_stays_open() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "0");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_some(), "a zero salary was accepted");
        assert!(!app.status.is_empty(), "the refusal said nothing");
        assert_eq!(
            setting::get(&app.db, key::ANNUAL_SALARY).unwrap(),
            Some(Cents::from_dollars(100_000))
        );
    }

    #[test]
    fn e_saves_a_birth_date_and_the_age_moves_with_it() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'u');
        let birth = today().with_year(today().year() - 41).unwrap();
        type_str(&mut app, &birth.format("%Y-%m-%d").to_string());
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert_eq!(setting::get(&app.db, key::BIRTH_DATE).unwrap(), Some(birth));
        assert!(drawn(&mut app).contains("age 41"));
    }

    /// 37's tax-free band starts at 11.0%, and nothing here is held
    /// tax-free: 11% of $330,000.
    #[test]
    fn a_short_tax_free_share_names_the_dollars_to_move_across() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(screen.contains("Short $36,300"), "{screen}");
        assert!(screen.contains("$400K – $450K"), "{screen}");
        // Tenths in the standing and the milestones, hundredths in Accounts.
        assert!(screen.contains("0.0%"), "{screen}");
        assert!(screen.contains("11.0 – 12.0%"), "{screen}");
        assert!(screen.contains("12.5 – 15.0%"), "{screen}");
        assert!(screen.contains("100.00%"), "{screen}");

        // Each milestone's shortfall against today's $330,000 -- 45 asks
        // $500,000, and 15% of that, $75,000, tax-free.
        let by_45 = screen.lines().find(|l| l.contains("By 45")).unwrap();
        assert!(by_45.contains("$170,000"), "{by_45}");
        assert!(by_45.contains("$75,000"), "{by_45}");
    }
}
