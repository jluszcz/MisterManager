//! The two recurring screens, and the picker that turns a catalog entry into
//! a goal.
//!
//! Recurring transactions and recurring goals are separate tables answering
//! separate keys, and they share a module because they are one idea twice:
//! a rule on record, and the rows or goals regenerated from it.

use super::{Account, App, NOTHING_SELECTED};
use crate::db::recurring_goal::{self, Entry};
use crate::db::setting::{self, key};
use crate::db::{RecurringGoalId, account, goal, recurring_txn};
use crate::goal as goal_engine;
use crate::recurring_txn::{self as recurring_engine, Extended};
use crate::tui::cursor;
use crate::tui::modal::{Confirm, Modal};
use crate::tui::picker::{self, Picker};
use crate::tui::recurring_goal::RecurringGoalForm;
use crate::tui::recurring_txn::RecurringTxnForm;
use crate::tui::search::{self, Search};
use anyhow::{Result, ensure};
use chrono::Datelike;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashSet;

impl App {
    pub(super) fn recurring_txn_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.recurring_txn, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.open_recurring_txn_add()?,
            KeyCode::Char('e') => self.open_recurring_txn_edit()?,
            KeyCode::Char('d') => self.open_recurring_txn_delete(),
            KeyCode::Char('g') => self.regenerate_selected()?,
            KeyCode::Char('G') => self.regenerate_every()?,
            KeyCode::Char('x') => self.extend_selected()?,
            KeyCode::Char('P') => self.make_paycheck()?,
            _ => {}
        }
        Ok(())
    }

    fn open_recurring_txn_add(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::RecurringTxn(RecurringTxnForm::add(
            accounts, self.today,
        )?));
        Ok(())
    }

    fn open_recurring_txn_edit(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let found = recurring_txn::get(&self.db, row.recurring_txn_id)?;
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::RecurringTxn(RecurringTxnForm::edit(
            accounts, self.today, &found,
        )?));
        Ok(())
    }

    pub(super) fn commit_recurring_txn(&mut self) -> Result<()> {
        let Some(Modal::RecurringTxn(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                recurring_txn::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                recurring_txn::insert(&self.db, &new)?;
                "added"
            }
        };
        self.status = format!(
            "{verb} {} · g generates its rows",
            crate::demo::text(&new.description)
        );
        self.close_modal();
        self.reload()
    }

    /// The confirmation says what actually happens: the rows are released, not
    /// deleted, so no balance moves.
    fn open_recurring_txn_delete(&mut self) {
        match self.recurring_txn.selected() {
            None => self.status = NOTHING_SELECTED.to_string(),
            Some(row) => {
                let label = format!(
                    "{} · {} · {} ledger rows will be released, not deleted",
                    crate::demo::text(&row.description),
                    crate::demo::figure(row.cents),
                    row.owned
                );
                self.modal = Some(Modal::Confirm {
                    action: Confirm::DeleteRecurringTxn(row.recurring_txn_id),
                    label,
                });
            }
        }
    }

    fn regenerate_selected(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (
            row.recurring_txn_id,
            crate::demo::text(&row.description).into_owned(),
        );
        let report = recurring_engine::regenerate(&self.db, id, self.today)?;
        self.status = format!("{description}: {report}");
        self.reload()
    }

    fn regenerate_every(&mut self) -> Result<()> {
        let report = recurring_engine::regenerate_all(&self.db, self.today)?;
        self.status = format!("every recurring transaction: {report}");
        self.reload()
    }

    /// `x`: push the selected recurring transaction one rolling horizon
    /// further out and regenerate it.
    ///
    /// Both refusals name the date that binds, because "nothing happened" on
    /// a screen whose whole subject is future rows reads as a bug.
    fn extend_selected(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (
            row.recurring_txn_id,
            crate::demo::text(&row.description).into_owned(),
        );
        self.status = match recurring_engine::extend(&self.db, id, self.today)? {
            Extended::Through { through, report } => {
                format!("{description} extended through {through}: {report}")
            }
            Extended::Ends(end) => {
                format!("{description} ends {end} — change that with e")
            }
            Extended::Ceiling(reach) => {
                format!("{description} already reaches {reach}, ten years out")
            }
        };
        self.reload()
    }

    fn make_paycheck(&mut self) -> Result<()> {
        let Some(row) = self.recurring_txn.selected() else {
            return self.nothing_selected();
        };
        let (id, description) = (
            row.recurring_txn_id,
            crate::demo::text(&row.description).into_owned(),
        );
        recurring_txn::set_paycheck(&self.db, id)?;
        self.status = format!("{description} is now the paycheck transaction");
        self.reload()
    }

    pub(super) fn recurring_goal_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.recurring_goal, key.code) {
            return Ok(());
        }
        match key.code {
            // Pure view state: the screen holds every entry already, so
            // unlike the ledgers' `[` and `]` there is nothing to re-query.
            KeyCode::Char('[') => self.recurring_goal.previous_month(),
            KeyCode::Char(']') => self.recurring_goal.next_month(),
            KeyCode::Esc => {
                if !search::escape_kept_filter(&mut self.recurring_goal) {
                    self.recurring_goal.clear_month();
                }
            }
            KeyCode::Char('/') => self.recurring_goal.begin_search(),
            KeyCode::Char('a') => self.open_recurring_goal_add()?,
            KeyCode::Char('e') => self.open_recurring_goal_edit()?,
            KeyCode::Char('d') => self.open_recurring_goal_delete()?,
            KeyCode::Char('s') => self.open_recurring_goals()?,
            _ => {}
        }
        Ok(())
    }

    fn open_recurring_goal_add(&mut self) -> Result<()> {
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::add(
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }

    fn open_recurring_goal_edit(&mut self) -> Result<()> {
        let Some(row) = self.recurring_goal.selected() else {
            return self.nothing_selected();
        };
        let found = recurring_goal::get(&self.db, row.recurring_goal_id)?;
        self.modal = Some(Modal::RecurringGoalEntry(RecurringGoalForm::edit(
            &found,
            setting::get(&self.db, key::TAX_RATE)?,
        )));
        Ok(())
    }

    pub(super) fn commit_recurring_goal(&mut self) -> Result<()> {
        let Some(Modal::RecurringGoalEntry(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                recurring_goal::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                recurring_goal::insert(&self.db, &new)?;
                "added"
            }
        };
        self.status = format!("{verb} {}", crate::demo::text(&new.name));
        self.close_modal();
        self.reload()
    }

    /// The confirmation names the entry and how many goals -- open or closed
    /// -- reference it: that is the number `recurring_goal::delete` actually
    /// gates on, which is not the same as the screen's "Open" column. Querying
    /// it fresh rather than reading `row.open_goals` is what keeps the
    /// confirmation from promising a delete the gate is about to refuse.
    fn open_recurring_goal_delete(&mut self) -> Result<()> {
        let Some(row) = self.recurring_goal.selected() else {
            return self.nothing_selected();
        };
        let goals = recurring_goal::goal_count(&self.db, row.recurring_goal_id)?;
        let label = format!(
            "{} · {} · {goals} goal(s), open or closed, reference it",
            crate::demo::text(&row.name),
            crate::demo::figure(row.base_cents)
        );
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteRecurringGoal(row.recurring_goal_id),
            label,
        });
        Ok(())
    }

    pub(super) fn reload_recurring_txns(&mut self) -> Result<()> {
        self.recurring_txn.set_accounts(account::list(&self.db)?);
        self.recurring_txn.set_recurring_txns(
            recurring_txn::list(&self.db)?,
            recurring_txn::owned_counts(&self.db)?,
            recurring_txn::last_owned_dates(&self.db)?,
        );
        Ok(())
    }

    pub(super) fn reload_recurring_goals(&mut self) -> Result<()> {
        self.recurring_goal.set_entries(
            recurring_goal::list(&self.db)?,
            recurring_goal::open_goal_counts(&self.db)?,
            setting::get(&self.db, key::TAX_RATE)?,
            self.periods_per_year()?,
        )
    }

    /// `n`: the recurring goal, to create the next round of goals from.
    ///
    /// A goal ends and never becomes its successor, so the next round is
    /// created here rather than rolled forward on the goal that just closed.
    /// Opens on the container the screen is filtered to, or the first when the
    /// filter is All.
    /// `s` on the Recurring Goals screen: goals created *from* the entries the
    /// screen is showing.
    ///
    /// The container is the Savings screen's `Tab` filter, which is the app's
    /// one answer to "which container" -- this screen's entries carry none.
    /// The month filter preselects rather than narrows, and an entry that
    /// already has an open goal is left unticked: the annual reseed is what
    /// the preselection is for, and such an entry has already been through it.
    /// A second open goal against one entry is still legitimate, so `Space`
    /// still adds it.
    fn open_recurring_goals(&mut self) -> Result<()> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(());
        };
        let entries = recurring_goal::list(&self.db)?;
        if entries.is_empty() {
            self.status = "there are no recurring goals".to_string();
            return Ok(());
        }
        let counts = recurring_goal::open_goal_counts(&self.db)?;
        let month = self.recurring_goal.selected_month();
        let preselected: HashSet<RecurringGoalId> = entries
            .iter()
            .filter(|e| month.is_none_or(|m| e.month == m))
            .filter(|e| counts.get(&e.id).copied().unwrap_or(0) == 0)
            .map(|e| e.id)
            .collect();
        let account = account::get(&self.db, container)?;
        self.modal = Some(Modal::Picker(Picker::new(
            entries,
            counts,
            &preselected,
            Account::named(std::slice::from_ref(&account), container),
        )));
        Ok(())
    }

    pub(super) fn picker_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Enter {
            return self.commit_picker();
        }
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return Ok(());
        };
        if cursor::scroll_key(picker, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.close_modal(),
            KeyCode::Char(' ') => picker.toggle(),
            _ => {}
        }
        Ok(())
    }

    /// Create one goal per selected entry, all in one transaction.
    ///
    /// Nothing here refuses an entry that already has an open goal: goal names
    /// are not unique, and a second open goal against one recurring goal entry
    /// is a legitimate thing to want. The picker's "Open?" column is the hint.
    fn commit_picker(&mut self) -> Result<()> {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return Ok(());
        };
        let container = picker.container();
        let chosen: Vec<Entry> = picker.chosen().into_iter().cloned().collect();
        ensure!(!chosen.is_empty(), NOTHING_SELECTED);
        // The picker is the second place a taxed goal is written, alongside
        // the goal form's own commit, and it is the one that hands the flag
        // across rather than computing anything -- so it has to ask for the
        // rate here, before there is a goal for the read side to call corrupt.
        if chosen.iter().any(|entry| entry.taxed) {
            ensure!(
                setting::get(&self.db, key::TAX_RATE)?.is_some(),
                goal_engine::NO_TAX_RATE
            );
        }
        // Every goal created here is dated, so each takes its place in the
        // container's dated block by deadline rather than landing at the end.
        // `sort` still runs in the order the picker showed them -- the ticked
        // group first, since that is the order the picker sorted itself into --
        // but among dated goals it decides only which of two falling on the
        // same day comes first.
        let first_sort = goal::next_sort(&self.db, container)?;
        let mut new_goals = Vec::with_capacity(chosen.len());
        for (offset, entry) in chosen.iter().enumerate() {
            let has_goal_this_year =
                goal::has_goal_dated_in_year(&self.db, entry.id, self.today.year())?;
            new_goals.push(goal::NewGoal {
                name: entry.name.clone(),
                container_account_id: container,
                // The entry's base and its flag, handed across rather than
                // spent: a goal made from a taxed entry is indistinguishable
                // from one the owner marked taxed by hand, and the lambda runs
                // once, on read.
                base_cents: entry.base_cents,
                goal_date: Some(picker::goal_date(entry, has_goal_this_year, self.today)?),
                recurring_goal_id: Some(entry.id),
                interest_eligible: true,
                sort: first_sort + offset as i64,
                taxed: entry.taxed,
            });
        }
        goal::insert_all(&self.db, &new_goals)?;
        self.status = format!("created {} goals", new_goals.len());
        self.close_modal();
        self.reload()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Group, Kind};
    use crate::db::setting::{self, key};
    use crate::db::txn::{Filter, NewTxn};
    use crate::db::{Db, goal, recurring_goal, recurring_txn, txn};
    use crate::money::Cents;
    use crate::test_support::day;
    use crate::tui::app::Screen;
    use crate::tui::app::test_support::*;
    use crate::tui::cursor::Scroll;
    use crate::tui::modal::{Confirm, Modal};
    use crate::tui::planning::Target;
    use crate::{db, goal as goal_engine};
    use chrono::NaiveDate;
    use ratatui::crossterm::event::KeyCode;

    /// `s` creates every selected entry at once, dating each one and recording
    /// the entry it came from -- so the picker's "Open?" column stays true.
    #[test]
    fn s_creates_a_goal_for_every_selected_catalog_entry() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(128),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let created = app
            .savings
            .rows()
            .into_iter()
            .find(|r| r.name == "Dropbox")
            .expect("the new goal must be on screen without another keystroke");
        assert_eq!(created.goal, Cents::from_dollars(128));
        assert_eq!(
            created.goal_date,
            Some(day(2027, 9, 1)),
            "a reseed is for the year ahead"
        );
        assert_eq!(created.current, Cents::ZERO);
    }

    /// `PAY_PERIODS_PER_YEAR` is editable on the Planning screen, and that
    /// commit reloads every screen -- so the Recurring Goals title divides by
    /// the count the database now holds rather than the one the app opened
    /// with, which is what every other per-paycheck figure in the app just
    /// moved to.
    #[test]
    fn editing_the_pay_period_count_moves_the_recurring_goals_title() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Dropbox".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(1_300),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        assert_eq!(
            app.recurring_goal.title(),
            "Recurring Goals · All · $1,300 Annually ($50/paycheck)"
        );

        Target::PeriodsPerYear
            .write(&app.db, today(), "24")
            .unwrap();
        app.reload().unwrap();
        assert_eq!(
            app.recurring_goal.title(),
            "Recurring Goals · All · $1,300 Annually ($55/paycheck)"
        );
    }

    /// The wiring rather than the rule, which `picker`'s own tests pin:
    /// `commit_picker` has to ask about *this calendar year*, so an entry that
    /// has already had its round skips the year between rather than repeating
    /// it.
    #[test]
    fn s_dates_a_biennial_entry_that_has_had_this_years_round_two_years_out() {
        let mut app = app();
        let id = recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: recurring_goal::Cadence::Biennial,
            },
        )
        .unwrap();
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Backblaze".to_string(),
                container_account_id: app.savings.default_container().unwrap(),
                base_cents: Cents::from_dollars(99),
                goal_date: Some(day(2026, 11, 1)),
                recurring_goal_id: Some(id),
                interest_eligible: true,
                sort: 9,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        // It has an open goal, so it opens unticked -- Space is the deliberate
        // second round the picker never refuses.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);

        let dates: Vec<Option<NaiveDate>> = app
            .savings
            .rows()
            .into_iter()
            .filter(|r| r.name == "Backblaze")
            .map(|r| r.goal_date)
            .collect();
        assert_eq!(dates, [Some(day(2026, 11, 1)), Some(day(2028, 11, 1))]);
    }

    /// `Tax()` applies to the entries flagged for it and to no others -- the
    /// workbook's own `Savings!Q` column is a mix.
    ///
    /// The picker hands the flag across rather than spending it, so the screen
    /// shows the derived target while the table holds the base.
    #[test]
    fn only_a_taxed_catalog_entry_goes_through_the_tax_lambda() {
        let mut app = app();
        setting::set(&app.db, key::TAX_RATE, crate::rate::BasisPoints(625)).unwrap();
        for (name, taxed) in [("Rolex", true), ("Dropbox", false)] {
            recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month: 9,
                    base_cents: Cents::from_dollars(9_000),
                    taxed,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
        }
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        let target = |name: &str| {
            app.savings
                .rows()
                .into_iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no goal named {name}"))
                .goal
        };
        // 9,000 × 1.0625 = 9,562.50, rounded up to the next $5.
        assert_eq!(target("Rolex"), Cents::from_dollars(9_565));
        assert_eq!(target("Dropbox"), Cents::from_dollars(9_000));

        // ...and what the *table* holds is the base and the flag, handed
        // across rather than spent: the lambda runs on read, so a goal made
        // from a taxed entry is indistinguishable from one the owner marked
        // taxed by hand, and nothing can tax the taxed figure a second time.
        let stored = |name: &str| {
            let g = goal::all_with_balances(&app.db)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no goal named {name}"));
            (g.goal.base_cents, g.goal.taxed)
        };
        assert_eq!(stored("Rolex"), (Cents::from_dollars(9_000), true));
        assert_eq!(stored("Dropbox"), (Cents::from_dollars(9_000), false));
    }

    /// The picker is the second place a taxed goal is written, alongside the
    /// goal form's own commit, and it hands the flag across rather than
    /// computing anything -- so on a database with no rate on record it has
    /// to refuse before `insert_all` ever runs. Without the guard this writes
    /// `taxed = 1` with no rate anywhere, which is the row the read side
    /// calls corrupt -- every screen then draws it against its base, and
    /// every path that would spend the figure refuses.
    #[test]
    fn s_refuses_a_taxed_entry_on_a_database_with_no_tax_rate_and_writes_no_goal() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Rolex".to_string(),
                month: 9,
                base_cents: Cents::from_dollars(9_000),
                taxed: true,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the picker stays open on a refusal");
        assert_eq!(app.status, goal_engine::NO_TAX_RATE);
        assert!(
            goal::all_with_balances(&app.db)
                .unwrap()
                .iter()
                .all(|g| g.goal.name != "Rolex"),
            "no goal is written"
        );
    }

    /// "Open?" is a hint, never a block: goal names are not unique, and a
    /// second open goal against one entry is a legitimate thing to want. It is
    /// the one the preselection will not tick, so the second round takes an
    /// explicit `Space` where the first took none.
    #[test]
    fn a_second_goal_against_an_entry_that_already_has_one_is_allowed() {
        let mut app = app();
        recurring_goal::insert(
            &app.db,
            &recurring_goal::NewEntry {
                name: "Lego".to_string(),
                month: 12,
                base_cents: Cents::from_dollars(340),
                taxed: false,
                cadence: recurring_goal::Cadence::Annual,
            },
        )
        .unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);

        let legos = app
            .savings
            .rows()
            .into_iter()
            .filter(|r| r.name == "Lego")
            .count();
        assert_eq!(legos, 2);
    }

    fn recurring_txns_app() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        // The workbook's pre-entered future rows, unclaimed. A "Salary
        // Income" row lets the paycheck form's Esc dismiss the suggestion
        // popup instead of the whole form, the same way "Mortgage" does.
        write(&db, checking, day(2026, 9, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 10, 1), -120_000, "Mortgage");
        write(&db, checking, day(2026, 8, 28), 500_000, "Salary");
        App::new(db, today()).unwrap()
    }

    fn add_mortgage_rule(app: &mut App) {
        press(app, KeyCode::Char('8'));
        press(app, KeyCode::Char('a'));
        type_str(app, "Mortgage");
        press(app, KeyCode::Esc); // dismiss the suggestion popup, keep the form
        press(app, KeyCode::Tab);
        type_str(app, "-1200.00");
        press(app, KeyCode::Tab);
        press(app, KeyCode::Tab);
        press(app, KeyCode::Right); // monthly
        press(app, KeyCode::Tab);
        for _ in 0..10 {
            press(app, KeyCode::Backspace);
        }
        type_str(app, "2026-09-01");
        press(app, KeyCode::Enter);
    }

    #[test]
    fn seven_opens_the_rules_screen_and_a_writes_a_rule() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);

        assert_eq!(app.screen, Screen::RecurringTxns);
        assert!(app.modal.is_none(), "{}", app.status);
        let recurring_txn = crate::db::recurring_txn::list(&app.db).unwrap();
        assert_eq!(recurring_txn.len(), 1);
        assert_eq!(recurring_txn[0].description, "Mortgage");
        assert_eq!(
            recurring_txn[0].cadence,
            crate::db::recurring_txn::Cadence::Monthly
        );
        assert_eq!(
            app.recurring_txn.rows()[0].owned,
            0,
            "nothing generated yet"
        );
    }

    /// Adoption is invisible otherwise: "adopted 2" is how the owner sees that
    /// the first `g` claimed the imported rows instead of duplicating them.
    #[test]
    fn g_regenerates_the_selected_rule_and_reports_its_counts() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        let before = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert!(app.status.contains("adopted 2"), "{}", app.status);
        assert!(app.status.contains("inserted 1"), "{}", app.status);
        assert_eq!(
            txn::count(&app.db).unwrap(),
            before + 1,
            "the two imported rows were adopted, not duplicated"
        );
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    /// From 2026-08-15 the three-month window ends 2026-11-15, so the
    /// mortgage's rows stop at 2026-11-01. One `x` buys another window: the
    /// rows run to 2027-02-01, and the screen's date column says so.
    #[test]
    fn x_extends_the_selected_rule_and_the_last_date_follows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2026, 11, 1))
        );

        press(&mut app, KeyCode::Char('x'));

        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2027, 2, 1))
        );
        assert_eq!(app.recurring_txn.rows()[0].owned, 6);
        assert!(app.status.contains("2027-02-15"), "{}", app.status);
    }

    /// The extension is durable, or the next `g` would sweep the extra rows
    /// straight back out again.
    #[test]
    fn a_g_after_an_x_keeps_the_extended_rows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('x'));
        let extended = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert_eq!(txn::count(&app.db).unwrap(), extended);
        assert_eq!(
            app.recurring_txn.rows()[0].last_owned,
            Some(day(2027, 2, 1))
        );
    }

    /// A recurring transaction that ends cannot be extended past its end:
    /// moving that date is the form's decision, and the status line says
    /// where to make it.
    #[test]
    fn x_on_a_rule_that_has_ended_names_the_date_it_ends() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        let id = app.recurring_txn.rows()[0].recurring_txn_id;
        let mut ends = recurring_txn::get(&app.db, id).unwrap();
        ends.horizon = Some(day(2026, 10, 1));
        recurring_txn::update(
            &app.db,
            id,
            &recurring_txn::NewRecurringTxn {
                description: ends.description,
                cents: ends.cents,
                account_id: ends.account_id,
                cadence: ends.cadence,
                anchor_date: ends.anchor_date,
                horizon: ends.horizon,
            },
        )
        .unwrap();

        press(&mut app, KeyCode::Char('x'));

        assert!(app.status.contains("2026-10-01"), "{}", app.status);
        assert_eq!(
            recurring_txn::get(&app.db, id).unwrap().generate_through,
            None,
            "a refused extension writes nothing"
        );
    }

    /// Regenerating twice in a row must produce identical rows.
    #[test]
    fn a_second_g_changes_nothing() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        let after_first = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('g'));

        assert_eq!(txn::count(&app.db).unwrap(), after_first);
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    #[test]
    fn capital_g_regenerates_every_rule() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('G'));
        assert!(app.status.contains("adopted 2"), "{}", app.status);
        assert_eq!(app.recurring_txn.rows()[0].owned, 3);
    }

    /// The whole point of the flag: Paycheck-Eve stops being today.
    #[test]
    fn capital_p_names_the_paycheck_rule_and_the_overview_column_follows_it() {
        let mut app = recurring_txns_app();
        assert_eq!(app.dates.adhoc, today(), "no paycheck transaction yet");

        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Salary");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "5000.00");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-08-28");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('P'));

        assert!(app.recurring_txn.rows()[0].is_paycheck);
        assert_eq!(app.dates.adhoc, day(2026, 8, 27));
        assert_eq!(
            app.adhoc,
            day(2026, 8, 27),
            "the scrub follows the baseline"
        );
        assert_eq!(app.scrubbed_days(), 0);
    }

    /// At offset 0 `self.adhoc = self.dates.adhoc` would pass just as well --
    /// this scrubs first, so only carrying the *offset* across the moved
    /// baseline can make it pass.
    #[test]
    fn capital_p_moves_the_baseline_and_carries_a_nonzero_scrub_across_it() {
        let mut app = recurring_txns_app();
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.scrubbed_days(), 2, "scrubbed two days off today");

        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Salary");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "5000.00");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-08-28");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('P'));

        assert_eq!(app.dates.adhoc, day(2026, 8, 27), "the baseline moved");
        assert_eq!(
            app.scrubbed_days(),
            2,
            "the scrub is unchanged, not reset by the moved baseline"
        );
        assert_eq!(
            app.adhoc,
            day(2026, 8, 29),
            "still two days past the new baseline"
        );
    }

    /// Deleting a recurring transaction must not silently move a balance.
    #[test]
    fn d_then_y_deletes_a_rule_and_releases_its_rows() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        let rows = txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(crate::db::recurring_txn::list(&app.db).unwrap().is_empty());
        assert_eq!(txn::count(&app.db).unwrap(), rows, "no row was deleted");
        assert!(app.status.contains("released"), "{}", app.status);
    }

    #[test]
    fn d_on_an_empty_rules_screen_says_nothing_is_selected() {
        let mut app = recurring_txns_app();
        press(&mut app, KeyCode::Char('8'));
        press(&mut app, KeyCode::Char('d'));
        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing selected"), "{}", app.status);
    }

    #[test]
    fn e_opens_the_selected_rule_prefilled() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);

        press(&mut app, KeyCode::Char('e'));

        match &app.modal {
            Some(Modal::RecurringTxn(form)) => {
                assert_eq!(
                    form.display(crate::tui::recurring_txn::RecurringTxnField::Description)
                        .plain_text(),
                    "Mortgage"
                );
                assert_eq!(
                    form.editing,
                    Some(crate::db::recurring_txn::list(&app.db).unwrap()[0].id)
                );
            }
            _ => panic!("no recurring transaction form is open"),
        }
    }

    fn all_cash_rows(db: &Db) -> Vec<crate::db::txn::Txn> {
        txn::list(
            db,
            &Filter {
                kind: Kind::Cash,
                account_id: None,
                from: day(2000, 1, 1),
                to: day(2100, 1, 1),
            },
        )
        .unwrap()
    }

    /// `e`'s only write is `recurring_txn::update`; it does not regenerate.
    /// When the anchor moves and `g` runs next, the hand-corrected row from
    /// the old schedule is exempt from `release_generated` (it is `edited`)
    /// and matches no occurrence the new schedule produces, so step 1b hands
    /// it back to the ledger: the correction survives untouched, but as an
    /// ordinary row the owner can keep or delete rather than one the recurring
    /// transaction still claims.
    ///
    /// The row itself stays: nothing decides that a correction moved off its
    /// occurrence *consumes* that occurrence, so four rows stand where the
    /// schedule says three. Ownership is coherent; the count is the owner's
    /// call.
    #[test]
    fn editing_a_rules_anchor_releases_the_hand_corrected_row_at_the_old_occurrence() {
        let mut app = recurring_txns_app();
        add_mortgage_rule(&mut app);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.recurring_txn.rows()[0].owned, 3, "09-01, 10-01, 11-01");

        // Hand-correct the first generated row -- the same effect a ledger
        // edit has: any write through `txn::update` on a recurring
        // transaction-owned row flags it `edited`.
        let original = all_cash_rows(&app.db)
            .into_iter()
            .find(|t| t.date == day(2026, 9, 1))
            .unwrap();
        txn::update(
            &app.db,
            original.id,
            &NewTxn {
                date: original.date,
                cents: Cents(-300_000),
                account_id: original.account_id,
                description: original.description.clone(),
                recurring_txn_id: original.recurring_txn_id,
            },
        )
        .unwrap();

        // Move the recurring transaction's whole schedule off the 1st and onto
        // the 15th.
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab); // Amount
        press(&mut app, KeyCode::Tab); // Account
        press(&mut app, KeyCode::Tab); // Cadence
        press(&mut app, KeyCode::Tab); // Anchor
        for _ in 0..10 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2026-09-15");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "{}", app.status);

        // `e` alone does not regenerate: nothing moves until the next `g`.
        press(&mut app, KeyCode::Char('g'));

        assert!(app.status.contains("released 1"), "{}", app.status);
        let rows = all_cash_rows(&app.db);
        let given_back = rows
            .iter()
            .find(|t| t.date == day(2026, 9, 1))
            .expect("the hand-corrected row was released, not deleted");
        assert_eq!(given_back.cents, Cents(-300_000), "the correction survives");
        assert_eq!(
            given_back.recurring_txn_id, None,
            "no longer claimed by a recurring transaction whose schedule does not produce this date"
        );
        assert!(!given_back.edited, "a released row is nobody's correction");

        // The released row stays in, not filtered out: a spurious second
        // insert at 09-01 must fail this the same way a missing 09-15 would.
        let mortgage_dates: Vec<NaiveDate> = rows
            .iter()
            .filter(|t| t.description == "Mortgage")
            .map(|t| t.date)
            .collect();
        assert_eq!(
            mortgage_dates,
            vec![
                day(2026, 9, 1),
                day(2026, 9, 15),
                day(2026, 10, 15),
                day(2026, 11, 15),
            ],
            "the recurring transaction regenerated on the new schedule, leaving the released row behind rather than folding it in"
        );
        assert_eq!(
            app.recurring_txn.rows()[0].owned,
            3,
            "the recurring transaction owns its three occurrences and nothing else"
        );
    }

    #[test]
    fn seven_opens_the_catalog_and_a_adds_an_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "128");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let entries = recurring_goal::list(&app.db).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Dropbox");
        assert_eq!(app.recurring_goal.rows()[0].name, "Dropbox");
    }

    fn add_dropbox_entry(app: &mut App) {
        press(app, KeyCode::Char('7'));
        press(app, KeyCode::Char('a'));
        type_str(app, "Dropbox");
        press(app, KeyCode::Tab);
        for _ in 1..9 {
            press(app, KeyCode::Right); // January to September
        }
        press(app, KeyCode::Tab);
        type_str(app, "128");
        press(app, KeyCode::Enter);
    }

    /// The keys reach the screen. `app()`'s today is in August, so the first
    /// `]` out of All lands there — where nothing recurs — and the
    /// second on September, where the Dropbox entry is.
    #[test]
    fn the_bracket_keys_and_esc_filter_the_catalog_by_month() {
        let mut app = app();
        add_dropbox_entry(&mut app);
        assert_eq!(app.recurring_goal.selected_month(), None);

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.selected_month(), Some(8));
        assert!(
            app.recurring_goal.rows().is_empty(),
            "August holds no entry"
        );

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.rows()[0].name, "Dropbox");

        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.recurring_goal.selected_month(), Some(8));

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.recurring_goal.selected_month(), None);
        assert_eq!(app.recurring_goal.rows().len(), 1);
    }

    #[test]
    fn e_opens_the_selected_catalog_entry_prefilled() {
        let mut app = app();
        add_dropbox_entry(&mut app);

        press(&mut app, KeyCode::Char('e'));

        match &app.modal {
            Some(Modal::RecurringGoalEntry(form)) => {
                assert_eq!(
                    form.display(crate::tui::recurring_goal::RecurringGoalField::Name)
                        .plain_text(),
                    "Dropbox"
                );
                assert_eq!(
                    form.editing,
                    Some(recurring_goal::list(&app.db).unwrap()[0].id)
                );
            }
            _ => panic!("no recurring goal form is open"),
        }
    }

    /// The wiring `e` depends on: `commit_recurring_goal` must reach
    /// `recurring_goal::update` with the id the form is editing. An `insert`
    /// here, or the wrong id, leaves the old entry in place beside a new one.
    #[test]
    fn committing_an_edited_catalog_entry_updates_it_rather_than_adding_a_second() {
        let mut app = app();
        add_dropbox_entry(&mut app);
        let before = recurring_goal::list(&app.db).unwrap()[0].id;

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab); // Month
        press(&mut app, KeyCode::Tab); // Base
        for _ in 0..6 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "144");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let entries = recurring_goal::list(&app.db).unwrap();
        assert_eq!(entries.len(), 1, "the entry was updated, not duplicated");
        assert_eq!(entries[0].id, before);
        assert_eq!(entries[0].base_cents, Cents::from_dollars(144));
        assert_eq!(
            app.recurring_goal.rows()[0].base_cents,
            Cents::from_dollars(144)
        );
    }

    #[test]
    fn e_on_an_empty_catalog_says_nothing_is_selected() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('e'));
        assert!(app.modal.is_none());
        assert!(app.status.contains("nothing selected"), "{}", app.status);
    }

    #[test]
    fn d_on_a_catalog_entry_with_goals_against_it_is_refused_and_says_why() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "128");
        press(&mut app, KeyCode::Enter);
        // Create a goal from it, through the picker `s` opens.
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(recurring_goal::list(&app.db).unwrap().len(), 1);
        assert!(
            app.status.contains("reference this recurring goal"),
            "{}",
            app.status
        );
    }

    /// The screen's "Open" column and the confirmation box read from two
    /// different counts on purpose: `recurring_goal::goal_count`, which is
    /// what `recurring_goal::delete` actually gates on, counts a closed goal
    /// same as an open one. An entry whose only goal has been closed must
    /// still show and confirm a nonzero count, not the zero the "Open" column
    /// shows.
    #[test]
    fn d_on_a_catalog_entry_whose_only_goal_is_closed_still_confirms_a_nonzero_count() {
        let mut app = app();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Dropbox");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        // $500.00 rather than $128.00: no digit `1` anywhere in the rendered
        // amount, so a stray `1` in the price cannot rescue an assertion
        // that is meant to be reading the goal count.
        type_str(&mut app, "500");
        press(&mut app, KeyCode::Enter);
        // Create a goal from it, through the picker `s` opens, then close it
        // out.
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);
        let goal_id = goal::all_with_balances(&app.db)
            .unwrap()
            .into_iter()
            .find(|g| g.goal.name == "Dropbox")
            .unwrap()
            .goal
            .id;
        goal::close(&app.db, goal_id).unwrap();
        // The close went straight through `db::goal`, bypassing `App`'s own
        // write path, so the cached screen needs an explicit reload to catch
        // up -- the same as any other out-of-band change would.
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('7'));
        assert_eq!(
            app.recurring_goal.rows()[0].open_goals,
            0,
            "the goal is closed, not open"
        );

        press(&mut app, KeyCode::Char('d'));
        match &app.modal {
            Some(Modal::Confirm {
                action: Confirm::DeleteRecurringGoal(_),
                label,
            }) => {
                // The exact count, not a bare digit: a label built from
                // `row.open_goals` would read "0 open goals" here, which
                // also happens to contain a `1` if the amount were $128.00
                // -- this is the assertion the review asked for.
                assert!(label.contains("1 goal(s)"), "{label}");
            }
            _ => panic!("no delete confirmation is open"),
        }

        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            recurring_goal::list(&app.db).unwrap().len(),
            1,
            "the closed goal still blocks the delete"
        );
        assert!(app.status.contains("1 goal(s)"), "{}", app.status);
    }

    /// The eighth `Scroll` implementor, and the one the shared fixtures cannot
    /// reach on their own: the picker only opens over recurring goal entries, so
    /// two are inserted here to give `End` somewhere to go.
    #[test]
    fn the_scroll_keys_work_inside_the_recurring_goal_picker() {
        let mut app = app();
        for name in ["Dropbox", "Rolex"] {
            recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month: 9,
                    base_cents: Cents::from_dollars(128),
                    taxed: false,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
        }
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        press(&mut app, KeyCode::End);
        assert_eq!(
            picker(&app).selected_index(),
            1,
            "End must reach the last entry"
        );

        press(&mut app, KeyCode::Home);
        assert_eq!(picker(&app).selected_index(), 0);
    }

    /// The names the Goals screen is left showing.
    fn entry_names(app: &App) -> Vec<String> {
        app.recurring_goal
            .rows()
            .iter()
            .map(|r| r.name.clone())
            .collect()
    }

    /// The same box the ledgers and Savings open, on the screen that had only
    /// a month filter until now.
    #[test]
    fn slash_on_the_goals_screen_narrows_the_entries_as_they_are_typed() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "ro");

        assert!(app.recurring_goal.is_searching());
        assert_eq!(entry_names(&app), ["Dropbox", "Rolex"]);
    }

    /// Both filters at once, so the needle cannot be narrowing one list while
    /// the month narrows another.
    #[test]
    fn the_goals_month_filter_narrows_within_a_kept_search() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "ro");
        press(&mut app, KeyCode::Enter);
        assert!(!app.recurring_goal.is_searching(), "Enter left the box");

        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.selected_month(), Some(9));
        assert_eq!(entry_names(&app), ["Dropbox"]);
    }

    /// `Esc` inside the box is the box's, which is the handler the month
    /// filter must not have taken over.
    #[test]
    fn esc_in_the_goals_search_box_clears_the_search_not_the_month() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "rol");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.recurring_goal.search(), "");
        assert_eq!(app.recurring_goal.selected_month(), Some(8));
    }

    /// `Enter` leaves the box and keeps the needle, so `Esc` outside it is
    /// what clears one -- and the needle goes before the month, the same
    /// order every other `/` screen reads `Esc` in.
    #[test]
    fn esc_outside_the_goals_box_clears_a_kept_search_before_the_month() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "lego");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.recurring_goal.search(), "");
        assert_eq!(
            app.recurring_goal.selected_month(),
            Some(9),
            "the month is the next thing out, not this one"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.recurring_goal.selected_month(), None);
        assert_eq!(entry_names(&app), ["Dropbox", "Lego", "Rolex"]);
    }

    /// A digit or a `q` typed into the box is part of the needle: `dispatch`
    /// hands the key to the box above its own screen and quit arms.
    #[test]
    fn q_while_searching_the_goals_screen_types_into_the_box() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "q");

        assert!(!app.should_quit());
        assert_eq!(app.recurring_goal.search(), "q");
        assert!(app.recurring_goal.rows().is_empty());
    }

    /// `e`, `d` and `s` act on the selection, so a narrowed list must leave
    /// the cursor inside it rather than on the row that was there before.
    #[test]
    fn a_kept_search_is_what_the_goals_row_keys_act_on() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::End);
        assert_eq!(app.recurring_goal.selected().unwrap().name, "Rolex");

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "lego");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        match &app.modal {
            Some(Modal::RecurringGoalEntry(form)) => assert_eq!(
                form.editing,
                Some(app.recurring_goal.selected().unwrap().recurring_goal_id)
            ),
            _ => panic!("e opened no form"),
        }
        assert_eq!(entry_names(&app), ["Lego"]);
    }

    /// Three entries: two in September and one in October, with one of
    /// September's already carrying an open goal.
    fn app_with_recurring_goals() -> App {
        let mut app = app();
        for (name, month) in [("Dropbox", 9), ("Lego", 9), ("Rolex", 10)] {
            let id = recurring_goal::insert(
                &app.db,
                &recurring_goal::NewEntry {
                    name: name.to_string(),
                    month,
                    base_cents: Cents::from_dollars(128),
                    taxed: false,
                    cadence: recurring_goal::Cadence::Annual,
                },
            )
            .unwrap();
            if name == "Dropbox" {
                goal::insert(
                    &app.db,
                    &goal::NewGoal {
                        name: name.to_string(),
                        container_account_id: app.savings.default_container().unwrap(),
                        base_cents: Cents::from_dollars(128),
                        goal_date: Some(day(2026, 9, 1)),
                        recurring_goal_id: Some(id),
                        interest_eligible: true,
                        sort: 9,
                        taxed: false,
                    },
                )
                .unwrap();
            }
        }
        app.reload().unwrap();
        app
    }

    /// `[`/`]` from All enter at today's month, so a second step reaches
    /// September.
    fn open_september_picker(app: &mut App) {
        press(app, KeyCode::Char('7'));
        press(app, KeyCode::Char(']'));
        press(app, KeyCode::Char(']'));
        assert_eq!(app.recurring_goal.selected_month(), Some(9));
        press(app, KeyCode::Char('s'));
    }

    #[test]
    fn s_preselects_the_entries_the_month_filter_is_showing() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert_eq!(chosen(&app), ["Lego"], "October's entry came along");
    }

    /// The annual reseed is what the preselection is for, and an entry that
    /// already has an open goal is one it has already been through. Selecting
    /// it again is still legal -- `Space` -- just not the default.
    #[test]
    fn s_does_not_preselect_an_entry_that_already_has_an_open_goal() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert!(!chosen(&app).contains(&"Dropbox".to_string()));
    }

    /// The filter preselects; it does not narrow. September's filter is a
    /// starting point the list can still be scrolled out of.
    #[test]
    fn s_lists_every_entry_even_under_a_month_filter() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        let picker = picker(&app);
        let names: Vec<&str> = picker.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
    }

    /// Ticks alone are easy to miss in a list dozens long. The ticked entries are
    /// also the ones the list opens on, in the order the table holds them.
    #[test]
    fn s_sorts_the_entries_it_ticked_to_the_top() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);

        assert_eq!(entries(&app), ["Lego", "Dropbox", "Rolex"]);
    }

    /// Under All the split is open-goal alone, so the entries a reseed would
    /// create float above the ones it would skip.
    #[test]
    fn s_sorts_the_unopened_entries_above_the_rest_under_the_all_filter() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        assert_eq!(entries(&app), ["Lego", "Rolex", "Dropbox"]);
    }

    #[test]
    fn s_under_the_all_filter_preselects_every_unopened_entry() {
        let mut app = app_with_recurring_goals();
        press(&mut app, KeyCode::Char('7'));
        press(&mut app, KeyCode::Char('s'));

        assert_eq!(chosen(&app), ["Lego", "Rolex"]);
    }

    #[test]
    fn enter_on_the_preselected_picker_creates_the_goals_it_shows_as_chosen() {
        let mut app = app_with_recurring_goals();
        open_september_picker(&mut app);
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        press(&mut app, KeyCode::Char('4'));
        assert!(savings_names(&app).contains(&"Lego".to_string()));
    }
}
