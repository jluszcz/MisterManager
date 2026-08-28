//! One goal's allocation history: the modal `Enter` opens on a Savings row,
//! and the two writes that correct a row in it.
//!
//! Three modes over one `Option<Modal>` -- the list, the editor, the
//! confirmation -- so `Esc` peels one layer at a time and nothing in the app
//! becomes the first thing to open a modal over a modal. The dispatch between
//! them is `modal_key`'s two guards; what each one does is here.

use super::App;
use crate::db::goal;
use crate::money::Cents;
use crate::tui::cursor;
use crate::tui::goal_form::{AllocTarget, AllocationForm};
use crate::tui::history::{History, Mode};
use crate::tui::modal::{Confirm, Modal};
use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// `Enter` on a Savings row: the long form of the balance cell that row
    /// already carries.
    pub(super) fn open_history(&mut self) -> Result<()> {
        let Some(row) = self.savings.selected() else {
            return self.nothing_selected();
        };
        let (goal_id, name, container) = (row.goal_id, row.name.clone(), row.container.id());
        let container_name = self.savings.account_name(container).to_string();
        let rows = goal::allocations(&self.db, goal_id)?;
        self.modal = Some(Modal::History(History::new(
            goal_id,
            &name,
            container,
            &container_name,
            rows,
        )));
        Ok(())
    }

    /// The list mode's keys: the shared scroll keys, `e`, `d` and `Esc`, and
    /// nothing else.
    ///
    /// `Enter` is deliberately unbound. It commits in the editing mode a
    /// keystroke away, and one key that opens an editor in one mode and
    /// commits it in the next is the reflex-breaking case the keyboard rules
    /// exist to prevent.
    pub(super) fn history_key(&mut self, key: KeyEvent) -> Result<()> {
        // Scoped, so the borrow the scroll keys need is over before `Esc`
        // reaches the modal the list is inside.
        {
            let Some(Modal::History(history)) = &mut self.modal else {
                return Ok(());
            };
            if cursor::scroll_key(history, key.code) {
                return Ok(());
            }
        }
        match key.code {
            KeyCode::Esc => self.close_modal(),
            KeyCode::Char('e') => return self.begin_history_edit(),
            KeyCode::Char('d') => return self.begin_history_delete(),
            _ => {}
        }
        Ok(())
    }

    /// `e`: the row under the cursor, in the form `a` on Savings already
    /// writes with.
    fn begin_history_edit(&mut self) -> Result<()> {
        let Some(Modal::History(history)) = &self.modal else {
            return Ok(());
        };
        let Some(row) = history.selected().cloned() else {
            return self.nothing_selected();
        };
        let (goal_name, container, container_name) = (
            history.goal_name().to_string(),
            history.container(),
            history.container_name().to_string(),
        );
        // The pot `/N` divides, snapshotted as it stands now -- the same
        // figure and the same rule as `a`. The row's own current amount is
        // **not** folded back into it, so `/2` while editing means half of
        // what is unallocated right now rather than half of a pot that
        // includes the row being edited.
        let unallocated = self
            .savings
            .excess()
            .iter()
            .find(|(id, _)| *id == container)
            .map(|(_, cents)| *cents)
            .unwrap_or(Cents::ZERO);
        let form = AllocationForm::edit(&row, &goal_name, &container_name, unallocated, self.today);
        self.set_history_mode(Mode::Editing(form));
        Ok(())
    }

    /// `d`: the same question every other delete asks, in the same words --
    /// `Confirm` owns them, so what `y` writes, what the border asks and how
    /// a cancel reads stay one exhaustive match per confirmable write.
    fn begin_history_delete(&mut self) -> Result<()> {
        let confirming = match &self.modal {
            Some(Modal::History(history)) => history.selected().map(|row| Mode::Confirming {
                action: Confirm::DeleteAllocation(row.id),
                label: history.label(row),
            }),
            _ => return Ok(()),
        };
        let Some(mode) = confirming else {
            return self.nothing_selected();
        };
        self.set_history_mode(mode);
        Ok(())
    }

    /// The confirming mode's keys, and deliberately not [`App::confirm_key`]:
    /// a commit or a cancel here returns to the list rather than closing the
    /// modal, because this dialog is a *mode* of the history rather than a
    /// modal of its own.
    ///
    /// The write runs while the question is still on screen, for the reason
    /// the top-level dialog runs its own there: a refusal leaves the question
    /// up with the reason under it.
    pub(super) fn history_confirm_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(Modal::History(history)) = &self.modal else {
            return Ok(());
        };
        let Mode::Confirming { action, .. } = history.mode() else {
            return Ok(());
        };
        let action = *action;
        if key.code != KeyCode::Char('y') {
            self.set_history_mode(Mode::List);
            self.status = action.cancelled().to_string();
            return Ok(());
        }
        let status = action.commit(&self.db)?;
        self.set_history_mode(Mode::List);
        self.status = status;
        self.reload()?;
        self.reload_history()
    }

    /// The editing mode's keys, which are `form_key`'s -- the caret keys, the
    /// `Ctrl` editing keys and the date steppers all work with no new code --
    /// except for `Esc`.
    ///
    /// `Esc` peels one layer rather than closing the modal: the editor is a
    /// mode of the history, so backing out of it leaves the list it was opened
    /// from on screen. Gated on the popup for the reason `form_key` gates its
    /// own `Esc` -- an allocation form raises no suggestions today, and a
    /// dismissable popup must still get the first `Esc` if one ever does.
    pub(super) fn history_edit_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc && self.popup.visible() == 0 {
            self.set_history_mode(Mode::List);
            return Ok(());
        }
        self.form_key(key, App::commit_history_edit)
    }

    /// `Enter` in the editing mode.
    ///
    /// Every write here reloads the whole app, as `commit_allocation` does: a
    /// goal's balance feeds its `%`, its `$/Pay`, its container's unallocated
    /// excess, and every Planning gate measured against a shortfall.
    pub(super) fn commit_history_edit(&mut self) -> Result<()> {
        let Some(Modal::History(history)) = &self.modal else {
            return Ok(());
        };
        let Mode::Editing(form) = history.mode() else {
            return Ok(());
        };
        let target = form.target();
        let edit = form.commit()?;
        match target {
            AllocTarget::Update(id) => goal::update_allocation(&self.db, id, &edit)?,
            // The history only ever builds `AllocationForm::edit`, which is an
            // `Update`. An `Insert` reaching here would be a form this modal
            // did not open, and writing a second row for it is the one answer
            // that is certainly wrong.
            AllocTarget::Insert => bail!("the history's editor is not open on an allocation"),
        }
        self.set_history_mode(Mode::List);
        self.status = format!("allocation saved · {}", crate::demo::figure(edit.cents));
        self.reload()?;
        self.reload_history()
    }

    /// The history's own rows again, for the modal still open over the screen
    /// [`App::reload`] has just rebuilt. A no-op when the history is closed,
    /// so a write elsewhere can call it unconditionally.
    pub(super) fn reload_history(&mut self) -> Result<()> {
        let Some(Modal::History(history)) = &self.modal else {
            return Ok(());
        };
        let rows = goal::allocations(&self.db, history.goal_id())?;
        if let Some(Modal::History(history)) = &mut self.modal {
            history.set_rows(rows);
        }
        Ok(())
    }

    fn set_history_mode(&mut self, mode: Mode) {
        if let Some(Modal::History(history)) = &mut self.modal {
            history.set_mode(mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::money::Cents;
    use crate::test_support::{day, walk_until};
    use crate::tui::app::Screen;
    use crate::tui::app::test_support::*;
    use crate::tui::goal_form::AllocField;
    use crate::tui::help::Topic;
    use ratatui::crossterm::event::KeyCode;

    /// The Savings screen with the cursor on `Vacation 2027`, whose one
    /// allocation is the row every test below corrects.
    /// The editor the history has open, as the test that pressed `e` expects
    /// it to be.
    fn editing(app: &App) -> &AllocationForm {
        match history(app).mode() {
            Mode::Editing(form) => form,
            _ => panic!(
                "the history is not editing, with {:?} on the status line",
                app.status
            ),
        }
    }

    fn on_vacation() -> App {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        // Down once: the undated Couch heads the list.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.savings.selected().unwrap().name, "Vacation 2027");
        app
    }

    #[test]
    fn enter_opens_the_history_of_the_selected_goal() {
        let mut app = on_vacation();
        let goal_id = app.savings.selected().unwrap().goal_id;

        press(&mut app, KeyCode::Enter);

        let history = history(&app);
        assert_eq!(history.goal_id(), goal_id);
        assert_eq!(history.goal_name(), "Vacation 2027");
        assert_eq!(history.rows().len(), 1);
        assert_eq!(history.total(), Cents(1_000_000));
    }

    /// The border, the total and the one row are all on screen without
    /// another keystroke.
    #[test]
    fn the_history_draws_its_goals_rows_and_what_they_come_to() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);

        let screen = drawn(&mut app);
        assert!(screen.contains("Vacation 2027"), "{screen}");
        assert!(screen.contains("2026-08-01"), "{screen}");
        assert!(screen.contains("Total 10,000.00"), "{screen}");
    }

    /// Every other row key on this screen says so rather than doing nothing,
    /// and an empty list is the state a fresh database opens in.
    #[test]
    fn enter_with_nothing_selected_says_so() {
        let db = db::open_in_memory().unwrap();
        let mut app = App::new(db, today()).unwrap();
        press(&mut app, KeyCode::Char('4'));

        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.status, "nothing selected");
    }

    /// A goal reached before it was ever funded is the case the placeholder
    /// exists for, and it is still a modal that opens.
    #[test]
    fn a_goal_with_no_allocations_still_opens_a_history() {
        let mut app = app();
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        type_str(&mut app, "Skis");
        walk_until!(
            matches!(app.modal, Some(Modal::Goal(ref f)) if f.focus
                == crate::tui::goal_form::GoalField::Target),
            press(&mut app, KeyCode::Tab)
        );
        type_str(&mut app, "800");
        press(&mut app, KeyCode::Enter);
        walk_until!(
            app.savings.selected().is_some_and(|row| row.name == "Skis"),
            press(&mut app, KeyCode::Down)
        );

        press(&mut app, KeyCode::Enter);

        assert!(history(&app).rows().is_empty());
        assert!(drawn(&mut app).contains("no allocations yet"));

        // Both row keys say so rather than doing nothing, and the list stays
        // open behind the refusal.
        for key in ['e', 'd'] {
            press(&mut app, KeyCode::Char(key));
            assert!(matches!(history(&app).mode(), Mode::List), "{key}");
            assert_eq!(app.status, "nothing selected", "{key}");
        }
    }

    /// `e` is the editing mode and `Esc` is out of it again -- one layer at a
    /// time, with the list still open behind.
    #[test]
    fn e_enters_the_editing_mode_and_esc_leaves_it_with_the_list_still_open() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(history(&app).mode(), Mode::Editing(_)));
        assert!(drawn(&mut app).contains("Edit allocation to Vacation 2027"));
        // The footer follows the mode without the screen asking it to.
        assert_eq!(app.topic(), Topic::Form);

        press(&mut app, KeyCode::Esc);
        assert!(matches!(history(&app).mode(), Mode::List));
        assert_eq!(app.topic(), Topic::History);
    }

    /// The form opens on the row rather than on a blank: all three fields
    /// carry what the row holds, the note included when there is one.
    #[test]
    fn the_editor_opens_prefilled_from_the_row_under_the_cursor() {
        let mut app = on_vacation();
        let goal_id = app.savings.selected().unwrap().goal_id;
        goal::insert_allocation(
            &app.db,
            goal_id,
            day(2026, 8, 20),
            Cents(50_000),
            Some("top up"),
            None,
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        let form = editing(&app);
        assert_eq!(form.display(AllocField::Amount).plain_text(), "500.00");
        assert_eq!(form.display(AllocField::Note).plain_text(), "top up");
        assert!(
            form.display(AllocField::Date)
                .plain_text()
                .contains("2026-08-20"),
            "{}",
            form.display(AllocField::Date).plain_text()
        );
    }

    /// The whole point of the modal: a figure entered wrongly is rewritten in
    /// place rather than offset by a second row, and everything derived from
    /// the goal's balance follows it.
    #[test]
    fn a_committed_edit_moves_the_goals_balance_and_the_containers_remainder() {
        let mut app = on_vacation();
        let before = app.savings.excess()[0].1;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        // The editor opens on the amount: clear the prefill and retype it.
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "9000");
        press(&mut app, KeyCode::Enter);

        // Back to the list, one row still, and the figure rewritten.
        assert!(matches!(history(&app).mode(), Mode::List));
        assert_eq!(history(&app).rows().len(), 1);
        assert_eq!(history(&app).total(), Cents(900_000));
        assert_eq!(app.savings.rows()[1].current, Cents(900_000));
        assert_eq!(app.savings.excess()[0].1, before + Cents(100_000));
    }

    /// A row the import or an interest posting wrote carries cents, and it is
    /// exactly the kind of row this modal exists to correct -- so the editor
    /// has to be able to save what it just prefilled.
    #[test]
    fn an_amount_with_cents_in_it_saves_where_a_new_allocation_would_be_refused() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "9000.55");

        press(&mut app, KeyCode::Enter);

        assert!(matches!(history(&app).mode(), Mode::List), "{}", app.status);
        assert_eq!(history(&app).total(), Cents(900_055));
    }

    /// The date is editable too, and the row lands where the new date puts it.
    #[test]
    fn a_committed_edit_can_move_the_row_to_another_date() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        // Back one field: the editor opens on the amount.
        press(&mut app, KeyCode::BackTab);
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "2026-09-09");
        press(&mut app, KeyCode::Enter);

        assert_eq!(history(&app).rows()[0].date, day(2026, 9, 9));
    }

    /// A field that will not parse reports itself on the status line and
    /// leaves the editor open on what was typed, the way every other form
    /// does.
    #[test]
    fn an_unparseable_amount_leaves_the_editor_open() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "lots");

        press(&mut app, KeyCode::Enter);

        assert!(matches!(history(&app).mode(), Mode::Editing(_)));
        assert!(!app.status.is_empty());
        assert_eq!(history(&app).total(), Cents(1_000_000));
    }

    #[test]
    fn d_then_y_deletes_the_row_and_the_list_stays_open_on_what_is_left() {
        let mut app = on_vacation();
        let before = app.savings.excess()[0].1;
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(history(&app).mode(), Mode::Confirming { .. }));
        assert_eq!(app.topic(), Topic::Confirm);
        assert!(drawn(&mut app).contains("Delete this allocation?"));

        press(&mut app, KeyCode::Char('y'));

        assert!(matches!(history(&app).mode(), Mode::List));
        assert!(history(&app).rows().is_empty());
        assert_eq!(app.status, "allocation deleted");
        assert_eq!(app.savings.rows()[1].current, Cents::ZERO);
        assert_eq!(app.savings.excess()[0].1, before + Cents(1_000_000));
    }

    #[test]
    fn d_then_any_other_key_writes_nothing() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('n'));

        assert!(matches!(history(&app).mode(), Mode::List));
        assert_eq!(history(&app).rows().len(), 1);
        assert_eq!(app.status, "delete cancelled");
    }

    /// `Esc` peels one layer at a time: from the confirmation back to the
    /// list, and only then out of the modal.
    #[test]
    fn esc_out_of_the_confirmation_returns_to_the_list_rather_than_closing() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('d'));

        press(&mut app, KeyCode::Esc);
        assert!(matches!(history(&app).mode(), Mode::List));

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert_eq!(app.screen, Screen::Savings);
    }

    /// `Enter` is deliberately unbound in the list: it commits in the editing
    /// mode a keystroke away, and one key that opens an editor in one mode
    /// and commits it in the next is the reflex there is no getting back.
    #[test]
    fn enter_in_the_list_does_nothing() {
        let mut app = on_vacation();
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Enter);

        assert!(matches!(history(&app).mode(), Mode::List));
        assert_eq!(history(&app).rows().len(), 1);
    }

    /// The list takes the shared scroll keys like every other list in the
    /// app, and it opens on the most recent row.
    #[test]
    fn the_list_opens_on_the_most_recent_row_and_scrolls_off_it() {
        let mut app = on_vacation();
        let goal_id = app.savings.selected().unwrap().goal_id;
        goal::insert_allocation(
            &app.db,
            goal_id,
            day(2026, 8, 20),
            Cents(50_000),
            Some("top up"),
            None,
        )
        .unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Enter);
        assert_eq!(history(&app).selected().unwrap().cents, Cents(50_000));

        press(&mut app, KeyCode::Up);
        assert_eq!(history(&app).selected().unwrap().cents, Cents(1_000_000));
    }

    /// The pot `/N` divides is the container's remainder as it stands now.
    /// The row's own amount is not folded back into it, so the figure the
    /// editor divides is the one the Savings footer is showing behind it.
    #[test]
    fn a_share_typed_while_editing_divides_the_remainder_as_it_stands() {
        let mut app = on_vacation();
        let savings = app.savings.excess()[0].0;
        // Top Rainy Day up to a round 2,500.00 unallocated.
        write(&app.db, savings, day(2026, 8, 15), 1_255_000, "Interest");
        app.reload().unwrap();
        assert_eq!(app.savings.excess()[0].1, Cents(250_000));

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "/2");
        press(&mut app, KeyCode::Enter);

        assert_eq!(history(&app).total(), Cents(125_000));
    }
}
