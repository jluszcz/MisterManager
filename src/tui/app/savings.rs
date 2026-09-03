//! The Savings screen: the goals in each container, and the writes that
//! allocate to one, rewrite it, reorder it, or end it.
//!
//! `reload_savings` runs inside `App::new`, which is why it reads goals
//! through [`Reading::Tolerant`]: a strict read here would stop the
//! application starting over a tax rate that is set from inside it.

use super::{Account, App, Move};
use crate::db::setting::{self, key};
use crate::db::{GoalId, account, goal};
use crate::goal as goal_engine;
use crate::money::Cents;
use crate::reading::Reading;
use crate::tui::cursor;
use crate::tui::goal_form::{AllocationForm, CloseForm, GoalForm, GoalTarget, GoalTransferForm};
use crate::tui::modal::Modal;
use crate::tui::search::{self, Search};
use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn savings_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.savings, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Tab => self.savings.next_container(),
            KeyCode::BackTab => self.savings.previous_container(),
            // Pure view state, like the Recurring Goals screen's and unlike
            // the ledgers': every goal is already loaded for the footer's
            // reconciliation, so there is nothing to re-query.
            KeyCode::Char('[') => self.savings.previous_month(),
            KeyCode::Char(']') => self.savings.next_month(),
            KeyCode::Esc => {
                if !search::escape_kept_filter(&mut self.savings) {
                    self.savings.clear_filters();
                }
            }
            KeyCode::Char('/') => self.savings.begin_search(),
            KeyCode::Char('a') => self.open_allocate()?,
            KeyCode::Char('A') => self.open_payday()?,
            KeyCode::Char('i') => self.open_interest()?,
            KeyCode::Char('t') => self.open_goal_transfer()?,
            KeyCode::Char('e') => self.open_goal_edit()?,
            KeyCode::Char('c') => self.open_close_out()?,
            KeyCode::Char('n') => self.open_new_goal()?,
            KeyCode::Char('K') => self.move_goal(Move::Up)?,
            KeyCode::Char('J') => self.move_goal(Move::Down)?,
            KeyCode::Char('f') => self.toggle_favorite()?,
            KeyCode::Char('U') => self.open_undo()?,
            // The long form of the balance cell the row already carries.
            KeyCode::Enter => self.open_history()?,
            _ => {}
        }
        Ok(())
    }

    /// [`Reading::Tolerant`], because this runs during `App::new`: a taxed
    /// goal with no rate on record would otherwise stop the application
    /// starting, and the rate is set from inside it.
    pub(super) fn reload_savings(&mut self) -> Result<()> {
        self.savings.set_goals(
            goal_engine::all_with_balances(&self.db, Reading::Tolerant)?,
            self.periods_per_year()?,
        )?;
        let excess = crate::savings::containers_with_excess(&self.db)?;
        let containers = excess.iter().map(|(id, _)| *id).collect();
        self.savings.set_containers(containers);
        self.savings.set_excess(excess);
        Ok(())
    }

    fn open_allocate(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container) = (row.goal_id, row.name.clone(), row.container.id());
        // The pot `/N` divides. A container with nothing unallocated is not an
        // error -- the form still opens, and `/N` there is zero.
        let unallocated = self
            .savings
            .excess()
            .iter()
            .find(|(id, _)| *id == container)
            .map(|(_, cents)| *cents)
            .unwrap_or(Cents::ZERO);
        self.modal = Some(Modal::Allocation(AllocationForm::new(
            goal_id,
            &name,
            self.savings.account_name(container),
            unallocated,
            self.today,
        )));
        Ok(())
    }

    pub(super) fn commit_allocation(&mut self) -> Result<()> {
        let Some(Modal::Allocation(form)) = &self.modal else {
            return Ok(());
        };
        let allocated = form.commit()?;
        goal::insert_allocation(
            &self.db,
            form.goal_id,
            allocated.date,
            allocated.cents,
            allocated.note.as_deref(),
            None,
        )?;
        self.status = format!(
            "allocated {} · U undoes the last batch, not this",
            crate::demo::figure(allocated.cents)
        );
        self.close_modal();
        self.reload()
    }

    /// `n`: a goal typed from scratch, in the container the screen defaults
    /// to. The container is checked here rather than only at commit, so the
    /// form never opens over a container it invented.
    fn open_new_goal(&mut self) -> Result<()> {
        let Some(container) = self.savings.default_container() else {
            self.status = "no container holds goals yet".to_string();
            return Ok(());
        };
        let account = account::get(&self.db, container)?;
        self.modal = Some(Modal::Goal(GoalForm::add(
            Account::named(std::slice::from_ref(&account), container),
            setting::get(&self.db, key::TAX_RATE)?,
            self.today,
        )));
        Ok(())
    }

    fn open_goal_edit(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        self.modal = Some(Modal::Goal(GoalForm::new(
            row.goal_id,
            &row.name,
            row.base,
            row.goal_date,
            row.interest_eligible,
            row.taxed,
            row.floating,
            setting::get(&self.db, key::TAX_RATE)?,
            self.today,
        )));
        Ok(())
    }

    /// `e` and `n` share a form, so they share a commit: an id means the goal
    /// exists and is being edited, and none means it is being created in the
    /// container the screen defaults to.
    pub(super) fn commit_goal(&mut self) -> Result<()> {
        let Some(Modal::Goal(form)) = &self.modal else {
            return Ok(());
        };
        let target = form.target();
        let edit = form.commit()?;
        match target {
            GoalTarget::Update(id) => {
                goal::update(&self.db, id, &edit)?;
                self.status = format!("updated {}", crate::demo::text(&edit.name));
            }
            GoalTarget::Create(container) => {
                goal::insert(
                    &self.db,
                    &goal::NewGoal {
                        name: edit.name.clone(),
                        container_account_id: container,
                        base_cents: edit.base_cents,
                        goal_date: edit.goal_date,
                        // A free-form goal answers to no recurring entry.
                        recurring_goal_id: None,
                        interest_eligible: edit.interest_eligible,
                        sort: goal::next_sort(&self.db, container)?,
                        taxed: edit.taxed,
                        floating: edit.floating,
                    },
                )?;
                self.status = format!("created {}", crate::demo::text(&edit.name));
            }
        }
        self.close_modal();
        self.reload()
    }

    /// Move the selected undated goal one place in its container's manual
    /// order, and put the cursor back on it.
    ///
    /// Two refusals rather than two silences. A dated goal takes its place
    /// from its date, so there is no manual order for it to move in; and a
    /// kept search hides part of the block being reordered, so a move would
    /// be one place in a list the owner cannot see. Either one says so,
    /// because a key that sometimes quietly does nothing is a key nobody
    /// trusts.
    ///
    /// The position is computed against the container's undated goals rather
    /// than against the rows on screen: `goal::reorder` renumbers that block,
    /// and the two must be counting the same list. `reload_savings` then
    /// redraws from the table, and the cursor is put back **by id** -- the
    /// rows moved under it, so an index would leave the selection on whatever
    /// took the vacated place and the next press would move that goal
    /// instead.
    fn move_goal(&mut self, direction: Move) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (id, container, name, dated) = (
            row.goal_id,
            row.container.id(),
            row.name.clone(),
            row.goal_date.is_some(),
        );
        if !self.savings.search().is_empty() {
            self.status = "Clear the search before reordering goals".to_string();
            return Ok(());
        }
        if dated {
            self.status = format!(
                "{} has a goal date, so its place comes from that date",
                crate::demo::text(&name)
            );
            return Ok(());
        }
        let undated: Vec<GoalId> = goal::list(&self.db, container)?
            .into_iter()
            .filter(|g| g.goal_date.is_none())
            .map(|g| g.id)
            .collect();
        let from = undated
            .iter()
            .position(|g| *g == id)
            .context("the selected goal is open and undated, so its container lists it")?;
        let Some(to) = direction.applied(from, undated.len()) else {
            return Ok(());
        };
        goal::reorder(&self.db, id, to)?;
        self.reload_savings()?;
        self.savings.select_goal(id);
        Ok(())
    }

    /// Mark or unmark the selected goal, and redraw the screen from the
    /// database.
    ///
    /// `reload_savings` rather than a write to the row in hand: the row is a
    /// copy of what the query returned, and one write that updated the copy
    /// instead of re-reading is how a screen starts disagreeing with the
    /// table under it. It is also cheap here -- every goal is already loaded
    /// for the reconciliation line -- and it keeps the cursor, which is an
    /// index into rows this does not reorder.
    fn toggle_favorite(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (id, favorite) = (row.goal_id, row.favorite);
        goal::set_favorite(&self.db, id, !favorite)?;
        self.reload_savings()
    }

    /// `t`: part of the selected goal's value into another goal of the same
    /// container.
    ///
    /// The siblings come from the container rather than from the screen's
    /// filtered rows, the reason `open_close_out` gives: a search must not
    /// narrow where value may land. A container holding nothing else open is
    /// refused here rather than opening a form over an empty selector --
    /// returning value to unallocated is `a` with a negative amount, and
    /// ending the goal is `c`.
    fn open_goal_transfer(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container, current) = (
            row.goal_id,
            row.name.clone(),
            row.container.id(),
            row.current,
        );
        let siblings: Vec<(GoalId, String)> = goal::list_with_balances(&self.db, container)?
            .into_iter()
            .filter(|g| g.goal.id != goal_id)
            .map(|g| (g.goal.id, g.goal.name))
            .collect();
        if siblings.is_empty() {
            self.status = "this container has no other open goal to move value to".to_string();
            return Ok(());
        }
        self.modal = Some(Modal::GoalTransfer(GoalTransferForm::new(
            goal_id, &name, current, siblings, self.today,
        )));
        Ok(())
    }

    /// Unlike every other write on this screen, `U` really does undo this
    /// one: the two rows are one batch, so the status line says so rather
    /// than saying what `U` will not reach.
    pub(super) fn commit_goal_transfer(&mut self) -> Result<()> {
        let Some(Modal::GoalTransfer(form)) = &self.modal else {
            return Ok(());
        };
        let moved = form.commit()?;
        goal::transfer_value(&self.db, form.goal_id, moved.to, moved.cents, moved.date)?;
        self.status = format!("moved {} · U undoes it", crate::demo::figure(moved.cents));
        self.close_modal();
        self.reload()
    }

    fn open_close_out(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container, current) = (
            row.goal_id,
            row.name.clone(),
            row.container.id(),
            row.current,
        );
        // Built from the container, not from the screen's filtered rows: a
        // search must not narrow what a close-out may move value into.
        let siblings = goal::list_with_balances(&self.db, container)?
            .into_iter()
            .filter(|g| g.goal.id != goal_id)
            .map(|g| (g.goal.id, g.goal.name))
            .collect();
        self.modal = Some(Modal::CloseOut(CloseForm::new(
            goal_id, &name, current, siblings, self.today,
        )));
        Ok(())
    }

    pub(super) fn commit_close_out(&mut self) -> Result<()> {
        let Some(Modal::CloseOut(form)) = &self.modal else {
            return Ok(());
        };
        let ending = form.commit()?;
        goal::move_value(&self.db, form.goal_id, ending.to, ending.date)?;
        self.status = match ending.to {
            None => "closed, value returned to unallocated · U undoes the last batch, not this"
                .to_string(),
            Some(_) => "closed, value moved · U undoes the last batch, not this".to_string(),
        };
        self.close_modal();
        self.reload()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Group, Kind};
    use crate::db::goal;
    use crate::money::Cents;
    use crate::rate::Percent;
    use crate::test_support::{day, walk_until};
    use crate::tui::app::Screen;
    use crate::tui::app::test_support::*;
    use crate::tui::goal_form;
    use crate::tui::goal_form::GoalTarget;
    use crate::tui::modal::Modal;
    use crate::tui::planning::Target;
    use crate::tui::search::Search;
    use ratatui::crossterm::event::KeyCode;

    /// `PAY_PERIODS_PER_YEAR` is the only thing `$/Pay` divides a runway by,
    /// it is editable on the Planning screen, and that commit reloads this
    /// one -- so the column follows the setting rather than the cadence the
    /// app opened with. The other half of the same read is the Recurring
    /// Goals title, which
    /// `editing_the_pay_period_count_moves_the_recurring_goals_title` pins.
    #[test]
    fn editing_the_pay_period_count_moves_the_savings_per_paycheck_column() {
        let mut app = app();
        let ask = |app: &App| {
            app.savings
                .rows()
                .iter()
                .find(|r| r.name == "Vacation 2027")
                .expect("the fixture's one dated goal")
                .per_paycheck
        };
        // $5,000 short, 139 days of runway: ten fortnights, five months.
        assert_eq!(ask(&app), Some(Cents::from_dollars(500)));

        Target::PeriodsPerYear
            .write(&app.db, today(), "12")
            .unwrap();
        app.reload().unwrap();
        assert_eq!(ask(&app), Some(Cents::from_dollars(1_000)));
    }

    /// One container holding three undated goals and one dated, for the
    /// manual order `K` and `J` move things around in.
    fn app_with_undated_goals() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        for (i, (name, date)) in [
            ("Couch", None),
            ("Bike", None),
            ("Camera", None),
            ("Vacation 2027", Some(day(2027, 1, 1))),
        ]
        .into_iter()
        .enumerate()
        {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: savings,
                    base_cents: Cents(100_000),
                    goal_date: date,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort: i as i64,
                    taxed: false,
                    floating: false,
                },
            )
            .unwrap();
        }
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));
        app
    }

    /// The whole point of the manual order: the owner arranges the goals no
    /// deadline arranges for them.
    #[test]
    fn k_moves_the_selected_undated_goal_up() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.savings.selected().unwrap().name, "Camera");

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Camera", "Bike", "Vacation 2027"]
        );
    }

    /// The rows move under the cursor, so the cursor has to be put back on
    /// the goal by id -- an index kept across the move would leave the
    /// selection on whichever goal took the vacated place, and a second press
    /// would move that one instead.
    #[test]
    fn the_cursor_follows_the_goal_it_moved() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.savings.selected().unwrap().name, "Camera");
        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Camera", "Couch", "Bike", "Vacation 2027"],
            "already first, so the second press had nowhere to go"
        );
        assert_eq!(app.savings.selected().unwrap().name, "Camera");
    }

    #[test]
    fn j_moves_the_selected_undated_goal_down() {
        let mut app = app_with_undated_goals();
        assert_eq!(app.savings.selected().unwrap().name, "Couch");

        press(&mut app, KeyCode::Char('J'));

        assert_eq!(
            savings_names(&app),
            vec!["Bike", "Couch", "Camera", "Vacation 2027"]
        );
        assert_eq!(app.savings.selected().unwrap().name, "Couch");
    }

    #[test]
    fn moving_the_last_undated_goal_down_changes_nothing() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('J'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Bike", "Camera", "Vacation 2027"],
            "the dated block below is not somewhere an undated goal can move into"
        );
    }

    /// A dated goal's place comes from its date, so the key says so rather
    /// than doing nothing and leaving the owner to wonder which it was.
    #[test]
    fn moving_a_dated_goal_is_refused_with_a_message() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::End);
        assert_eq!(app.savings.selected().unwrap().name, "Vacation 2027");

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(
            savings_names(&app),
            vec!["Couch", "Bike", "Camera", "Vacation 2027"]
        );
        assert!(
            app.status.contains("goal date"),
            "said nothing about why: {}",
            app.status
        );
    }

    /// A kept search hides part of the block being reordered, so a move
    /// would be one place in a list the owner cannot see. Refused rather
    /// than guessed at.
    #[test]
    fn moving_while_a_kept_search_narrows_the_list_is_refused() {
        let mut app = app_with_undated_goals();
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "e");
        press(&mut app, KeyCode::Enter);
        assert_eq!(savings_names(&app), vec!["Bike", "Camera"]);
        press(&mut app, KeyCode::Down);

        press(&mut app, KeyCode::Char('K'));

        assert_eq!(savings_names(&app), vec!["Bike", "Camera"]);
        assert!(
            app.status.contains("search"),
            "said nothing about the search: {}",
            app.status
        );
    }

    /// `f` is a toggle over the selected row, written straight through: there
    /// is nothing to confirm and nothing to type, so a modal would be a
    /// keystroke asking whether the owner meant the keystroke.
    #[test]
    fn f_marks_the_selected_goal_and_pressing_it_again_takes_the_mark_back() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(savings_favorites(&app), vec![false, false]);

        press(&mut app, KeyCode::Char('f'));
        assert_eq!(savings_favorites(&app), vec![true, false]);

        press(&mut app, KeyCode::Char('f'));
        assert_eq!(savings_favorites(&app), vec![false, false]);
    }

    /// The write has to reach the database, not just the row: a mark that
    /// lived on the view would be gone at the next reload and the owner would
    /// not find out until the next launch.
    #[test]
    fn a_mark_survives_a_reload() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('f'));

        app.reload().unwrap();

        assert_eq!(savings_favorites(&app), vec![true, false]);
    }

    /// The mark is a highlight, so it must not move the cursor off the row it
    /// was pressed on -- pressing `f` twice has to be the same row twice.
    #[test]
    fn marking_a_goal_leaves_the_cursor_where_it_was() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.selected().unwrap().goal_id;

        press(&mut app, KeyCode::Char('f'));

        assert_eq!(app.savings.selected().unwrap().goal_id, before);
        assert_eq!(savings_favorites(&app), vec![false, true]);
    }

    /// Every other row key on this screen says so rather than doing nothing,
    /// and an empty list is the state a fresh database opens in.
    #[test]
    fn f_with_nothing_selected_says_so() {
        let db = db::open_in_memory().unwrap();
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));

        press(&mut app, KeyCode::Char('f'));

        assert!(!app.status.is_empty(), "{:?}", app.status);
    }

    /// One form, two jobs, so the border is the only thing on screen that says
    /// which one is happening.
    #[test]
    fn the_goal_modal_names_the_job_it_is_doing() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        assert!(drawn(&mut app).contains("New goal"));

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('e'));
        assert!(drawn(&mut app).contains("Edit goal"));
    }

    /// A goal's container is fixed at creation, and under the `Tab` filter's
    /// All the screen's own title says only "Savings · All" -- so without this
    /// the border is silent about which container `n` is about to create in,
    /// the way `Picker` and `Worksheet` never are about theirs.
    #[test]
    fn the_new_goal_modal_names_the_container_it_will_create_in() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert!(
            drawn(&mut app).contains("Savings · All"),
            "the fixture must open on All for this to be worth asserting"
        );

        press(&mut app, KeyCode::Char('n'));

        assert!(drawn(&mut app).contains("New goal in Rainy Day"));
    }

    /// The Savings container filter cycles both ways too, so `BackTab` backs
    /// out of a container `Tab` stepped into.
    #[test]
    fn back_tab_steps_the_savings_container_filter_the_other_way() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Tab);
        let container = app.savings.selected_container();
        assert!(container.is_some());

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.savings.selected_container(), None);

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.savings.selected_container(), container);
    }

    #[test]
    fn four_opens_the_savings_screen_with_every_goal_on_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        assert_eq!(app.screen, Screen::Savings);
        let names: Vec<&str> = app.savings.rows().iter().map(|r| r.name.as_str()).collect();
        // Undated first: Couch has no date, Vacation 2027 does.
        assert_eq!(names, ["Couch", "Vacation 2027"]);
    }

    /// The reconciliation line is the Savings screen's alone.
    #[test]
    fn the_savings_screen_carries_each_containers_unallocated_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        let excess = app.savings.excess();
        assert_eq!(excess.len(), 1, "only Rainy Day holds goals");
        // Rainy Day holds one 200.00 transfer against 10,250.00 allocated.
        assert_eq!(excess[0].1, Cents(20_000) - Cents(1_025_000));
    }

    #[test]
    fn q_while_searching_the_savings_screen_types_into_the_box() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "q");

        assert!(!app.should_quit());
        assert_eq!(app.savings.search(), "q");
        assert!(app.savings.rows().is_empty());
    }

    /// The written allocation must be on screen without another keystroke,
    /// and the container's unallocated remainder must move with it.
    #[test]
    fn a_on_the_savings_screen_writes_an_allocation_against_the_selected_goal() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Down once: the undated Couch heads the list, and this is the goal
        // whose balance the figures below are about.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.savings.selected().unwrap().name, "Vacation 2027");
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "454");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[1].current, Cents(1_045_400));
        assert_eq!(app.savings.excess()[0].1, before - Cents(45_400));
    }

    /// `/N` on the amount is a fraction of the container's unallocated
    /// remainder -- what the Savings footer reports, and the same arithmetic
    /// the worksheet's `/N` does.
    #[test]
    fn a_share_typed_on_the_savings_form_books_a_fraction_of_the_remainder() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        // The fixture's goals hold more than Rainy Day does, so top it up to a
        // remainder with cents in it: the division has to floor them away.
        write(&app.db, savings, day(2026, 8, 15), 1_255_001, "Interest");
        app.reload().unwrap();
        assert_eq!(app.savings.excess()[0].1, Cents(250_001));
        let before = app.savings.rows()[0].current;

        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "/2");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.savings.rows()[0].current, before + Cents(125_000));
        assert_eq!(app.savings.excess()[0].1, Cents(125_001));
    }

    /// A divisor is not a figure, so the form resolves it on screen. Without
    /// this the owner commits to find out what they typed.
    #[test]
    fn the_allocation_form_shows_what_a_share_comes_to_before_it_is_committed() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        write(&app.db, savings, day(2026, 8, 15), 1_255_000, "Interest");
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "/12");

        let screen = drawn(&mut app);
        assert!(screen.contains("= 208"), "{screen}");
    }

    /// The pot is behind the modal and the key has no room in the help table
    /// `Topic::Form` shares with the forms that do not offer it, so the form
    /// says both itself.
    #[test]
    fn the_allocation_form_names_the_remainder_a_share_would_divide() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        let savings = app.savings.excess()[0].0;
        write(&app.db, savings, day(2026, 8, 15), 1_255_000, "Interest");
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('a'));

        let screen = drawn(&mut app);
        assert!(
            screen.contains("Rainy Day unallocated 2,500.00 · /N takes 1/N"),
            "{screen}"
        );
    }

    /// A bad divisor reports itself where every other unparseable field does,
    /// and the form stays open on what was typed.
    #[test]
    fn a_share_divided_by_zero_reports_itself_and_keeps_the_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "/0");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the form must stay open");
        assert!(app.status.contains("divide by 0"), "{}", app.status);
    }

    #[test]
    fn a_on_the_savings_screen_with_nothing_selected_says_so() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "zzz");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('a'));

        assert!(app.modal.is_none());
        assert_eq!(app.status, "nothing selected");
    }

    /// `%` and `$/Pay` are derived from the target, so an edit has to move
    /// them without another keystroke.
    #[test]
    fn e_on_the_savings_screen_rewrites_the_goal_and_its_derived_columns() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Down once: Vacation 2027 is the funded goal, so a rewritten target
        // moves a percentage rather than leaving it where it was.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab);
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "20000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let row = &app.savings.rows()[1];
        assert_eq!(row.goal, Cents(2_000_000));
        assert_eq!(row.percent, Some(Percent(50)));
    }

    /// Abandoning returns the value to unallocated, so the goal leaves the
    /// list and the container's remainder rises by exactly its balance.
    #[test]
    fn c_then_enter_abandons_the_selected_goal() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let names: Vec<&str> = app.savings.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Couch"]);
        assert_eq!(app.savings.excess()[0].1, before + Cents(1_000_000));
    }

    /// A close-out into another goal moves value inside one container, so the
    /// reconciliation must not move.
    #[test]
    fn c_into_another_goal_moves_the_balance_and_leaves_the_remainder_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.excess()[0].1;

        // The form opens focused on `To`, so the sibling is one `→` away.
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        let rows = app.savings.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Couch");
        assert_eq!(rows[0].current, Cents(1_025_000));
        assert_eq!(app.savings.excess()[0].1, before);
    }

    /// A transfer moves part of one goal into another inside one container,
    /// so both goals survive it and the reconciliation must not move. The
    /// form opens on the amount with the container's one other goal already
    /// selected, which is what keeps it to `t`, a figure and `Enter`.
    #[test]
    fn t_moves_part_of_the_selected_goals_value_into_another_goal() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        let before = app.savings.excess()[0].1;

        press(&mut app, KeyCode::Char('t'));
        type_str(&mut app, "250");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        let rows = app.savings.rows();
        assert_eq!(rows[0].name, "Couch");
        assert_eq!(rows[0].current, Cents(50_000));
        assert_eq!(rows[1].name, "Vacation 2027");
        assert_eq!(rows[1].current, Cents(975_000));
        assert_eq!(app.savings.excess()[0].1, before);
    }

    /// The form draws what it is about to do: which goal the value is
    /// leaving, the three fields, and the goal it would land in.
    #[test]
    fn t_draws_the_transfer_form_over_the_screen() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('t'));

        let drawn = drawn(&mut app);
        assert!(drawn.contains("Vacation 2027"), "{drawn}");
        assert!(drawn.contains("Amount"), "{drawn}");
        assert!(drawn.contains("To"), "{drawn}");
        assert!(drawn.contains("Couch"), "{drawn}");
    }

    /// The pair is one batch, so a fumbled amount is one keystroke back
    /// rather than two hand-corrections in two histories. A close-out is
    /// deliberately no batch; a transfer closes nothing, so nothing stands in
    /// the way of undoing it whole.
    #[test]
    fn a_goal_transfer_is_one_batch_so_capital_u_reverses_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('t'));
        type_str(&mut app, "250");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('U'));
        press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none(), "{}", app.status);
        assert_eq!(app.savings.rows()[0].current, Cents(25_000));
        assert_eq!(app.savings.rows()[1].current, Cents(1_000_000));
    }

    /// There is nowhere for the value to go, so the form does not open over a
    /// selector with nothing in it. Returning value to unallocated is `a`
    /// with a negative amount, and ending the goal is `c`.
    #[test]
    fn t_refuses_a_container_whose_only_open_goal_is_the_selected_one() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.savings.rows().len(),
            1,
            "one goal left in the container"
        );

        press(&mut app, KeyCode::Char('t'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("no other open goal"), "{}", app.status);
    }

    /// The guards above are what keep a taxed goal with no rate out of the
    /// database, but a database that already holds one -- hand-edited, or
    /// migrated from a build that had no flag -- still has to open. A strict
    /// read in `reload_savings` runs inside `App::new`, so the refusal would
    /// not blank a screen, it would stop the application starting, and the
    /// rate is set from inside the application.
    #[test]
    fn the_app_starts_on_a_taxed_goal_with_no_rate_and_draws_it_against_its_base() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        goal::insert(
            &db,
            &goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents(100_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
                floating: false,
            },
        )
        .unwrap();

        let app = App::new(db, today()).unwrap();
        let row = app
            .savings
            .rows()
            .into_iter()
            .find(|r| r.name == "Couch")
            .expect("the goal is drawn rather than dropped");
        assert_eq!(row.goal, Cents(100_000), "the base, for want of a rate");
    }

    /// The keys reach the Savings screen. `app()` holds one dated goal
    /// (January 2027) and one undated one, so the month filter is also what
    /// hides the undated goal.
    #[test]
    fn the_bracket_keys_and_esc_filter_savings_by_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);

        press(&mut app, KeyCode::Char(']'));
        assert_eq!(
            savings_names(&app),
            ["Vacation 2027"],
            "the undated Couch belongs to no month"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// The screen narrows two ways, and `Esc` is the way out of both: an
    /// owner who has tabbed to a container and stepped to a month should not
    /// have to remember which of the two is hiding the goal they are looking
    /// for.
    #[test]
    fn esc_on_savings_clears_the_container_filter_as_well_as_the_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(']'));
        assert!(app.savings.selected_container().is_some());
        assert!(app.savings.selected_month().is_some());

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_container(), None);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// `Esc` inside the search box still clears the search, which is the
    /// handler the month filter must not have taken over.
    #[test]
    fn esc_in_the_savings_search_box_clears_the_search_not_the_month() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "vac");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.savings.search(), "");
        assert!(app.savings.selected_month().is_some());
    }

    /// `Enter` leaves the box and keeps the filter, so `Esc` outside the box
    /// is what clears it -- and the needle goes before the container and the
    /// month, which `clear_filters` then takes together.
    #[test]
    fn esc_outside_the_savings_box_clears_a_kept_search_before_the_other_filters() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "vac");
        press(&mut app, KeyCode::Enter);
        assert!(!app.savings.is_searching(), "Enter left the box");
        assert_eq!(app.savings.search(), "vac");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.search(), "");
        assert!(
            app.savings.selected_container().is_some(),
            "the container is the next thing out, not this one"
        );
        assert!(
            app.savings.selected_month().is_some(),
            "and so is the month"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.savings.selected_container(), None);
        assert_eq!(app.savings.selected_month(), None);
        assert_eq!(app.savings.rows().len(), 2);
    }

    /// The goal form the app has open, for a test that has to walk its
    /// fields through the app's own keys.
    fn app_goal_form(app: &App) -> &goal_form::GoalForm {
        match &app.modal {
            Some(Modal::Goal(form)) => form,
            _ => panic!("no goal form is open"),
        }
    }

    /// `n` is a free-form goal: a name, a target and a date, in the container
    /// the `Tab` filter names. Creating goals *from* recurring goal entries is
    /// `s` on screen 7, over on the table those entries live in.
    #[test]
    fn n_on_savings_opens_a_blank_goal_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));

        let Some(Modal::Goal(form)) = &app.modal else {
            panic!("no goal form is open");
        };
        assert_eq!(
            form.target(),
            GoalTarget::Create(app.savings.default_container().unwrap()),
            "a new goal has no id to edit, and lands in the default container"
        );
        assert_eq!(form.display(goal_form::GoalField::Name).plain_text(), "");
    }

    #[test]
    fn committing_a_blank_goal_form_creates_the_goal_in_the_tab_container() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        type_str(&mut app, "Bike");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Tab);
        // The date opens prefilled, so a typed one replaces it the way it
        // replaces the prefilled date on every other form.
        for _ in 0.."2026-09-01".len() {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "2027-05-01");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open");
        let rows = app.savings.rows();
        let bike = rows
            .iter()
            .find(|r| r.name == "Bike")
            .expect("the new goal is not on the screen");
        assert_eq!(bike.goal, Cents::from_dollars(1_200));
        assert_eq!(bike.goal_date, Some(day(2027, 5, 1)));
        assert_eq!(
            bike.container.id(),
            app.savings.default_container().unwrap(),
            "the new goal landed outside the container the screen defaults to"
        );
    }

    /// `n` and `e` share a form, so they must share every field of it. The
    /// flag is the one that decides what the goal's target *is*, so a create
    /// path dropping it would make a floating goal creatable only by making
    /// one and then editing it.
    #[test]
    fn a_goal_created_floating_arrives_floating_and_reads_full() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        type_str(&mut app, "Brokerage");
        walk_until!(
            app_goal_form(&app).focus == goal_form::GoalField::Floating,
            press(&mut app, KeyCode::Tab)
        );
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open");
        let rows = app.savings.rows();
        let row = rows
            .iter()
            .find(|r| r.name == "Brokerage")
            .expect("the new goal is not on the screen");
        assert!(row.floating);
        assert_eq!(row.percent, Some(Percent(100)));
        assert_eq!(row.goal, Cents::ZERO, "a goal nothing has been put in yet");
    }

    /// The same guard `A` and `i` have: with no container there is nowhere for
    /// a goal to go, and the form must not open over a container it invented.
    #[test]
    fn n_on_savings_with_no_container_says_so() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));

        assert!(app.modal.is_none());
        assert_eq!(app.status, "no container holds goals yet");
    }
}
