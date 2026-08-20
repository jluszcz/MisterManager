//! An account on its way to the screen, and the one place it gets its color.
//!
//! `style::account_color` decides what an account looks like; this module is
//! what makes that decision unavoidable. [`Account`] holds an account's id,
//! the text a screen shows for it, and the owner's color, with **no reader
//! for the text outside this file**. So the only route from an account to a
//! glyph is [`account_cell`], which colors what it draws, and a screen that
//! wants an uncolored account has no way to ask for one.
//!
//! That is a stronger arrangement than a helper every screen is supposed to
//! remember. The tables all did remember; the titles and the form selectors
//! did not, and every one of them went wrong the same way -- a `format!` that
//! turned the account into a `String` before any render function could see
//! it. `db::account::AccountName` has no `Display` for the same reason, one
//! layer down.

use crate::db::AccountId;
use crate::db::account::{self, AccountColor};
use ratatui::text::Line as TextLine;
use ratatui::widgets::Cell;

use super::style;

/// An account as a screen shows it: which account, what text, what color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    id: AccountId,
    text: String,
    color: Option<AccountColor>,
}

impl Account {
    /// `Everyday` -- the ledgers, Savings, Overview and the Accounts screen.
    ///
    /// An id with no account draws as `?` and still takes a color: a corrupt
    /// row is not a reason to stop drawing a screen, and
    /// [`style::account_color`] has a shade for every id.
    pub fn named(accounts: &[account::Account], id: AccountId) -> Account {
        Account::look_up(accounts, id, |a| a.name.as_str().to_string())
    }

    /// `CHK` -- Recurring Transactions, whose other columns pin a row down
    /// already, so the code alone says which account and reads better tight
    /// than padded. Also the ledger title, whose account is a filter term.
    pub fn coded(accounts: &[account::Account], id: AccountId) -> Account {
        Account::look_up(accounts, id, |a| a.code.as_str().to_string())
    }

    /// `CHK — Everyday` -- the three form selectors, which have a whole
    /// field's width where a column has none.
    ///
    /// One segment rather than two: both halves name the same account, and
    /// splitting them would leave the code reading as chrome in front of a
    /// colored name.
    pub fn labelled(account: &account::Account) -> Account {
        Account {
            id: account.id,
            text: format!("{} — {}", account.code.as_str(), account.name.as_str()),
            color: account.color,
        }
    }

    pub fn id(&self) -> AccountId {
        self.id
    }

    /// The text, private to this file.
    ///
    /// Every caller is one of this module's own. A reader outside it would be
    /// a way to draw an account without its color, which is the whole of what
    /// this type prevents.
    fn as_text(&self) -> &str {
        &self.text
    }

    /// The text, for assertions.
    ///
    /// `pub` only in a test build, so it is not a route to an uncolored
    /// account in one that ships.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        self.as_text()
    }

    /// The color this account draws in, wherever it draws.
    fn color(&self) -> style::Color {
        style::account_color(self.id, self.color)
    }

    fn look_up(
        accounts: &[account::Account],
        id: AccountId,
        text: impl Fn(&account::Account) -> String,
    ) -> Account {
        match accounts.iter().find(|a| a.id == id) {
            Some(a) => Account {
                id,
                text: text(a),
                color: a.color,
            },
            None => Account {
                id,
                text: "?".to_string(),
                color: None,
            },
        }
    }
}

/// A cell naming an account, colored by [`style::account_color`].
///
/// One of this module's two exits, and it offers no way to skip the color.
pub(super) fn account_cell(account: &Account) -> Cell<'static> {
    super::tinted(
        TextLine::from(account.as_text().to_string()),
        Some(account.color()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{Group, Kind};

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "CHK".into(),
                name: "Everyday".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Checking,
                color: None,
            },
            account::Account {
                id: AccountId(2),
                code: "NST".into(),
                name: "Nest Egg".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: Some(AccountColor::Teal),
            },
        ]
    }

    /// Which half of an account a screen shows is the screen's business --
    /// Recurring Transactions has room for a code and no more -- but the id
    /// and the color travel with it either way, which is what makes the same
    /// account one color across all of them.
    #[test]
    fn an_account_carries_its_id_and_color_whichever_half_it_shows() {
        let all = accounts();
        let named = Account::named(&all, AccountId(2));
        let coded = Account::coded(&all, AccountId(2));
        assert_eq!(named.text(), "Nest Egg");
        assert_eq!(coded.text(), "NST");
        assert_eq!(named.id(), AccountId(2));
        assert_eq!(cell_color(&named), cell_color(&coded));
    }

    /// The three form selectors show both halves in one cell, so both halves
    /// are one segment and take one color -- splitting them would leave the
    /// code reading as chrome in front of a colored name.
    #[test]
    fn a_selectors_label_shows_the_code_and_the_name_as_one_account() {
        assert_eq!(Account::labelled(&accounts()[0]).text(), "CHK — Everyday");
    }

    /// An id with no account is a corrupt row, not a reason to stop drawing
    /// the screen -- so it renders, and it still gets a color.
    #[test]
    fn an_id_with_no_account_still_draws_and_still_has_a_color() {
        let missing = Account::named(&accounts(), AccountId(99));
        assert_eq!(missing.text(), "?");
        assert_eq!(
            cell_color(&missing),
            Some(style::account_color(AccountId(99), None))
        );
    }

    /// The whole guarantee, at the one place it is discharged: the cell an
    /// account draws into is colored, always, with no way for a caller to ask
    /// for an uncolored one.
    #[test]
    fn every_account_cell_is_colored() {
        let all = accounts();
        for id in [AccountId(1), AccountId(2), AccountId(99)] {
            assert!(cell_color(&Account::named(&all, id)).is_some(), "{id}");
        }
    }

    /// The owner's choice outranks the derived shade here exactly as it does
    /// in `style::account_color` -- this is the plumbing, not a second
    /// decision about what an account looks like.
    #[test]
    fn a_chosen_color_reaches_the_cell() {
        assert_eq!(
            cell_color(&Account::named(&accounts(), AccountId(2))),
            Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
        );
    }

    /// Reads the foreground the cell's first glyph draws in.
    ///
    /// Through a rendered buffer rather than off the `Cell`, because a
    /// `Cell`'s spans are private -- and because the buffer is what the
    /// terminal actually shows, which is the thing under test. The same trick
    /// `savings.rs` and `recurring_txn.rs` already use to read a column's
    /// color back.
    fn cell_color(account: &Account) -> Option<style::Color> {
        use ratatui::layout::{Constraint, Rect};
        use ratatui::widgets::{Row, Table, Widget};
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        Table::new(
            vec![Row::new(vec![account_cell(account)])],
            [Constraint::Length(20)],
        )
        .render(area, &mut buffer);
        buffer[(0, 0)].style().fg
    }
}
