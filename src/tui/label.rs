//! An account on its way to the screen, and the one place it gets its color.
//!
//! `style::account_color` decides what an account looks like; this module is
//! what makes that decision unavoidable. [`Account`] holds an account's id,
//! the text a screen shows for it, and the owner's color, with **no reader
//! for the text outside this file**. So the only routes from an account to a
//! glyph are [`account_cell`], which colors a table cell, and [`Label`]
//! through [`label_line`], which does the same for a title -- and a screen
//! that wants an uncolored account has no way to ask for one.
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

/// A string a view-state type may return that still knows which of its words
/// are accounts.
///
/// A title cannot be a `String` and be tinted: `format!` flattens the account
/// into text and the color is gone before any render function is reached,
/// which is how every uncolored title on this app got that way. It cannot be
/// a ratatui `Line` either -- view-state types hold no ratatui, so
/// `Savings::title` could not return one. So it is neither: a sequence of
/// plain runs and [`Account`]s, which [`label_line`] turns into spans and
/// which a test can read without a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label(Vec<Segment>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Plain(String),
    Account(Account),
}

impl Label {
    /// Opens a label with some text. [`Label::text`] appends more of it, and
    /// [`Label::account`] appends the one kind of segment that takes a color.
    pub fn plain(text: impl Into<String>) -> Label {
        Label(vec![Segment::Plain(text.into())])
    }

    pub fn text(mut self, text: impl Into<String>) -> Label {
        self.0.push(Segment::Plain(text.into()));
        self
    }

    pub fn account(mut self, account: Account) -> Label {
        self.0.push(Segment::Account(account));
        self
    }

    /// Puts plain text ahead of everything already in the label -- the one
    /// way to build a title that reads "verb, then a label built elsewhere",
    /// since `text` and `account` only ever add to the end.
    pub fn prepend(mut self, text: impl Into<String>) -> Label {
        self.0.insert(0, Segment::Plain(text.into()));
        self
    }

    /// The whole label as text, for the assertions that check wording.
    ///
    /// Not a `Display` impl: this type exists to stop an account being
    /// flattened by accident, and a `Display` on the thing that holds one
    /// would put the flattening back within reach of a `format!`.
    pub fn plain_text(&self) -> String {
        self.0
            .iter()
            .map(|s| match s {
                Segment::Plain(text) => text.as_str(),
                Segment::Account(account) => account.as_text(),
            })
            .collect()
    }

    /// The accounts this label names, for the assertions that check a title
    /// names one *as an account* rather than as text.
    pub fn accounts(&self) -> Vec<&Account> {
        self.0
            .iter()
            .filter_map(|s| match s {
                Segment::Account(account) => Some(account),
                Segment::Plain(_) => None,
            })
            .collect()
    }
}

impl From<&str> for Label {
    fn from(text: &str) -> Label {
        Label::plain(text)
    }
}

impl From<String> for Label {
    fn from(text: String) -> Label {
        Label::plain(text)
    }
}

/// A label as a line of spans, with every account segment in its own color
/// and nothing else colored at all.
///
/// The second of this module's two exits, beside [`account_cell`], and the
/// same guarantee: an account that reaches a screen through here is colored.
pub(super) fn label_line(label: &Label) -> TextLine<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    TextLine::from(
        label
            .0
            .iter()
            .map(|segment| match segment {
                Segment::Plain(text) => Span::raw(text.clone()),
                Segment::Account(account) => Span::styled(
                    account.as_text().to_string(),
                    Style::default().fg(account.color()),
                ),
            })
            .collect::<Vec<Span<'static>>>(),
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

    /// The point of the type: a title that names an account carries it as an
    /// account, so the render half can tint it. A title already flattened to a
    /// String could not be tinted by anything.
    #[test]
    fn a_label_keeps_its_accounts_apart_from_its_text() {
        let all = accounts();
        let label = Label::plain("Savings · ")
            .account(Account::named(&all, AccountId(2)))
            .text(" · Aug 2026");
        assert_eq!(label.plain_text(), "Savings · Nest Egg · Aug 2026");
        assert_eq!(label.accounts().len(), 1);
        assert_eq!(label.accounts()[0].id(), AccountId(2));
    }

    /// Exactly the account segments are colored. A title is mostly chrome, and
    /// chrome in an account's color would say the whole line is about that
    /// account rather than the two words that are.
    #[test]
    fn only_the_account_segments_of_a_label_are_colored() {
        let all = accounts();
        let label = Label::plain("Savings · ").account(Account::named(&all, AccountId(2)));
        let colors: Vec<Option<style::Color>> = label_line(&label)
            .spans
            .iter()
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(
            colors,
            vec![
                None,
                Some(style::account_color(AccountId(2), Some(AccountColor::Teal)))
            ]
        );
    }

    /// A title naming nothing is still a Label, so one function draws every
    /// title rather than one for the plain ones and one for the rest.
    #[test]
    fn a_label_with_no_account_draws_as_plain_text() {
        let line = label_line(&Label::from("Edit goal"));
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].style.fg, None);
    }
}
