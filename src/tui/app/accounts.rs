//! The Accounts screen: the four things about an account that are the
//! owner's rather than the workbook's -- its name, its color, its order and
//! its Overview band -- plus the interest policy and the two savings-block
//! keys, which no cell of the sheet carries either.

use super::App;
use crate::db::account::{self, Kind};
use crate::db::{AccountId, setting};
use crate::default_source::Source;
use crate::savings_block::Block as SavingsBlock;
use crate::tui::accounts::{self as accounts_screen, AccountForm};
use crate::tui::cursor;
use crate::tui::modal::Modal;
use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// The account each `Savings` block names, one entry per `Block::ALL` entry.
type Containers = [Option<AccountId>; SavingsBlock::ALL.len()];

/// The account each money form opens its `From` on, one entry per
/// `Source::ALL` entry.
type Defaults = [Option<AccountId>; Source::ALL.len()];

impl App {
    /// Insert the account `a` describes, appended to whatever its kind
    /// already holds.
    ///
    /// The sort is computed the way `import::constants` computes it, and for
    /// the same reason: `sort` is only ever read through an `ORDER BY` that
    /// breaks ties by code, so "after the ones already there" is the only
    /// placement that does not depend on rows nobody has seen. Moving it
    /// anywhere else is `e`'s `Order` selector, which renumbers the kind.
    fn commit_new_account(&mut self) -> Result<()> {
        let Some(Modal::Account(form)) = &self.modal else {
            return Ok(());
        };
        let new = form.commit_new()?;
        let sort = account::list_by_kind(&self.db, new.kind)?.len() as i64;
        account::insert(&self.db, &new.code, &new.name, new.kind, sort)?;
        self.status = format!("{} added", crate::demo::text(new.name.as_str()));
        self.close_modal();
        self.reload()
    }

    /// Rebuild the Accounts screen, and hand the refreshed list to every
    /// screen that shows an account's name.
    ///
    /// Those three hold their own `Vec<Account>` -- a rename made here would
    /// otherwise not reach the Overview's neighbours until a restart.
    pub(super) fn reload_accounts(&mut self) -> Result<()> {
        let accounts = account::list(&self.db)?;
        let containers = self.savings_containers()?;
        let defaults = self.default_sources()?;
        let mut rows = Vec::with_capacity(accounts.len());
        for account in &accounts {
            rows.push(accounts_screen::Row {
                account: super::Account::named(&accounts, account.id),
                code: account.code.as_str().to_string(),
                kind: account.kind,
                group: account.group,
                policy: account::interest_policy(&self.db, account.id)?,
                block: block_of(&containers, account.id),
                defaults: sources_of(&defaults, account.id),
            });
        }
        self.accounts.set_rows(rows);
        self.savings.set_accounts(accounts);
        // Named rather than reached through `ledgers_mut`: each ledger holds
        // only its own kind's accounts -- that list *is* its `Tab` filter --
        // so the two take different arguments, exactly as `Ledger::new` does.
        self.cash
            .set_accounts(account::list_by_kind(&self.db, Kind::Cash)?);
        self.credit
            .set_accounts(account::list_by_kind(&self.db, Kind::Credit)?);
        Ok(())
    }

    /// Point this account at `block`, and off whichever block it held.
    ///
    /// The form carries one value, so an account cannot claim both blocks;
    /// what it *can* do is move off the one it had, and that has to clear the
    /// key rather than leave it naming an account that no longer answers for
    /// it. A key is only ever cleared when this account is the one it names,
    /// so editing one account never disturbs the other block's mapping.
    fn set_savings_block(&mut self, id: AccountId, block: Option<SavingsBlock>) -> Result<()> {
        for candidate in SavingsBlock::ALL {
            let key = candidate.key();
            match Some(candidate) == block {
                true => setting::set(&self.db, key, id)?,
                false if setting::get(&self.db, key)? == Some(id) => setting::clear(&self.db, key)?,
                false => {}
            }
        }
        Ok(())
    }

    /// Point the forms in `defaults` at this account, and off whichever ones
    /// it used to answer for.
    ///
    /// A key is only ever cleared when this account is the one it names,
    /// which is what keeps editing one account from disturbing another's
    /// defaults -- the same rule [`App::set_savings_block`] follows, for the
    /// same reason. What differs is the shape: a source *set* rather than one
    /// value, because the two keys are independent and one account may
    /// answer for both.
    fn set_default_sources(&mut self, id: AccountId, defaults: &[Source]) -> Result<()> {
        for source in Source::ALL {
            let key = source.key();
            match defaults.contains(&source) {
                true => setting::set(&self.db, key, id)?,
                false if setting::get(&self.db, key)? == Some(id) => setting::clear(&self.db, key)?,
                false => {}
            }
        }
        Ok(())
    }

    pub(super) fn accounts_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.accounts, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('a') => self.modal = Some(Modal::Account(AccountForm::add())),
            KeyCode::Char('e') => self.open_account_edit()?,
            _ => {}
        }
        Ok(())
    }

    fn open_account_edit(&mut self) -> Result<()> {
        let Some(row) = self.accounts.selected() else {
            return self.nothing_selected();
        };
        let id = row.account.id();
        let (position, of_kind) = self
            .accounts
            .position_of(id)
            .context("the selected account is not in the list it came from")?;
        let account = account::get(&self.db, id)?;
        let policy = account::interest_policy(&self.db, id)?;
        let block = block_of(&self.savings_containers()?, id);
        let defaults = sources_of(&self.default_sources()?, id);
        self.modal = Some(Modal::Account(AccountForm::edit(
            &account, policy, position, of_kind, block, &defaults,
        )));
        Ok(())
    }

    /// The account each `Savings` block names, in `Block::ALL`'s order.
    ///
    /// Read from the keys rather than from a column: the mapping is a fact
    /// about the *workbook*, not about the account, and `savings_block::Block`
    /// is what pairs each key with the block it names. A key naming an account
    /// that is gone simply matches nothing -- this is a lookup, not a
    /// resolution, so a dangling one is `import::savings::containers`' error
    /// to raise.
    ///
    /// The mapping is the same whichever account is being asked about, which
    /// is why it is read once and `block_of` then answers per row.
    fn savings_containers(&self) -> Result<Containers> {
        let mut containers: Containers = [None; SavingsBlock::ALL.len()];
        for (container, block) in containers.iter_mut().zip(SavingsBlock::ALL) {
            *container = setting::get(&self.db, block.key())?;
        }
        Ok(containers)
    }

    /// The account each money form opens its `From` on, in `Source::ALL`'s
    /// order.
    ///
    /// Read from the keys rather than from a column, and a key naming an
    /// account that is gone simply matches nothing -- both for
    /// [`App::savings_containers`]' reasons. What it costs here is smaller
    /// still: a stale default is a form opening on the head of its list,
    /// which the owner can see and correct on this screen.
    fn default_sources(&self) -> Result<Defaults> {
        let mut defaults: Defaults = [None; Source::ALL.len()];
        for (account, source) in defaults.iter_mut().zip(Source::ALL) {
            *account = setting::get(&self.db, source.key())?;
        }
        Ok(defaults)
    }

    /// `a`'s one write, or the six `e` stands for.
    ///
    /// The six are ordered so `reorder` means what it says: it renumbers by
    /// position, so it goes after the band change rather than before one that
    /// could move the row. The two `setting` writes under it read no column
    /// at all, which is why they can follow. `a` writes none of the six -- a
    /// new account takes its kind's default band, no color, no interest
    /// policy, no `Savings` block and neither money form's default, and `e`
    /// is where it is placed.
    pub(super) fn commit_account(&mut self) -> Result<()> {
        let Some(Modal::Account(form)) = &self.modal else {
            return Ok(());
        };
        let Some(id) = form.editing else {
            return self.commit_new_account();
        };
        let edit = form.commit()?;
        account::set_name(&self.db, id, &edit.name)?;
        account::set_color(&self.db, id, edit.color)?;
        account::set_group(&self.db, id, edit.group)?;
        account::set_interest_policy(&self.db, id, edit.policy)?;
        account::reorder(&self.db, id, edit.position)?;
        self.set_savings_block(id, edit.block)?;
        self.set_default_sources(id, &edit.defaults)?;
        self.status = format!("{} saved", crate::demo::text(edit.name.as_str()));
        self.close_modal();
        self.reload()
    }
}

/// Which `Savings` block `id` is the container for, if either, against a
/// mapping `savings_containers` has already read.
fn block_of(containers: &Containers, id: AccountId) -> Option<SavingsBlock> {
    SavingsBlock::ALL
        .into_iter()
        .zip(containers)
        .find(|(_, container)| **container == Some(id))
        .map(|(block, _)| block)
}

/// Which money forms open on `id`, against a mapping `default_sources` has
/// already read. `block_of`'s counterpart, and a set rather than an option
/// for the reason the `Default` selector's choices are subsets: the two keys
/// are independent, so an account may answer for both.
fn sources_of(defaults: &Defaults, id: AccountId) -> Vec<Source> {
    Source::ALL
        .into_iter()
        .zip(defaults)
        .filter(|(_, account)| **account == Some(id))
        .map(|(source, _)| source)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Group, Kind};
    use crate::db::{AccountId, setting};
    use crate::default_source::Source;
    use crate::savings_block::Block as SavingsBlock;
    use crate::test_support::walk_until;
    use crate::tui::accounts as accounts_screen;
    use crate::tui::app::test_support::*;
    use crate::tui::modal::Modal;
    use ratatui::crossterm::event::KeyCode;

    /// Everything on screen 9 is the owner's rather than the workbook's, and
    /// `e` is the one key that writes any of it.
    ///
    /// The account renamed is the goals' container, so the Savings rows -- which
    /// cache their container's name rather than resolving it as they draw --
    /// are in the set of things that have to move.
    #[test]
    fn editing_an_account_renames_it_everywhere_that_shows_a_name() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[1].id;
        assert_eq!(account::get(&app.db, id).unwrap().name, "Rainy Day");

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(app.modal, Some(Modal::Account(_))));
        for _ in 0..40 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(account::get(&app.db, id).unwrap().name, "Nest Egg");
        // The three screens holding their own account list see it without a
        // restart, which is what `reload_accounts` is for.
        assert_eq!(app.savings.account_name(id), "Nest Egg");
        assert_eq!(app.accounts.rows()[1].account.text(), "Nest Egg");
        assert!(
            app.overview
                .cash
                .bands
                .iter()
                .flat_map(|b| &b.lines)
                .any(|l| l.account.as_ref().is_some_and(|a| a.text() == "Nest Egg"))
        );
        // The cached column, which only moves if `reload` refreshes the account
        // list before it re-sets the goals.
        let cached: Vec<&str> = app
            .savings
            .rows()
            .iter()
            .filter(|r| r.container.id() == id)
            .map(|r| r.container.text())
            .collect();
        assert!(
            !cached.is_empty(),
            "the fixture's goals sit in this account"
        );
        assert!(cached.iter().all(|n| *n == "Nest Egg"), "{cached:?}");
    }

    /// The order is a place among the accounts of one kind, and the write
    /// renumbers all of them -- so moving the last cash account to the front
    /// reverses nothing else.
    #[test]
    fn reordering_an_account_moves_it_among_its_own_kind() {
        let mut app = app();
        let before: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert!(before.len() > 1, "the fixture needs two cash accounts");
        let cards: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Credit)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();

        press(&mut app, KeyCode::Char('9'));
        for _ in 1..before.len() {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(
            app.accounts.selected().unwrap().account.id(),
            *before.last().unwrap()
        );
        press(&mut app, KeyCode::Char('e'));
        // Tab to Order, then step it to the front.
        walk_until!(
            matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Order),
            press(&mut app, KeyCode::Tab)
        );
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        let after: Vec<AccountId> = account::list_by_kind(&app.db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        let mut expected = before.clone();
        let moved = expected.pop().unwrap();
        expected.insert(0, moved);
        assert_eq!(after, expected);
        assert_eq!(
            account::list_by_kind(&app.db, Kind::Credit)
                .unwrap()
                .into_iter()
                .map(|a| a.id)
                .collect::<Vec<_>>(),
            cards,
            "reordering one kind moved the other"
        );
    }

    /// `a` writes the one row the workbook cannot: an account the sheet does
    /// not name. It lands last among its own kind, in that kind's default
    /// band and with no color, because everything else about an account is a
    /// placement and `e` is where an account is placed.
    #[test]
    fn a_creates_an_account_at_the_end_of_its_own_kind() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Cash).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "NST");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        assert_eq!(cash.len(), before + 1);
        let added = cash.last().unwrap();
        assert_eq!(added.code, "NST");
        assert_eq!(added.name, "Nest Egg");
        assert_eq!(added.group, Group::Savings);
        assert_eq!(added.color, None);
    }

    /// The kind selector is what puts a new account on the Credit ledger
    /// rather than the Cash one, and it is asked here because `e` cannot
    /// change it afterwards.
    #[test]
    fn a_creates_a_card_when_the_kind_selector_says_credit() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Credit).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "CC3");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Card Three");
        press(&mut app, KeyCode::Enter);

        let cards = account::list_by_kind(&app.db, Kind::Credit).unwrap();
        assert_eq!(cards.len(), before + 1);
        let added = cards.last().unwrap();
        assert_eq!(added.name, "Card Three");
        assert_eq!(added.group, Group::Credit);
    }

    /// A code the kind already holds is what the next import would match two
    /// rows against. The modal stays open with the message on it, rather than
    /// a `UNIQUE constraint failed` naming an index the owner never typed.
    #[test]
    fn a_refuses_a_code_the_kind_already_holds() {
        let mut app = app();
        let before = account::list_by_kind(&app.db, Kind::Cash).unwrap().len();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "SAV");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Second Rainy Day");
        press(&mut app, KeyCode::Enter);

        assert!(app.status.contains("SAV"), "{}", app.status);
        assert!(app.modal.is_some(), "the form closed over a refused write");
        assert_eq!(
            account::list_by_kind(&app.db, Kind::Cash).unwrap().len(),
            before
        );
    }

    /// The refusal is per kind, because `UNIQUE (code, kind)` is: one code
    /// naming both a cash account and the card drawn on it is the shape the
    /// constraint exists for.
    #[test]
    fn a_accepts_a_code_that_only_the_other_kind_holds() {
        let mut app = app();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "SAV");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Rainy Day Card");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "{}", app.status);
        assert_eq!(
            account::by_code(&app.db, "SAV", Kind::Credit)
                .unwrap()
                .unwrap()
                .name,
            "Rainy Day Card"
        );
    }

    /// A new account has to reach the screens that hold their own account
    /// list, the way a rename does: a card nobody can filter the Credit
    /// ledger to is a card that exists on one screen only.
    #[test]
    fn an_added_account_reaches_the_screens_that_cache_the_account_list() {
        let mut app = app();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "NST");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Nest Egg");
        press(&mut app, KeyCode::Enter);

        assert!(
            app.accounts.rows().iter().any(|r| r.code == "NST"),
            "the Accounts screen does not show it"
        );
        assert!(
            app.cash.accounts().iter().any(|a| a.name == "Nest Egg"),
            "the cash ledger cannot be filtered to it"
        );
    }

    /// The policy decides how an interest posting is divided, so it is a
    /// property of the account and belongs with the rest of its row.
    #[test]
    fn the_interest_policy_is_written_from_the_account_form() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        assert_eq!(
            account::interest_policy(&app.db, id).unwrap(),
            account::InterestPolicy::Manual
        );

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('e'));
        walk_until!(
            matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Interest),
            press(&mut app, KeyCode::Tab)
        );
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        assert_ne!(
            account::interest_policy(&app.db, id).unwrap(),
            account::InterestPolicy::Manual
        );
        assert_eq!(
            app.accounts.rows()[0].policy,
            account::interest_policy(&app.db, id).unwrap()
        );
    }

    /// A card has no band to move between and no goals to divide interest
    /// among, so its form is two fields and neither selector is offered.
    /// The Color field end to end: the selector is cycled on the form, `Enter`
    /// writes it, and the row the Accounts screen redraws carries it. Nothing
    /// else on the row moves -- the same press must not rename or reband the
    /// account it recolors.
    #[test]
    fn e_on_the_accounts_screen_writes_the_color_it_was_left_on() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let before = app.accounts.selected().unwrap().clone();
        assert_eq!(
            account::get(&app.db, before.account.id()).unwrap().color,
            None,
            "an account starts with no color"
        );

        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Account(form)) = &mut app.modal else {
            panic!("e did not open the account form");
        };
        // The form opens on the shade the row is already drawn in, so one
        // step off it is the *next* color rather than the head of the list.
        let opening = account::AccountColor::derived(before.account.id());
        form.next_choice_on(accounts_screen::AccountField::Color);
        let picked = form
            .color_choice()
            .expect("one step off a color is a color");
        assert_ne!(picked, opening, "the step did not move");
        press(&mut app, KeyCode::Enter);

        let after = app
            .accounts
            .rows()
            .iter()
            .find(|r| r.account.id() == before.account.id())
            .expect("the account is gone");
        assert_eq!(after.account.text(), before.account.text());
        assert_eq!(after.group, before.group);
        assert_eq!(
            account::get(&app.db, before.account.id()).unwrap().color,
            Some(picked),
            "the color did not reach the database"
        );
    }

    /// The one cost of opening on the derived shade: Enter on an untouched
    /// form writes it down. Nothing on screen changes, because it is the
    /// shade the row was already drawn in -- what it gives up is the stored
    /// difference between "not chosen" and "chosen to be what it already
    /// was", which no screen shows.
    #[test]
    fn enter_on_an_untouched_account_form_pins_the_derived_color() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let id = app.accounts.selected().unwrap().account.id();
        assert_eq!(account::get(&app.db, id).unwrap().color, None);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            account::get(&app.db, id).unwrap().color,
            Some(account::AccountColor::derived(id))
        );
        // And the row is drawn in exactly the shade it was before.
        assert_eq!(
            crate::tui::style::account_color(id, Some(account::AccountColor::derived(id))),
            crate::tui::style::account_color(id, None)
        );
    }

    /// `—` is a choice the owner can take back, so cycling the whole way
    /// round and saving has to leave the account exactly as it was found.
    #[test]
    fn a_color_can_be_cleared_from_the_accounts_screen() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        let id = app.accounts.selected().unwrap().account.id();
        account::set_color(&app.db, id, Some(account::AccountColor::Rose)).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('e'));
        let Some(Modal::Account(form)) = &mut app.modal else {
            panic!("e did not open the account form");
        };
        // Step round to `—` rather than counting to it: how long the cycle
        // is, is the form's business and not this test's.
        for _ in 0..=account::AccountColor::ALL.len() {
            if form
                .display(accounts_screen::AccountField::Color)
                .plain_text()
                == "—"
            {
                break;
            }
            form.next_choice_on(accounts_screen::AccountField::Color);
        }
        assert_eq!(
            form.display(accounts_screen::AccountField::Color)
                .plain_text(),
            "—"
        );
        press(&mut app, KeyCode::Enter);

        assert_eq!(account::get(&app.db, id).unwrap().color, None);
    }

    #[test]
    fn a_card_gets_a_shorter_account_form() {
        let mut app = app();
        press(&mut app, KeyCode::Char('9'));
        walk_until!(
            app.accounts.selected().unwrap().kind == Kind::Credit,
            press(&mut app, KeyCode::Down)
        );
        press(&mut app, KeyCode::Char('e'));

        let Some(Modal::Account(form)) = &app.modal else {
            panic!("e did not open the account form");
        };
        assert_eq!(
            form.fields(),
            vec![
                accounts_screen::AccountField::Name,
                // Color survives the trim: a card is named on the Credit
                // ledger and on Recurring Transactions, so it is tinted
                // there, and the choice belongs to every account.
                accounts_screen::AccountField::Color,
                accounts_screen::AccountField::Order
            ]
        );
    }

    /// The block mapping is what `mm import` waits on, and screen 9 is the
    /// only place it can be set. One selector per account, so an account
    /// cannot claim both blocks -- and moving it off a block clears that
    /// block's key rather than leaving it naming an account that no longer
    /// answers for it.
    #[test]
    fn the_account_form_points_a_savings_block_at_an_account_and_off_again() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        assert!(
            setting::get(&app.db, SavingsBlock::Goals.key())
                .unwrap()
                .is_none()
        );

        let pick_block = |app: &mut App, steps: usize| {
            press(app, KeyCode::Char('9'));
            press(app, KeyCode::Char('e'));
            walk_until!(
                matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Savings),
                press(app, KeyCode::Tab)
            );
            for _ in 0..steps {
                press(app, KeyCode::Right);
            }
            press(app, KeyCode::Enter);
        };

        // One step off "neither" is the first block.
        pick_block(&mut app, 1);
        assert_eq!(
            setting::get(&app.db, SavingsBlock::Goals.key()).unwrap(),
            Some(id)
        );
        assert_eq!(
            app.accounts.rows()[0].block,
            Some(SavingsBlock::Goals),
            "the screen does not show what was written"
        );

        // A full cycle back to "neither" clears it again.
        pick_block(&mut app, SavingsBlock::ALL.len());
        assert!(
            setting::get(&app.db, SavingsBlock::Goals.key())
                .unwrap()
                .is_none(),
            "moving off a block left its key naming the account"
        );
        assert_eq!(app.accounts.rows()[0].block, None);
    }

    /// Screen 9 is where the two money forms are told where to open, and the
    /// set is what the account answers for: a source dropped from it clears
    /// that key rather than leaving it naming an account the owner has just
    /// taken it off.
    #[test]
    fn the_account_form_points_a_money_form_at_an_account_and_off_again() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;
        for source in Source::ALL {
            assert!(setting::get(&app.db, source.key()).unwrap().is_none());
        }

        // One step off "neither" is the first source on its own; a full
        // cycle back is "neither" again.
        pick_default(&mut app, 1);
        assert_eq!(
            setting::get(&app.db, Source::Transfer.key()).unwrap(),
            Some(id)
        );
        assert!(
            setting::get(&app.db, Source::Payment.key())
                .unwrap()
                .is_none(),
            "one step claimed both forms"
        );
        assert_eq!(
            app.accounts.rows()[0].defaults,
            vec![Source::Transfer],
            "the screen does not show what was written"
        );

        pick_default(&mut app, default_choices_len() - 1);
        for source in Source::ALL {
            assert!(
                setting::get(&app.db, source.key()).unwrap().is_none(),
                "{source:?} was left naming the account it was taken off"
            );
        }
        assert_eq!(app.accounts.rows()[0].defaults, Vec::new());
    }

    /// Both keys at once, which is the state the `Savings` selector beside it
    /// deliberately cannot reach: paying a card and moving savings are two
    /// decisions, and one account is allowed to answer both.
    #[test]
    fn one_account_can_be_the_default_for_both_money_forms() {
        let mut app = app();
        let id = account::list_by_kind(&app.db, Kind::Cash).unwrap()[0].id;

        pick_default(&mut app, default_choices_len() - 1);

        for source in Source::ALL {
            assert_eq!(setting::get(&app.db, source.key()).unwrap(), Some(id));
        }
        assert_eq!(app.accounts.rows()[0].defaults, Source::ALL.to_vec());
    }

    /// At most one account per key: pointing a source at a second account
    /// takes it off the first, because the key holds one id.
    #[test]
    fn pointing_a_money_form_at_a_second_account_takes_it_off_the_first() {
        let mut app = app();
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        setting::set(&app.db, Source::Transfer.key(), cash[0].id).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Down);
        edit_default(&mut app, 1);

        assert_eq!(
            setting::get(&app.db, Source::Transfer.key()).unwrap(),
            Some(cash[1].id)
        );
        assert_eq!(app.accounts.rows()[0].defaults, Vec::new());
        assert_eq!(app.accounts.rows()[1].defaults, vec![Source::Transfer]);
    }

    /// Editing one account must not disturb the other account's defaults:
    /// each key is only ever cleared when this account is the one it names.
    #[test]
    fn editing_one_account_leaves_another_accounts_defaults_alone() {
        let mut app = app();
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        setting::set(&app.db, Source::Payment.key(), cash[1].id).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, Source::Payment.key()).unwrap(),
            Some(cash[1].id)
        );
    }

    /// How many subsets the `Default` selector cycles, which is what a full
    /// lap of it costs.
    fn default_choices_len() -> usize {
        1 << Source::ALL.len()
    }

    /// Open `e` on the selected account, step its `Default` selector, save.
    fn edit_default(app: &mut App, steps: usize) {
        press(app, KeyCode::Char('e'));
        walk_until!(
            matches!(&app.modal, Some(Modal::Account(f)) if f.focus == accounts_screen::AccountField::Default),
            press(app, KeyCode::Tab)
        );
        for _ in 0..steps {
            press(app, KeyCode::Right);
        }
        press(app, KeyCode::Enter);
    }

    /// The same, from whichever screen the test is on, on the first account.
    fn pick_default(app: &mut App, steps: usize) {
        press(app, KeyCode::Char('9'));
        edit_default(app, steps);
    }

    /// Editing one account must not disturb the other block's mapping: the
    /// key is only ever cleared when this account is the one it names.
    #[test]
    fn editing_one_account_leaves_the_other_blocks_container_alone() {
        let mut app = app();
        let cash = account::list_by_kind(&app.db, Kind::Cash).unwrap();
        assert!(cash.len() > 1, "the fixture needs two cash accounts");
        setting::set(&app.db, SavingsBlock::Buckets.key(), cash[1].id).unwrap();
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('9'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, SavingsBlock::Buckets.key()).unwrap(),
            Some(cash[1].id)
        );
    }
}
