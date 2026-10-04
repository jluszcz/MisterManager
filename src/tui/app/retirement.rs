//! Screen 8: retirement savings against the age rule. Nothing on it is
//! selectable, so it has no cursor and no scroll keys; what the keys move
//! besides the two settings is the charts' window.

use super::App;
use crate::db::setting::{self, key};
use crate::tui::modal::Modal;
use crate::tui::retirement::{RetirementForm, Window, WindowForm};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn retirement_key(&mut self, event: KeyEvent) -> Result<()> {
        let extent = crate::balance_history::extent(&self.history);
        let window = self.chart_window;
        match (event.code, extent) {
            (KeyCode::Char('e'), _) => {
                self.modal = Some(Modal::Retirement(RetirementForm::new(
                    self.today,
                    setting::get(&self.db, key::ANNUAL_SALARY)?,
                    setting::get(&self.db, key::BIRTH_DATE)?,
                )));
            }
            (KeyCode::Char('w'), _) => {
                let shown = extent.map(|extent| window.bounds(extent));
                self.modal = Some(Modal::ChartWindow(WindowForm::new(self.today, shown)));
            }
            (KeyCode::Char('['), Some(extent)) => self.chart_window = window.step_start(-1, extent),
            (KeyCode::Char(']'), Some(extent)) => self.chart_window = window.step_start(1, extent),
            (KeyCode::Char('{'), Some(extent)) => self.chart_window = window.step_end(-1, extent),
            (KeyCode::Char('}'), Some(extent)) => self.chart_window = window.step_end(1, extent),
            (KeyCode::Esc, _) => self.chart_window = Window::default(),
            _ => {}
        }
        Ok(())
    }

    /// With nothing recorded there is no history to hold the window inside,
    /// so it opens on the whole of it -- the only window there is.
    pub(super) fn commit_chart_window(&mut self) -> Result<()> {
        let Some(Modal::ChartWindow(form)) = &self.modal else {
            return Ok(());
        };
        let (start, end) = form.commit()?;
        self.chart_window = match crate::balance_history::extent(&self.history) {
            Some(extent) => Window::within(start, end, extent),
            None => Window::default(),
        };
        self.close_modal();
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
    use crate::test_support::day;
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
        assert!(screen.contains("Age 37"), "{screen}");
        assert!(screen.contains("3.30×"), "{screen}");
        assert!(screen.contains("Short $10,000"), "{screen}");
        assert!(screen.contains("Now (37)"), "{screen}");
        assert!(
            !screen.contains("By 35"),
            "a passed milestone is drawn:\n{screen}"
        );
        for title in ["Where you stand", "Milestones", "Cash balances over time"] {
            assert!(
                screen.lines().any(|l| l.contains('┌') && l.contains(title)),
                "no {title} box:\n{screen}"
            );
        }
    }

    /// The terminal height the charts are held to: the boxes above them take
    /// a fixed share and the charts get what is left, so at the 24 rows
    /// `drawn` uses there is none.
    const CHART_HEIGHT: u16 = 40;

    /// The fixture's ledger runs into last month, so the cash chart spans
    /// two months; the fund was recorded only today, so its chart spans one.
    #[test]
    fn the_charts_draw_cash_and_investment_history_and_leave_credit_out() {
        let mut app = retirement_app();
        crate::balance_history::take(&app.db, today()).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        for title in ["Cash balances over time", "Investment balances over time"] {
            assert!(
                screen.lines().any(|l| l.contains('┌') && l.contains(title)),
                "no {title} chart:\n{screen}"
            );
        }
        for name in ["Everyday", "Rainy Day", "Long Haul", "Total"] {
            assert!(screen.contains(name), "{name} has no line:\n{screen}");
        }
        assert!(screen.contains("Aug 2026"), "{screen}");
        for card in ["Card One", "Card Two"] {
            assert!(!screen.contains(card), "{card} was charted:\n{screen}");
        }
    }

    /// The fixture's ledger plus Everyday rows in March and May, so the
    /// history runs March through August.
    fn charted_app() -> App {
        let mut app = retirement_app();
        let everyday = account::by_code(&app.db, "CHK", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        write(&app.db, everyday, day(2026, 3, 10), 50_000, "Paycheck");
        write(&app.db, everyday, day(2026, 5, 10), 50_000, "Paycheck");
        crate::balance_history::take(&app.db, today()).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('8'));
        app
    }

    #[test]
    fn brackets_move_the_charts_start_and_esc_brings_back_the_whole_history() {
        let mut app = charted_app();
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("Mar 2026"), "{screen}");
        assert!(!screen.contains("Charts"), "{screen}");
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("Charts May 2026 – Aug 2026"), "{screen}");
        assert!(!screen.contains("Mar 2026"), "{screen}");
        press(&mut app, KeyCode::Esc);
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(!screen.contains("Charts"), "{screen}");
        assert!(screen.contains("Mar 2026"), "{screen}");
    }

    /// Cash runs March through August and the fund was recorded only in
    /// August, yet both charts span March to August and top out at the
    /// fund's $330K.
    #[test]
    fn the_two_charts_share_their_months_and_their_dollars() {
        let mut app = charted_app();
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        let lines: Vec<&str> = screen.lines().collect();
        let below = |title: &str| {
            let at = lines
                .iter()
                .position(|l| l.contains(title))
                .unwrap_or_else(|| panic!("no {title}:\n{screen}"));
            lines[at + 1]
        };
        let top = below("Cash balances over time");
        assert_eq!(top.matches("$330K").count(), 2, "{screen}");
        let x_labels = lines
            .iter()
            .find(|l| l.contains("Aug 2026"))
            .unwrap_or_else(|| panic!("no x axis:\n{screen}"));
        assert_eq!(x_labels.matches("Mar 2026").count(), 2, "{screen}");
    }

    #[test]
    fn braces_move_the_charts_end() {
        let mut app = charted_app();
        press(&mut app, KeyCode::Char('{'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("Charts Mar 2026 – Jul 2026"), "{screen}");
        press(&mut app, KeyCode::Char('}'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(!screen.contains("Charts"), "{screen}");
    }

    /// The investment fund was recorded only in August, so a window ending
    /// before it has history for that chart and none of it inside.
    #[test]
    fn a_chart_with_nothing_inside_the_window_says_so() {
        let mut app = charted_app();
        press(&mut app, KeyCode::Char('{'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(
            screen.contains("Nothing recorded in this window"),
            "{screen}"
        );
    }

    #[test]
    fn w_types_the_charts_window() {
        let mut app = charted_app();
        press(&mut app, KeyCode::Char('w'));
        ctrl_press(&mut app, 'u');
        for c in "2026-04-15".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'u');
        for c in "2026-06-15".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open");
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("Charts Apr 2026 – Jun 2026"), "{screen}");
    }

    /// Saved untouched, the end is still open: it was prefilled with the
    /// month the history stops at, which is where an open edge is stored.
    #[test]
    fn w_opens_on_the_window_the_charts_are_drawing() {
        let mut app = charted_app();
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('w'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("2026-04-01"), "{screen}");
        assert!(screen.contains("2026-08-31"), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open");
        assert_eq!(
            app.chart_window.start(),
            Some(crate::calc::Month::of(day(2026, 4, 1)))
        );
        assert_eq!(app.chart_window.end(), None);
    }

    #[test]
    fn an_end_before_the_start_is_refused_and_the_form_stays_open() {
        let mut app = charted_app();
        press(&mut app, KeyCode::Char('w'));
        ctrl_press(&mut app, 'u');
        for c in "2026-06-01".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'u');
        for c in "2026-04-01".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_some(), "the form closed");
        assert!(
            app.status.contains("the end is before the start"),
            "{}",
            app.status
        );
    }

    #[test]
    fn with_no_history_a_chart_says_so() {
        let mut app = app();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn_at(&mut app, CHART_HEIGHT);
        assert!(screen.contains("Nothing recorded yet"), "{screen}");
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
        assert!(drawn(&mut app).contains("Age 41"));
    }

    /// 37's target is $340,000 and its tax-free band starts at 11.0%, and
    /// nothing here is held tax-free: 11% of $340,000, in the box and on the
    /// Now row alike.
    #[test]
    fn a_short_tax_free_standing_names_the_dollars_of_the_target() {
        let mut app = retirement_app();
        press(&mut app, KeyCode::Char('8'));
        let screen = drawn(&mut app);
        assert!(screen.contains("Short $37,400"), "{screen}");
        let now = screen.lines().find(|l| l.contains("Now (37)")).unwrap();
        assert!(now.contains("$37,400"), "the Now row disagrees: {now}");
        assert!(screen.contains("$400K – $450K"), "{screen}");
        // Tenths in the standing and the milestones.
        assert!(screen.contains("0.0%"), "{screen}");
        assert!(screen.contains("11.0 – 12.0%"), "{screen}");
        assert!(screen.contains("12.5 – 15.0%"), "{screen}");

        // Each milestone's shortfall against today's $330,000 -- 45 asks
        // $500,000, and 15% of that, $75,000, tax-free.
        let by_45 = screen.lines().find(|l| l.contains("By 45")).unwrap();
        assert!(by_45.contains("$170,000"), "{by_45}");
        assert!(by_45.contains("$75,000"), "{by_45}");
    }
}
