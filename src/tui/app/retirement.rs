//! Screen 0: retirement savings against the age rule. Nothing on it is
//! selectable, so it has no cursor and no scroll keys.

use super::App;
use anyhow::Result;
use ratatui::crossterm::event::KeyEvent;

impl App {
    pub(super) fn retirement_key(&mut self, event: KeyEvent) -> Result<()> {
        let _ = event;
        Ok(())
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
        press(&mut app, KeyCode::Char('0'));
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
    }

    #[test]
    fn with_nothing_configured_the_screen_says_what_to_mark() {
        let mut app = app();
        press(&mut app, KeyCode::Char('0'));
        let screen = drawn(&mut app);
        assert!(
            screen.contains("Mark investment accounts as Retirement on Accounts (9)"),
            "{screen}"
        );
    }

    #[test]
    fn with_no_birth_date_the_screen_asks_for_one() {
        let mut app = retirement_app();
        setting::clear(&app.db, key::BIRTH_DATE).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('0'));
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
        press(&mut app, KeyCode::Char('0'));
        let screen = drawn(&mut app);
        assert!(screen.contains("Press e to set a salary"), "{screen}");
        assert!(!screen.contains("3.30×"), "{screen}");
    }
}
