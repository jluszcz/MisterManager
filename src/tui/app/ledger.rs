//! The two ledgers: rows, the forms that write them, and the reconcile.
//!
//! Cash and credit are one screen twice over, which is why `ledger`,
//! `ledger_mut` and `ledgers_mut` are here rather than on `App`: every key
//! on this screen acts on whichever of the two the tab bar is showing, and
//! the month and the window are shared between them.

use super::{Account, App, Label, NOTHING_SELECTED, Screen};
use crate::db::{account, setting, txn};
use crate::default_source::Source;
use crate::description;
use crate::money::Cents;
use crate::tui::autocomplete::Autocomplete;
use crate::tui::cursor;
use crate::tui::form::{self, DateField, TransferForm, TxnForm, ValueForm};
use crate::tui::ledger::Ledger;
use crate::tui::modal::{Confirm, Modal};
use crate::tui::search::{self, Search};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn ledger(&self) -> &Ledger {
        match self.screen {
            Screen::Credit => &self.credit,
            _ => &self.cash,
        }
    }

    pub(super) fn ledger_mut(&mut self) -> &mut Ledger {
        match self.screen {
            Screen::Credit => &mut self.credit,
            _ => &mut self.cash,
        }
    }

    /// Both ledgers, for the things that are not one screen's state: the
    /// shared month, the cursor re-anchoring that follows it, and the date
    /// range that bounds them. Iterating is what keeps "both, always" from
    /// decaying into a pair of lines one of which is later forgotten.
    pub(super) fn ledgers_mut(&mut self) -> [&mut Ledger; 2] {
        [&mut self.cash, &mut self.credit]
    }

    pub(super) fn ledger_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(self.ledger_mut(), key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('[') => {
                self.ledger_mut().previous_month();
                self.sync_month()?;
            }
            KeyCode::Char(']') => {
                self.ledger_mut().next_month();
                self.sync_month()?;
            }
            // Both filters at once, once a kept needle is gone: the account
            // back to All, and the window -- which has no All to reach, since
            // it bounds the query itself -- back to the month the screen
            // opens on. Only the window crosses to the other ledger; the
            // account and the needle belong to this one.
            KeyCode::Esc => {
                if !search::escape_kept_filter(self.ledger_mut()) {
                    let today = self.today;
                    self.ledger_mut().clear_filters(today);
                    self.sync_month()?;
                }
            }
            KeyCode::Tab => {
                self.ledger_mut().next_account();
                self.reload_and_anchor()?;
            }
            KeyCode::BackTab => {
                self.ledger_mut().previous_account();
                self.reload_and_anchor()?;
            }
            KeyCode::Char('/') => self.ledger_mut().begin_search(),
            KeyCode::Char('r') => self.open_reconcile(),
            KeyCode::Char('a') => self.open_add()?,
            // Cash-only: a transfer leaves an account you hold, so there is
            // nothing on the Credit ledger for it to start from. `p` is the
            // move that belongs to a card -- cash settling it.
            KeyCode::Char('t') if self.screen == Screen::Cash => self.open_transfer()?,
            KeyCode::Char('p') => self.open_payment()?,
            KeyCode::Char('e') => self.open_edit()?,
            KeyCode::Char('d') => self.open_delete(),
            _ => {}
        }
        Ok(())
    }

    /// `r`: the balance a statement says the filtered account holds.
    ///
    /// Only under an account filter. Under All the border quotes the whole
    /// kind's balance, which is not a figure any statement names, so there
    /// would be nothing for the target to be compared against.
    ///
    /// Opens on the target already set, so a figure being corrected is edited
    /// rather than retyped.
    fn open_reconcile(&mut self) {
        let ledger = self.ledger();
        let Some(id) = ledger.selected_account() else {
            self.status = "reconcile needs an account filter".to_string();
            return;
        };
        let label = Label::plain("Target · ").account(Account::named(ledger.accounts(), id));
        let prefill = ledger.target().map(|t| t.to_string()).unwrap_or_default();
        self.modal = Some(Modal::Reconcile(id, ValueForm::money(label, &prefill)));
    }

    /// An empty field clears the target: that is how one goes away, since
    /// `Esc` means "leave the figure alone" everywhere else in the app.
    ///
    /// Parsed before the modal closes, so a rejected figure keeps the form
    /// and everything typed into it. Nothing is written and nothing is
    /// reloaded -- the target lives on the `Ledger` until the app quits.
    pub(super) fn commit_reconcile(&mut self) -> Result<()> {
        let Some(Modal::Reconcile(id, form)) = &self.modal else {
            return Ok(());
        };
        let (id, raw) = (*id, form.value().trim().to_string());
        let target = if raw.is_empty() {
            None
        } else {
            Some(form::parse_amount(&raw)?)
        };
        let name = crate::demo::text(self.ledger().account_name(id)).into_owned();
        self.ledger_mut().set_target(target);
        self.status = match target {
            Some(cents) => format!("{name} target {}", crate::demo::figure(cents)),
            None => format!("{name} target cleared"),
        };
        self.close_modal();
        Ok(())
    }

    /// The balance a ledger's `Tab` filter names: the whole kind under All,
    /// one account when narrowed to one.
    ///
    /// Quoted at `today` and not at the window, which is what makes it the
    /// same figure as the Overview's To-Date column -- the two screens showing
    /// one balance under two numbers is the failure this exists to avoid. It
    /// is also as *stored*, so the Credit ledger's total is debt-positive like
    /// the column above it; the Overview is the one screen that negates.
    pub(super) fn ledger_total(&self, ledger: &Ledger) -> Result<Cents> {
        match ledger.selected_account() {
            Some(id) => txn::balance_at(&self.db, id, self.today),
            None => txn::balance_at_by_kind(&self.db, ledger.kind(), self.today),
        }
    }

    /// Opens on the account the ledger is filtered to, or on the first when
    /// the filter is All, and on [`App::entry_date`] -- today, until a row
    /// has been added this session.
    fn open_add(&mut self) -> Result<()> {
        let accounts = account::list_by_kind(&self.db, self.kind())?;
        let preselected = self.ledger().selected_account();
        self.modal = Some(Modal::Txn(TxnForm::add(
            accounts,
            self.entry_field(),
            preselected,
        )?));
        Ok(())
    }

    fn open_edit(&mut self) -> Result<()> {
        let Some(row) = self.ledger().selected().cloned() else {
            return self.nothing_selected();
        };
        let accounts = account::list_by_kind(&self.db, self.kind())?;
        self.modal = Some(Modal::Txn(TxnForm::edit(accounts, self.today, &row)?));
        Ok(())
    }

    /// Opens on [`App::entry_date`], the same day `a` does: a transfer or a
    /// card payment read off a statement belongs to the sitting the rows
    /// around it belong to.
    fn open_transfer(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::Transfer(TransferForm::transfer(
            accounts,
            self.entry_field(),
            setting::get(&self.db, Source::Transfer.key())?,
        )?));
        Ok(())
    }

    /// Opens on [`App::entry_date`], for the reason [`App::open_transfer`]
    /// gives.
    fn open_payment(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        self.modal = Some(Modal::Transfer(TransferForm::payment(
            accounts,
            self.entry_field(),
            setting::get(&self.db, Source::Payment.key())?,
        )?));
        Ok(())
    }

    /// The date field every form that writes a ledger row opens with: the day
    /// the last such row was written for, and today until there is one.
    fn entry_field(&self) -> DateField {
        DateField::on(self.today, self.entry_date.unwrap_or(self.today))
    }

    fn open_delete(&mut self) {
        match self.ledger().selected().cloned() {
            None => self.status = NOTHING_SELECTED.to_string(),
            Some(row) => {
                let label = format!(
                    "{}  {}  {}",
                    row.date,
                    description::render(&row.description),
                    crate::demo::figure(row.cents)
                );
                self.modal = Some(Modal::Confirm {
                    action: Confirm::DeleteTxn(row.id),
                    label,
                });
            }
        }
    }

    /// One or more characters in a form's description field opens the popup;
    /// an empty field, or a form with no description field at all, closes it.
    pub(super) fn refresh_suggestions(&mut self) -> Result<()> {
        let prefix = self
            .modal
            .as_mut()
            .and_then(Modal::fields_mut)
            .and_then(|f| f.suggestion_prefix())
            .unwrap_or_default()
            .to_string();
        if prefix.is_empty() {
            self.popup.clear();
            return Ok(());
        }
        let hits = txn::autocomplete(&self.db, &prefix, Autocomplete::LIMIT)?;
        self.popup.set(hits);
        Ok(())
    }

    pub(super) fn commit_txn_form(&mut self) -> Result<()> {
        let Some(Modal::Txn(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                txn::update(&self.db, id, &new)?;
                "updated"
            }
            None => {
                txn::insert(&self.db, &new)?;
                // Only an add. An edit is a correction to a row already
                // written rather than a statement about the day being worked
                // on, and fixing something months back must not drag the next
                // new row there with it.
                self.entry_date = Some(new.date);
                "added"
            }
        };
        // The date is named whatever it is. The form no longer opens on the
        // date field, so a row can be written without a keystroke ever
        // visiting it, and the confirmation is the only place the day it
        // landed on appears -- unconditionally, because a line that spoke up
        // only when the date was surprising is a line the eye learns to skip
        // on the rounds it says nothing.
        self.status = format!(
            "{verb} {} {} on {}",
            description::render(&new.description),
            crate::demo::figure(new.cents),
            new.date
        );
        self.close_modal();
        self.reload()
    }

    pub(super) fn commit_transfer_form(&mut self) -> Result<()> {
        let Some(Modal::Transfer(form)) = &self.modal else {
            return Ok(());
        };
        let moved = form.commit()?;
        // One description, used for both legs.
        txn::insert_transfer(
            &self.db,
            moved.from_account_id,
            moved.to_account_id,
            moved.date,
            moved.cents,
            &moved.description,
            &moved.description,
        )?;
        // Both legs are new rows, so this is a statement about the day being
        // entered for in the way an `e` is not, and it names its date for the
        // reason `commit_txn_form` does.
        self.entry_date = Some(moved.date);
        self.status = format!(
            "{} {} on {}",
            description::render(&moved.description),
            crate::demo::figure(moved.cents),
            moved.date
        );
        self.close_modal();
        self.reload()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::AccountId;
    use crate::db::account::{self, Group, Kind};
    use crate::money::Cents;
    use crate::test_support::day;
    use crate::tui::app::test_support::*;
    use crate::tui::form::{TransferField, TxnField};
    use crate::tui::modal::Modal;
    use crate::tui::search::Search;
    use ratatui::crossterm::event::KeyCode;

    /// Add a row through the keyboard, stepping the date `days` from wherever
    /// the form opens. The description must match nothing already written, or
    /// `Tab` and `Enter` would be answering the autocomplete popup instead of
    /// the form.
    fn add_row(app: &mut App, days: usize, description: &str) {
        press(app, KeyCode::Char('a'));
        focus(app, TxnField::Date);
        for _ in 0..days {
            press(app, KeyCode::Right);
        }
        focus(app, TxnField::Description);
        type_str(app, description);
        focus(app, TxnField::Amount);
        type_str(app, "10");
        press(app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
    }

    fn form_date(app: &App) -> String {
        form(app).display(TxnField::Date).plain_text()
    }

    fn transfer_date(app: &App) -> String {
        match &app.modal {
            Some(Modal::Transfer(form)) => form.display(TransferField::Date).plain_text(),
            _ => panic!("no transfer form is open"),
        }
    }

    /// The first row of a session has nothing behind it to take a date from,
    /// so the form opens where every other date field in the app does.
    #[test]
    fn the_first_add_of_a_session_opens_on_today() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));

        assert_eq!(form_date(&app), "2026-08-15");
    }

    /// Entering a statement is a run of rows landing on the same few days, so
    /// the day the last one was written for is a better opening guess than
    /// today -- and the owner who has moved off today said so by moving.
    #[test]
    fn an_add_opens_on_the_date_the_last_row_was_added_with() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// The date is the day being entered for rather than a property of either
    /// ledger, so it survives the walk from one to the other: a statement and
    /// the card rows on it are the same sitting.
    #[test]
    fn the_date_carries_from_one_ledger_to_the_other() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// `t` and `p` write ledger rows in the same sitting `a` does -- the
    /// card payment at the bottom of the statement whose rows were just
    /// entered -- so they open where `a` does rather than back on today.
    #[test]
    fn a_transfer_and_a_payment_open_on_the_date_the_last_row_was_added_with() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(transfer_date(&app), "2026-08-20");
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('p'));
        assert_eq!(transfer_date(&app), "2026-08-20");
    }

    /// Both legs are new rows, so a transfer is a statement about the day
    /// being entered for in the way an edit is not.
    #[test]
    fn committing_a_payment_moves_the_date_the_next_add_opens_on() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        for _ in 0..5 {
            press(&mut app, KeyCode::Right);
        }
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
        }
        type_str(&mut app, "10");
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// An edit is a correction to a row already written, not a statement
    /// about where the hand is working. Fixing the date on something months
    /// back must not drag the next new row there with it.
    #[test]
    fn editing_a_row_leaves_the_date_the_next_add_opens_on_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        add_row(&mut app, 5, "Kite");

        press(&mut app, KeyCode::Char('e'));
        focus(&mut app, TxnField::Date);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none(), "the edit stayed open: {}", app.status);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(form_date(&app), "2026-08-20");
    }

    /// `[`/`]` reach a form's date the way the arrows do -- and stop there.
    /// A bracket is an ordinary character in a description, so the key has to
    /// go on to the text when the caret is not in a date.
    #[test]
    fn brackets_step_a_form_date_a_month_and_still_type_into_a_description() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Date);
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(form_date(&app), "2026-09-15");
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Char('['));
        assert_eq!(form_date(&app), "2026-07-15");

        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Lot [3]");
        assert_eq!(
            form(&app).display(TxnField::Description).plain_text(),
            "Lot [3]"
        );
    }

    /// Rows in July, August and September, so `[` and `]` have somewhere to
    /// go: the window is clamped to the months the data covers, and the
    /// single-month fixture above cannot step at all.
    fn app_spanning_three_months() -> App {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, checking, day(2026, 7, 5), 100_000, "July");
        write(&db, checking, day(2026, 8, 5), 100_000, "August");
        write(&db, checking, day(2026, 9, 5), 100_000, "September");
        write(&db, card_one, day(2026, 7, 6), 1_000, "July card");
        write(&db, card_one, day(2026, 9, 6), 1_000, "September card");
        App::new(db, today()).unwrap()
    }

    #[test]
    fn stepping_the_month_on_cash_steps_credit_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.credit.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));

        assert_ne!(app.cash.window(), august, "] must move the active ledger");
        assert_eq!(app.credit.window(), app.cash.window());
    }

    #[test]
    fn stepping_the_month_on_credit_steps_cash_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('['));

        assert_ne!(app.credit.window(), august, "[ must move the active ledger");
        assert_eq!(app.cash.window(), app.credit.window());
    }

    /// The ledger narrows two ways as well, and `Esc` is the one key out of
    /// either: an owner who has tabbed to an account and stepped the month
    /// should not have to work out which of the two is hiding the row they
    /// are looking for.
    #[test]
    fn esc_clears_the_account_filter_as_well_as_the_window() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(']'));
        assert!(app.cash.selected_account().is_some());
        assert_ne!(app.cash.window(), august);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.selected_account(), None);
        assert_eq!(app.cash.window(), august);
    }

    /// The account filter belongs to one ledger the way the needle does --
    /// the two hold different accounts -- where the window belongs to both.
    #[test]
    fn clearing_the_account_filter_on_one_ledger_leaves_the_other_alone() {
        let mut app = app_spanning_three_months();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Tab);
        let card = app.credit.selected_account();
        assert!(card.is_some());

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.cash.selected_account(), None);
        assert_eq!(app.credit.selected_account(), card);
    }

    /// The innermost thing first: a kept needle goes before the two filters
    /// under it, exactly as it goes before the window on its own.
    #[test]
    fn esc_clears_a_kept_needle_before_the_account_filter() {
        let mut app = app_spanning_three_months();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        let checking = app.cash.selected_account();
        assert!(checking.is_some());

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "aug");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.search(), "");
        assert_eq!(
            app.cash.selected_account(),
            checking,
            "the account filter is the next thing out"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.selected_account(), None);
    }

    /// The ledgers have no All to clear to -- their window is pushed down
    /// into the SQL -- so `Esc` means "back to the window the screen opens
    /// on", and it takes the other ledger with it the way `[` and `]` do.
    #[test]
    fn esc_returns_both_ledgers_to_the_window_around_today() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));
        assert_ne!(app.cash.window(), august);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.window(), august);
        assert_eq!(app.credit.window(), august);
    }

    #[test]
    fn esc_on_credit_returns_cash_with_it() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.credit.window(), august);
        assert_eq!(app.cash.window(), august);
    }

    /// The other ledger is re-queried too: a synced window over stale rows
    /// would show August's rows under a September heading.
    #[test]
    fn the_other_ledgers_rows_follow_the_shared_month() {
        let mut app = app_spanning_three_months();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));

        assert_eq!(descriptions(&app.credit), ["September card"]);
    }

    /// `BackTab` is `Tab`'s cycle read backwards, and it re-queries the same
    /// way: a filter moved without the rows following it shows one account's
    /// rows under another's heading.
    #[test]
    fn back_tab_steps_the_ledgers_account_filter_the_other_way() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Transfer"],
            "the last cash account"
        );

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Rent"],
            "the first"
        );

        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"],
            "All"
        );
    }

    #[test]
    fn t_opens_a_transfer_form_on_the_cash_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));
        assert!(matches!(app.modal, Some(Modal::Transfer(_))));
    }

    /// A transfer leaves an account you hold, so the key has nothing to start
    /// from on the Credit ledger. `p` is the move that belongs to a card.
    #[test]
    fn t_opens_nothing_on_the_credit_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('t'));
        assert!(app.modal.is_none());
    }

    #[test]
    fn p_still_opens_a_payment_form_on_the_credit_ledger() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        assert!(matches!(app.modal, Some(Modal::Transfer(_))));
    }

    /// The account the form's `From` opens on, and the one its `To` does --
    /// both by id, which is what the setting holds and what the row it
    /// writes carries.
    fn opens_on(app: &App) -> (AccountId, AccountId) {
        let Some(Modal::Transfer(form)) = &app.modal else {
            panic!("no money form is open");
        };
        let side = |field| form.display(field).accounts()[0].id();
        (side(TransferField::From), side(TransferField::To))
    }

    /// With no default set, `t` opens on the first cash account and steps the
    /// destination off it: the `To` list is every account, so leaving both at
    /// the head of their lists opens the form on a transfer from an account
    /// to itself.
    #[test]
    fn t_opens_on_two_different_accounts_with_no_default_set() {
        let mut app = app();
        let first = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));

        let (from, to) = opens_on(&app);
        assert_eq!(from, first);
        assert_ne!(to, from);
    }

    /// `t` opens on the transfer key's account, and `To` steps off *that*
    /// one rather than off the head of the list.
    #[test]
    fn t_opens_on_the_default_transfer_source() {
        let mut app = app();
        let savings = account::list_by_kind(&app.db, Kind::Cash).unwrap()[1].id;
        setting::set(&app.db, Source::Transfer.key(), savings).unwrap();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));

        let (from, to) = opens_on(&app);
        assert_eq!(from, savings);
        assert_ne!(to, savings);
    }

    /// `p` reads its own key. The two are separate settings because paying a
    /// card and moving savings are separate decisions -- a payment opening on
    /// the transfer account would be the one thing one key could not express.
    #[test]
    fn p_opens_on_the_default_payment_source_and_not_the_transfer_one() {
        let mut app = app();
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        setting::set(&app.db, Source::Transfer.key(), cash[1].id).unwrap();
        setting::set(&app.db, Source::Payment.key(), cash[0].id).unwrap();

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));

        assert_eq!(opens_on(&app).0, cash[0].id);
    }

    /// One account may answer for both, which is what two independent keys
    /// buy over a single "default account" naming one.
    #[test]
    fn one_account_can_be_the_default_for_both_forms() {
        let mut app = app();
        let savings = account::list_by_kind(&app.db, Kind::Cash).unwrap()[1].id;
        for source in Source::ALL {
            setting::set(&app.db, source.key(), savings).unwrap();
        }

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(opens_on(&app).0, savings);

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(opens_on(&app).0, savings);
    }

    /// One form backs both keys, and the two write different things: a modal
    /// titled `Transfer` over a `CC1 Payment` description says the owner
    /// pressed the wrong key.
    #[test]
    fn the_two_money_forms_are_titled_by_the_key_that_opened_them() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('t'));
        let Some(Modal::Transfer(form)) = &app.modal else {
            panic!("t opened no form");
        };
        assert!(form.title().starts_with("Transfer"), "{}", form.title());

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('p'));
        let Some(Modal::Transfer(form)) = &app.modal else {
            panic!("p opened no form");
        };
        assert!(form.title().starts_with("Payment"), "{}", form.title());
    }

    /// The cursor opens on the last row dated on or before today, so `d`
    /// takes "Transfer" rather than the first row of the month.
    #[test]
    fn d_then_y_deletes_the_selected_row() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"]
        );

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(descriptions(&app.cash), ["Paycheck", "Whole Foods", "Rent"]);
        assert_eq!(app.status, "deleted");
        assert!(app.modal.is_none());
    }

    /// Opening on the first row of the month would put the cursor hundreds of
    /// rows from the ones just entered.
    #[test]
    fn the_ledger_opens_on_the_last_row_dated_on_or_before_today() {
        let app = app();
        assert_eq!(app.cash.selected().unwrap().description, "Transfer");
    }

    /// `Rent` is dated after today, so anchoring has to skip back over it
    /// rather than simply taking the last row.
    #[test]
    fn a_deleted_row_does_not_send_the_cursor_back_to_today() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::End);
        assert_eq!(app.cash.selected().unwrap().description, "Rent");

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(
            app.cash.selected().unwrap().description,
            "Transfer",
            "the cursor stays at its index, which is now the last row"
        );
    }

    /// The confirmation exists because there is no undo, so anything that is
    /// not `y` has to be a cancel rather than a fall-through.
    #[test]
    fn d_then_any_other_key_cancels_and_the_row_survives() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('n'));

        assert_eq!(
            descriptions(&app.cash),
            ["Paycheck", "Whole Foods", "Transfer", "Rent"]
        );
        assert_eq!(app.status, "delete cancelled");
        assert!(app.modal.is_none());
    }

    #[test]
    fn q_while_searching_types_into_the_box_rather_than_quitting() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "q");

        assert!(!app.should_quit());
        assert_eq!(app.cash.search(), "q");
        assert!(app.cash.rows().is_empty(), "nothing matches \"q\"");
    }

    /// `r` is reconciliation: the balance a statement says the account holds,
    /// typed in while the rows are being entered so a typo or a missed item
    /// shows up as a delta rather than as a surprise next month.
    #[test]
    fn a_typed_target_reaches_the_ledgers_border() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "$1,200.00");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(app.cash.target(), Some(Cents(120_000)));
        assert!(drawn(&mut app).contains("Target $1,200.00"));
    }

    /// Under All the border quotes the whole kind's balance, which no
    /// statement names. The key says so rather than opening a form over a
    /// figure it cannot check.
    #[test]
    fn r_under_the_all_filter_reports_that_it_needs_an_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('r'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("account filter"), "{}", app.status);
    }

    /// A target is the account's, not the screen's: the other ledger is a
    /// different set of accounts and reconciles against different statements.
    #[test]
    fn a_target_on_one_ledger_leaves_the_other_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));

        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Tab);

        assert_eq!(app.credit.target(), None);
        assert!(!drawn(&mut app).contains("Target"));
    }

    /// Reopening on a target already set shows it, so a figure being
    /// corrected is edited rather than retyped.
    #[test]
    fn r_reopens_on_the_target_already_set() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('r'));

        let Some(Modal::Reconcile(_, form)) = &app.modal else {
            panic!("r must reopen the form: {:?}", app.status);
        };
        assert_eq!(form.value(), "1,200.00");
    }

    /// Emptying the field is how a target goes away: the account is back to
    /// having none, and the border to the balance alone.
    #[test]
    fn an_emptied_target_form_clears_the_target() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));

        press(&mut app, KeyCode::Char('r'));
        for _ in 0.."1,200.00".len() {
            press(&mut app, KeyCode::Backspace);
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.cash.target(), None);
        assert!(!drawn(&mut app).contains("Target"));
    }

    /// `Esc` backs out of the form, which is not the same as clearing the
    /// figure the form opened on.
    #[test]
    fn esc_leaves_a_target_already_set_alone() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "9");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.cash.target(), Some(Cents(120_000)));
    }

    /// A figure that does not parse keeps the form and everything typed into
    /// it, the way every other form in the app refuses one.
    #[test]
    fn a_target_that_does_not_parse_keeps_the_form_open() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "twelve");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "{}", app.status);
        assert_eq!(app.cash.target(), None);
        assert!(drawn(&mut app).contains("twelve"));
    }

    /// The target belongs to the account it was typed on, so the `Tab` cycle
    /// carries it -- and the balance it is compared with moves with the same
    /// key.
    #[test]
    fn stepping_the_account_filter_carries_each_targets_own_delta() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('r'));
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Tab);
        assert!(
            !drawn(&mut app).contains("Target"),
            "Rainy Day has no target"
        );

        press(&mut app, KeyCode::BackTab);
        assert!(drawn(&mut app).contains("Target $1,200.00"));
    }

    #[test]
    fn a_on_a_ledger_with_no_accounts_reports_it_in_the_status_line() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));

        assert!(app.status.contains("no account"), "{}", app.status);
        assert!(app.modal.is_none());
        assert!(
            !app.should_quit(),
            "a failed write must not end the session"
        );
    }

    #[test]
    fn committing_a_form_reloads_the_rows_behind_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Zebra");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let rows = app.cash.rows();
        let written = rows
            .iter()
            .find(|t| t.description == "Zebra")
            .expect("the new row must be on screen without another keystroke");
        assert_eq!(written.cents, Cents(4_250));
        assert_eq!(written.date, today());
        assert_eq!(app.status, "added Zebra 42.50 on 2026-08-15");
    }

    /// The status line is the only confirmation a write gets, and a blank
    /// description would leave `added  42.50 on ...` -- a gap between two
    /// spaces that reads as a figure that failed to render rather than as a
    /// row the owner chose not to name.
    #[test]
    fn a_row_written_with_no_description_is_confirmed_by_its_em_dash() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert_eq!(app.status, "added — 42.50 on 2026-08-15");
        assert!(
            app.cash.rows().iter().any(|t| t.description.is_empty()),
            "the row itself keeps the empty description it was written with"
        );
    }

    /// Deleting is the one irreversible key on the ledger, and the label is
    /// the whole of what the question offers to identify the row by. An
    /// unnamed row still has to be identifiable as the row the cursor is on.
    #[test]
    fn the_delete_confirmation_names_an_unnamed_row_by_its_em_dash() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Amount);
        type_str(&mut app, "42.50");
        press(&mut app, KeyCode::Enter);

        for _ in 0..app.cash.rows().len() {
            if app
                .cash
                .selected()
                .is_some_and(|t| t.description.is_empty())
            {
                break;
            }
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Char('d'));

        let Some(Modal::Confirm { label, .. }) = &app.modal else {
            panic!("d must open the confirmation: {:?}", app.status);
        };
        assert_eq!(label, "2026-08-15  —  42.50");
    }

    /// `a` on a ledger filtered to one card must not open on a different one:
    /// it is a plausible misfiled row in exactly the workflow `Tab` exists
    /// for, and there is no undo.
    #[test]
    fn adding_on_an_account_filtered_ledger_preselects_that_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(descriptions(&app.credit), ["Batteries"]);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(
            form(&app).display(TxnField::Account).plain_text(),
            "CC2 — Card Two"
        );
    }

    #[test]
    fn adding_on_an_unfiltered_ledger_opens_on_the_first_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('a'));

        assert_eq!(
            form(&app).display(TxnField::Account).plain_text(),
            "CC1 — Card One"
        );
    }

    /// Without this the only way out of the popup is `Esc`, which discarded
    /// the whole form — everything typed with it.
    #[test]
    fn esc_closes_the_popup_first_and_the_form_second() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Whole");
        assert!(app.popup.visible() > 0, "\"Whole Foods\" is a suggestion");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup.visible(), 0);
        assert!(!app.popup.is_open());
        assert_eq!(
            form(&app).description(),
            "Whole",
            "dismissing the popup must keep what was typed"
        );

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    /// A popup whose rows were all clipped away captures no keys, so `Esc`
    /// must reach the modal rather than being swallowed by an invisible list.
    #[test]
    fn esc_closes_the_form_when_the_popup_drew_no_rows() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('a'));
        focus(&mut app, TxnField::Description);
        type_str(&mut app, "Whole");
        app.popup.set_visible(0);

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }

    /// `Tab` is the one key that moves the ledger total: All is the kind's
    /// balance, and narrowing to an account is that account's. Both are
    /// balances *at today*, so the Rent row dated the 20th is in neither.
    #[test]
    fn tab_moves_the_ledger_total_from_the_kind_to_the_selected_account() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        // Everyday 1,000.00 - 50.00, and Rainy Day 200.00.
        assert_eq!(app.cash.total(), Cents(115_000));
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.cash.selected_account(),
            Some(app.cash.rows()[0].account_id)
        );
        assert_eq!(app.cash.total(), Cents(95_000), "Everyday alone");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.cash.total(), Cents(20_000), "Rainy Day alone");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.cash.total(), Cents(115_000), "back to All");
    }

    /// The Credit ledger renders amounts as stored — debt positive — and its
    /// total agrees with the column above it rather than with the Overview,
    /// which is the one screen that negates.
    #[test]
    fn the_credit_total_reads_as_stored_debt_where_the_overview_negates_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.credit.total(), Cents(4_098));
        assert_eq!(app.overview.credit.total.to_date, Cents(-4_098));
    }

    /// The total is a balance at a date, so the rows it is drawn over do not
    /// bound it: stepping the window changes the rows and leaves it alone.
    /// The future-dated row is outside it under either window.
    #[test]
    fn stepping_the_month_leaves_the_ledger_total_alone() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        write(&db, checking, day(2026, 7, 4), 50_000, "July");
        write(&db, checking, day(2026, 8, 10), 30_000, "August");
        write(&db, checking, day(2026, 8, 20), 999_999, "Not yet");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.cash.rows().len(), 2, "August");
        assert_eq!(app.cash.total(), Cents(80_000));

        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.cash.rows().len(), 1, "July");
        assert_eq!(app.cash.total(), Cents(80_000));
    }

    /// The same order on a ledger, where the outer filter is the window.
    #[test]
    fn esc_outside_a_ledger_box_clears_a_kept_search_before_the_window() {
        let mut app = app_spanning_three_months();
        let august = app.cash.window();

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));
        let september = app.cash.window();
        assert_ne!(september, august);

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "sept");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.cash.rows().len(), 1);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.search(), "");
        assert_eq!(
            app.cash.window(),
            september,
            "the window is the next thing out"
        );

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.cash.window(), august);
    }

    /// The needle is one ledger's; the window is both. Clearing a kept filter
    /// on Cash must not reach across to Credit the way `Esc` on the window
    /// deliberately does.
    #[test]
    fn clearing_a_kept_filter_on_one_ledger_leaves_the_other_alone() {
        let mut app = app_spanning_three_months();
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "card");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "aug");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.cash.search(), "");
        assert_eq!(app.credit.search(), "card");
    }
}
