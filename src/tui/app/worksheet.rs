//! The worksheet modal: a container's pot, divided among its goals.
//!
//! One worksheet is one `batch`, so a fumbled payday is one `U` rather than
//! dozens of deletions -- and a payday runs the modal once per container,
//! which is why `open_next_worksheet` is a queue rather than a single open.

use super::{Account, App, week_step};
use crate::db::{GoalId, account, goal};
use crate::goal as goal_engine;
use crate::money::Cents;
use crate::tui::form::{DateField, Step};
use crate::tui::modal::{Confirm, Modal};
use crate::tui::search::{self, Search};
use crate::tui::worksheet::{self as worksheet_screen, Worksheet};
use crate::tui::{cursor, text};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// The worksheet is not a field form: its keys are line editing, not
    /// `Tab`-through-fields with autocomplete, so it does not go through
    /// `form_key`.
    pub(super) fn worksheet_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(Modal::Worksheet(sheet)) = &mut self.modal else {
            return Ok(());
        };
        // `/` waits for the next key: a digit is the fraction operator,
        // anything else begins a name filter.
        if sheet.is_pending_slash() {
            sheet.cancel_pending_slash();
            return match key.code {
                // The worksheet is a context that takes the editing keys, so
                // `App::dispatch` lets a `Ctrl` through to reach its date --
                // and this is one of the two arms here that must not read one
                // as the letter it arrives as. The other is the operators'.
                KeyCode::Char(_) if !text::is_bare(key) => Ok(()),
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    sheet.divide(c.to_digit(10).expect("checked above") as i64)
                }
                KeyCode::Char(c) => {
                    sheet.begin_search();
                    sheet.push_search(c);
                    Ok(())
                }
                _ => Ok(()),
            };
        }
        if sheet.is_searching() {
            search::search_key(sheet, key);
            return Ok(());
        }
        if cursor::scroll_key(sheet, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if search::escape_kept_filter(sheet) {
                    return Ok(());
                }
                self.close_modal();
                self.status = "cancelled".to_string();
                return self.open_next_worksheet();
            }
            KeyCode::Enter => return self.commit_worksheet(),
            KeyCode::Tab => sheet.next_focus(),
            KeyCode::BackTab => sheet.previous_focus(),
            KeyCode::Backspace => sheet.backspace(),
            // Only the date focus has a date to move; `step_date` is where
            // that is decided, so the two digit focuses are unreachable from
            // here.
            KeyCode::Left => sheet.step_date(week_step(key, Step::PREVIOUS_WEEK, Step::PREVIOUS)),
            KeyCode::Right => sheet.step_date(week_step(key, Step::NEXT_WEEK, Step::NEXT)),
            // The operators are line-editing keys, but they are live from the
            // amount too: that field takes digits and drops everything else,
            // so gating them on `Lines` only made them dead keys on the
            // worksheet as it opens. The date is a text field -- `-` is part
            // of a date, and `s` must not spread while one is being fixed --
            // so it is the one focus that types them instead.
            //
            // A modified character never reaches them: `Ctrl` means editing
            // text everywhere in the app and `Alt` means nothing anywhere, and
            // a hand reaching for `Ctrl`+`W` on the amount would otherwise
            // spread the whole pot.
            KeyCode::Char(c)
                if sheet.focus() != worksheet_screen::Focus::Date && text::is_bare(key) =>
            {
                match c {
                    ' ' => sheet.toggle_selection(),
                    '*' => sheet.select_all_visible(),
                    '-' => sheet.clear_selection(),
                    'z' => sheet.zero_untargeted(),
                    's' => sheet.spread()?,
                    'w' => sheet.spread_by_weight()?,
                    '/' => sheet.slash(),
                    _ => sheet.type_char(c),
                }
            }
            // The date takes the character itself and the editing keys alike;
            // for the other two focuses this is where a modified one stops.
            _ => {
                sheet.edit(key);
            }
        }
        Ok(())
    }

    /// Open the next queued worksheet, if any.
    ///
    /// The worksheets open prefilled rather than posting directly, so the
    /// numbers are reviewable and cancellable before they become an undoable
    /// batch.
    pub(super) fn open_next_worksheet(&mut self) -> Result<()> {
        // Taken rather than borrowed: if building the sheet below fails, the
        // `?` must not leave a non-empty queue behind with no worksheet on
        // screen to drain it -- that is what would let an unrelated `Esc` or
        // `Enter` resurrect it later. The tail is put back only once the
        // sheet is actually on screen.
        let mut queue = std::mem::take(&mut self.pending_worksheets);
        if queue.is_empty() {
            return Ok(());
        }
        let (container, date, pot, shares) = queue.remove(0);
        let mut prefill = Vec::new();
        for g in goal::list_with_balances(&self.db, container)? {
            prefill.push((g.goal.id, g.goal.name, Cents::ZERO));
        }
        let account = account::get(&self.db, container)?;
        let mut sheet = Worksheet::on(
            goal::BatchKind::Paycheck,
            Account::named(std::slice::from_ref(&account), container),
            DateField::on(self.today, date),
            prefill,
        );
        sheet.set_amount(pot);
        sheet.set_lines(&shares);
        self.modal = Some(Modal::Worksheet(sheet));
        self.pending_worksheets = queue;
        Ok(())
    }

    /// Opens on the container the screen is filtered to, or the first when the
    /// filter is All. Payday means running it twice -- once per container --
    /// which is what Planning instructs and what physically happens.
    pub(super) fn open_payday(&mut self) -> Result<()> {
        let sheet = self.new_worksheet(goal::BatchKind::Paycheck)?;
        if let Some(sheet) = sheet {
            self.modal = Some(Modal::Worksheet(sheet));
        }
        Ok(())
    }

    /// The worksheet every entry point starts from: this container's open
    /// goals, each prefilled with what `per_paycheck` asks of it.
    fn new_worksheet(&mut self, kind: goal::BatchKind) -> Result<Option<Worksheet>> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(None);
        };
        let mut prefill = Vec::new();
        for g in goal_engine::list_with_balances(&self.db, container)? {
            let ask = crate::savings::paycheck_ask(&g, self.today, self.period_days)?;
            prefill.push((g.goal.id, g.goal.name, ask.unwrap_or(Cents::ZERO)));
        }
        let account = account::get(&self.db, container)?;
        Ok(Some(Worksheet::new(
            kind,
            Account::named(std::slice::from_ref(&account), container),
            self.today,
            prefill,
        )))
    }

    /// `i` opens the worksheet on the container's excess, with the shares its
    /// policy prefers.
    ///
    /// The lines are every open goal, not only the eligible ones: the prefill
    /// decides where the money starts, and the owner may still move it.
    pub(super) fn open_interest(&mut self) -> Result<()> {
        let Some(mut sheet) = self.new_worksheet(goal::BatchKind::Interest)? else {
            return Ok(());
        };
        let container = sheet.container();
        let goals = goal::list_with_balances(&self.db, container)?;
        let eligible: Vec<(GoalId, Cents)> = goals
            .iter()
            .filter(|g| g.goal.interest_eligible)
            .map(|g| (g.goal.id, g.current))
            .collect();
        // Handed over whole: `interest_prefill` keeps the weights inside
        // `eligible`, so a goal closed or made ineligible since the last
        // posting drops out there and `pro_rata` renormalizes over the rest.
        let previous = match goal::last_batch(&self.db, goal::BatchKind::Interest, container)? {
            None => Vec::new(),
            Some(batch) => goal::batch_shares(&self.db, batch.id)?,
        };
        // Clamped at zero: `pro_rata` refuses a negative total, and an
        // over-allocated container is a state to fix by hand, not to split.
        let total = goal::container_excess(&self.db, container)?.max(Cents::ZERO);
        let policy = account::interest_policy(&self.db, container)?;
        let shares = worksheet_screen::interest_prefill(policy, total, &eligible, &previous)?;
        sheet.set_amount(total);
        sheet.set_lines(&shares);
        self.modal = Some(Modal::Worksheet(sheet));
        Ok(())
    }

    fn commit_worksheet(&mut self) -> Result<()> {
        let Some(Modal::Worksheet(sheet)) = &self.modal else {
            return Ok(());
        };
        let committed = sheet.commit()?;
        let kind = sheet.kind();
        let total: Cents = committed.shares.iter().map(|(_, c)| *c).sum();
        goal::insert_allocations(&self.db, kind, committed.date, &committed.shares, None)?;
        self.status = format!(
            "posted {} across {} goals · U undoes it",
            crate::demo::figure(total),
            committed.shares.len()
        );
        self.close_modal();
        // Taken rather than borrowed, the same as `commit_plan_transfers`:
        // if `reload` fails, the `?` must not leave a non-empty queue behind
        // with no worksheet on screen to drain it -- that is what would let
        // some unrelated `Esc` or `Enter` resurrect a stale worksheet from
        // this payday later, prefilled with amounts already posted.
        let queue = std::mem::take(&mut self.pending_worksheets);
        self.reload()?;
        self.pending_worksheets = queue;
        self.open_next_worksheet()
    }

    /// The most recent batch, whatever it was. Never an `Import` batch:
    /// `goal::most_recent_batch` excludes it, because that one holds every
    /// opening balance in the database.
    pub(super) fn open_undo(&mut self) -> Result<()> {
        let Some(batch) = goal::most_recent_batch(&self.db)? else {
            self.status = "nothing to undo".to_string();
            return Ok(());
        };
        let shares = goal::batch_shares(&self.db, batch.id)?;
        let total: Cents = shares.iter().map(|(_, c)| *c).sum();
        let label = format!(
            "{} {} · {} goals · {}",
            batch.kind.as_str(),
            batch.date,
            shares.len(),
            crate::demo::figure(total)
        );
        self.modal = Some(Modal::Confirm {
            action: Confirm::UndoBatch(batch.id),
            label,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Kind};
    use crate::db::{GoalId, goal};
    use crate::money::Cents;
    use crate::test_support::day;
    use crate::tui::app::test_support::*;
    use crate::tui::cursor::Scroll;
    use crate::tui::modal::Modal;
    use crate::tui::search::Search;
    use crate::tui::{goal_form, worksheet as worksheet_screen};
    use ratatui::crossterm::event::KeyCode;

    /// The payday worksheet opens on the Tab container with `per_paycheck`
    /// down every line, so it starts at zero remaining.
    #[test]
    fn capital_a_opens_a_payday_worksheet_prefilled_from_per_paycheck() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));

        let sheet = worksheet(&app);
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        // The worksheet lists the container in screen order, so the undated
        // Couch leads it.
        assert_eq!(names, ["Couch", "Vacation 2027"]);
        // Couch is undated and asks nothing; Vacation 2027 needs 4,986.00
        // over 10 paychecks.
        assert_eq!(sheet.lines()[0].amount, Cents::ZERO);
        assert_eq!(sheet.lines()[1].amount, Cents(50_000));
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// One batch per commit, so a fumbled payday is one `U` rather than dozens
    /// of deletions -- and the goals move without another keystroke.
    #[test]
    fn committing_a_worksheet_writes_one_batch_and_reloads_the_screen() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Row 1 is Vacation 2027, the only goal the prefill asks anything of.
        let before = app.savings.rows()[1].current;
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, before + Cents(50_000));
        let batch = goal::most_recent_batch(&app.db).unwrap().unwrap();
        assert_eq!(batch.kind, goal::BatchKind::Paycheck);
        assert_eq!(goal::batch_shares(&app.db, batch.id).unwrap().len(), 1);
    }

    /// `/` then a digit is the fraction operator; `/` then anything else is
    /// the name filter. Both live under one key, so this is the test that
    /// keeps them apart.
    #[test]
    fn a_slash_then_a_digit_divides_and_a_slash_then_a_letter_filters() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        // Tab twice: Amount -> Date -> Lines.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('*'));
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(worksheet(&app).lines()[0].amount, Cents(25_000));

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('C'));
        assert!(worksheet(&app).is_searching());
        assert_eq!(worksheet(&app).search(), "C");
        // "Vacation 2027" also matches "C" (Vacation), so disambiguate before
        // asserting the filtered list -- the filter is a substring match, and
        // this test exists to prove `/`+digit divides while `/`+letter
        // filters, not to pin the filter's matching rule.
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('u'));
        let names: Vec<&str> = worksheet(&app)
            .lines()
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(names, ["Couch"]);
    }

    /// The operator keys are line-editing keys. With the date focused they
    /// have to type instead, or `s` would spread while you are fixing a date.
    #[test]
    fn the_operator_keys_type_into_the_date_field_rather_than_acting_on_lines() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);
        let before: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();

        press(&mut app, KeyCode::Char('s'));

        let after: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();
        assert_eq!(before, after, "s must not spread while the date is focused");
        assert!(worksheet(&app).date_text().ends_with('s'));
    }

    /// The worksheet's date is a text field with a key handler of its own,
    /// and the editing keys have to reach it there too.
    #[test]
    fn the_worksheets_date_answers_the_editing_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Tab);
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "2026-09-01");

        assert_eq!(worksheet(&app).date_text(), "2026-09-01");
    }

    /// `Ctrl` means editing text everywhere in the app, so it must not reach
    /// an operator: a hand reaching for "delete the last word" would
    /// otherwise spread the whole pot.
    #[test]
    fn a_ctrl_key_does_not_reach_the_worksheets_operators() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        let before: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();

        ctrl_press(&mut app, 'z');

        let after: Vec<Cents> = worksheet(&app).lines().iter().map(|l| l.amount).collect();
        assert_eq!(before, after, "Ctrl+Z zeroed the untargeted lines");
    }

    /// Over-allocating would hand out money the container does not hold, and
    /// the failure has to reach the status line rather than the panic hook.
    #[test]
    fn committing_an_over_allocated_worksheet_reports_it_and_keeps_the_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the worksheet must stay open");
        assert!(app.status.contains("over-allocated"), "{}", app.status);
    }

    /// The amount prefills from the container's unallocated remainder: the
    /// interest row is entered on the Cash screen first, so at that point the
    /// excess *is* the interest.
    #[test]
    fn i_opens_a_worksheet_on_the_containers_unallocated_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Give Rainy Day an unallocated balance to post: the fixture's goals hold
        // more than the account does, so top it up first.
        write(
            &app.db,
            app.savings.excess()[0].0,
            day(2026, 8, 15),
            1_100_000,
            "Interest",
        );
        app.reload().unwrap();
        let excess = app.savings.excess()[0].1;
        assert!(excess > Cents::ZERO);

        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        assert_eq!(sheet.kind(), goal::BatchKind::Interest);
        assert_eq!(sheet.amount(), excess);
        // Rainy Day is `manual` with no previous posting, so it falls back to
        // pro-rata across the eligible goals' balances.
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// A Brokerage-shaped container: `pro_rata`, three buckets, and the
    /// down-payment one excluded from the split the way `Planning!J7`
    /// excludes it. Its goals hold 1,945.00 less than the account does --
    /// the interest row, already entered on the Cash screen.
    ///
    /// Returned in screen order: down payment, emergency, mom and dad.
    fn pro_rata_container() -> (App, GoalId, GoalId, GoalId) {
        let db = db::open_in_memory().unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 0).unwrap();
        account::set_interest_policy(&db, brokerage, account::InterestPolicy::ProRata).unwrap();
        let add_goal = |name: &str, target: i64, eligible: bool, balance: i64| {
            let id = goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: brokerage,
                    base_cents: Cents(target),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: eligible,
                    sort: 0,
                    taxed: false,
                },
            )
            .unwrap();
            goal::insert_allocation(&db, id, day(2026, 1, 1), Cents(balance), None, None).unwrap();
            id
        };
        let down_payment = add_goal("Home Down Payment", 50_000_000, false, 50_000_000);
        let emergency = add_goal("Emergency Savings", 10_000_000, true, 10_600_195);
        let mom_and_dad = add_goal("Mom & Dad", 2_500_000, true, 2_500_000);
        write(&db, brokerage, day(2026, 7, 31), 63_300_195, "Balance");
        (
            App::new(db, today()).unwrap(),
            down_payment,
            emergency,
            mom_and_dad,
        )
    }

    /// `Planning!J7` forces the down-payment bucket's interest weight to zero,
    /// which the importer records as `interest_eligible = 0`. A prefill that
    /// ignored the flag would misallocate every future posting.
    #[test]
    fn a_pro_rata_prefill_skips_goals_that_are_not_interest_eligible() {
        let (mut app, down_payment, emergency, mom_and_dad) = pro_rata_container();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        let share = |id| {
            sheet
                .lines()
                .iter()
                .find(|l| l.goal_id == id)
                .expect("every open goal is a line")
                .amount
        };
        assert_eq!(share(down_payment), Cents::ZERO, "excluded from the split");
        assert_eq!(share(emergency), Cents::from_dollars(1_618));
        assert_eq!(share(mom_and_dad), Cents::from_dollars(382));
    }

    /// Eligibility is the owner's call once the import has had its say: a
    /// bucket the sheet excluded can be brought into the split without
    /// touching the importer or the database by hand.
    #[test]
    fn making_a_goal_interest_eligible_brings_it_into_the_split() {
        let (mut app, down_payment, ..) = pro_rata_container();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Goal(form)) = &app.modal else {
            panic!("e opens the goal form");
        };
        assert_eq!(
            form.display(goal_form::GoalField::Name).plain_text(),
            "Home Down Payment"
        );
        // Walked to rather than counted to: the form grows fields, and a
        // count would send the `Right` below into whichever one it grew.
        while !matches!(&app.modal, Some(Modal::Goal(form)) if form.focus == goal_form::GoalField::Interest)
        {
            press(&mut app, KeyCode::Tab);
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('i'));

        let sheet = worksheet(&app);
        let share = sheet
            .lines()
            .iter()
            .find(|l| l.goal_id == down_payment)
            .expect("every open goal is a line")
            .amount;
        assert!(share > Cents::ZERO, "now weighted, got {share}");
    }

    /// Every worksheet opens on the amount, which takes digits and drops
    /// everything else -- so the line operators have to be live there, or a
    /// tick does nothing until the reader has found `Tab` twice.
    #[test]
    fn space_ticks_a_line_on_a_worksheet_that_has_just_opened() {
        let (mut app, ..) = pro_rata_container();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(worksheet(&app).focus(), worksheet_screen::Focus::Amount);

        press(&mut app, KeyCode::Char(' '));

        assert_eq!(worksheet(&app).selected_count(), 1);
    }

    /// The flow the two keys exist for: tick who the posting funds, `z` to
    /// free the pot, `w` to divide it in the prefill's proportions. One
    /// ticked goal takes the lot.
    #[test]
    fn ticking_one_goal_then_z_and_w_posts_the_whole_excess_to_it() {
        let (mut app, down_payment, emergency, mom_and_dad) = pro_rata_container();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('i'));
        let pot = worksheet(&app).amount();

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('w'));

        let share = |id| {
            worksheet(&app)
                .lines()
                .iter()
                .find(|l| l.goal_id == id)
                .expect("every open goal is a line")
                .amount
        };
        assert_eq!(share(emergency), pot);
        assert_eq!(share(down_payment), Cents::ZERO);
        assert_eq!(share(mom_and_dad), Cents::ZERO);
        assert_eq!(worksheet(&app).remaining(), Cents::ZERO);
    }

    /// Committing an interest posting returns the container to reconciled,
    /// which is the whole shape of the operation.
    #[test]
    fn committing_an_interest_posting_returns_the_container_to_zero_excess() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        write(
            &app.db,
            app.savings.excess()[0].0,
            day(2026, 8, 15),
            1_100_000,
            "Interest",
        );
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.excess()[0].1, Cents::ZERO);
    }

    /// One batch per worksheet commit, so a fumbled payday is one `U` rather
    /// than dozens of deletions.
    #[test]
    fn capital_u_then_y_undoes_the_last_batch() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Row 1 is Vacation 2027, the only goal the prefill moves.
        let before = app.savings.rows()[1].current;
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        assert_ne!(app.savings.rows()[1].current, before);

        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, before);
        assert!(goal::most_recent_batch(&app.db).unwrap().is_none());
    }

    /// The confirmation exists because a batch is many rows: anything that is
    /// not `y` has to be a cancel rather than a fall-through.
    #[test]
    fn capital_u_then_any_other_key_cancels_and_the_batch_survives() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        let after_payday = app.savings.rows()[0].current;

        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('n'));

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[0].current, after_payday);
        assert!(goal::most_recent_batch(&app.db).unwrap().is_some());
    }

    /// The import batch holds every opening balance in the database. Undoing
    /// it would empty every goal in one keystroke, so `U` must not see it.
    #[test]
    fn capital_u_never_offers_the_import_batch() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Vacation 2027".to_string(),
                container_account_id: savings,
                base_cents: Cents(1_500_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocations(
            &db,
            goal::BatchKind::Import,
            day(2026, 8, 12),
            &[(id, Cents(1_000_000))],
            Some("imported balance"),
        )
        .unwrap();
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('U'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing to undo"), "{}", app.status);
        assert_eq!(app.savings.rows()[0].current, Cents(1_000_000));
    }

    /// On a modal the outer thing is the modal itself, so a kept filter has to
    /// be cleared before `Esc` may throw the worksheet away.
    #[test]
    fn esc_with_a_kept_filter_clears_it_rather_than_cancelling_the_worksheet() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "couch");
        press(&mut app, KeyCode::Enter);
        assert_eq!(worksheet(&app).lines().len(), 1);

        press(&mut app, KeyCode::Esc);
        assert_eq!(worksheet(&app).search(), "");
        assert_eq!(worksheet(&app).lines().len(), 2, "the sheet is still open");

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    /// The worksheet is a modal, so it is reached through the Savings screen rather
    /// than a screen key -- but it implements the same `Scroll` and must answer the
    /// same keys.
    #[test]
    fn the_scroll_keys_work_inside_the_worksheet() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('A'));
        // The line list is the third focus; the amount and the date come first.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);

        press(&mut app, KeyCode::End);
        let sheet = worksheet(&app);
        let last = sheet.selected_index();

        press(&mut app, KeyCode::Home);
        let sheet = worksheet(&app);
        assert_eq!(sheet.selected_index(), 0);
        assert!(last > 0, "End does not reach the last line");
    }
}
