//! An account on its way to a display, in any medium.
//!
//! [`Account`] holds an account's id, the text a screen shows for it, and the
//! owner's color, with [`Account::render_with`] the only reader of the text --
//! it always hands the resolved color alongside, so there is no route to the
//! text that skips the color. [`Label`] carries the same guarantee for a
//! title: a sequence of plain runs and `Account`s rather than a `String`, so
//! an account segment stays tinted instead of being flattened before
//! anything can draw it. [`Label::plain_text`] is the one escape from that,
//! kept for the assertions that check wording and never meant to feed a draw.
//!
//! Two sinks turn the pair into a medium: `tui::label::account_cell` and
//! `label_line` for the terminal, and `report::html` for the report. Both
//! reach the text only through `render_with`.
//!
//! That is a stronger arrangement than a helper every screen is supposed to
//! remember. The tables all did remember; the titles and the form selectors
//! did not, and every one of them went wrong the same way -- a `format!` that
//! turned the account into a `String` before any render function could see
//! it. `db::account::AccountName` has no `Display` for the same reason, one
//! layer down.

use crate::db::AccountId;
use crate::db::account::{self, AccountColor};

/// How many characters of a name a widened label spends before it elides.
///
/// A name is owner-entered and bounded by nothing, and a column truncates
/// from the right -- so a long enough name would push the kind off the end of
/// the very label the kind is the point of, leaving two rows sharing a prefix
/// and nothing else. Eliding the name instead keeps the suffix whole however
/// long the name is.
///
/// The number is what leaves the kind, and every other column, whole on the
/// one screen that draws a widened label at `tui::MIN_WIDTH`;
/// `a_widened_account_column_keeps_its_kind_when_the_name_is_far_too_long`
/// is the guard, and it is a screen's test rather than this module's because
/// it is a screen that has the width.
const NAME_CAP: usize = 24;

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
    /// [`AccountColor::derived`] has a shade for every id.
    pub fn named(accounts: &[account::Account], id: AccountId) -> Account {
        Account::look_up(accounts, id, |a| a.name.as_str().to_string())
    }

    /// `CHK` -- Recurring Transactions, whose other columns pin a row down
    /// already, so the code alone says which account and reads better tight
    /// than padded. Also the ledger title, whose account is a filter term.
    ///
    /// `Everyday — Cash` where the code names more than one account in the
    /// list, under the rule [`Account::distinctly`] states.
    pub fn coded(accounts: &[account::Account], id: AccountId) -> Account {
        Account::distinctly(accounts, id, |a| a.code.as_str().to_string())
    }

    /// `CHK — Everyday` -- the three form selectors, which have a whole
    /// field's width where a column has none.
    ///
    /// One segment rather than two: both halves name the same account, and
    /// splitting them would leave the code reading as chrome in front of a
    /// colored name.
    ///
    /// An account whose name is still its code shows that text once. The
    /// import names every new account after its own code, so `CHK — CHK` is
    /// what this reads as until the owner renames the row on the Accounts
    /// screen -- and a selector saying the same word twice reads as two
    /// facts about the account rather than one it has yet to be told.
    ///
    /// The comparison folds case, the way `account::by_code` and the index
    /// under it do: `CHK` and `Chk` are one code as far as an import's match
    /// is concerned, so `CHK — Chk` is the same word said twice. The half
    /// drawn is then the *name*, since that is the one the owner typed.
    /// `Everyday — Cash` where both halves together still name more than one
    /// account in the list, under the rule [`Account::distinctly`] states.
    /// The collapse above is what makes that reachable: a freshly imported
    /// pair sharing a code is a pair sharing this label too.
    pub fn labelled(accounts: &[account::Account], id: AccountId) -> Account {
        Account::distinctly(accounts, id, |a| {
            let code = a.code.as_str();
            let name = a.name.as_str();
            if code.eq_ignore_ascii_case(name) {
                name.to_string()
            } else {
                format!("{code} — {name}")
            }
        })
    }

    /// An account whose row the caller already has -- the transfers a payday
    /// would write, which `transfer::plan` builds from the account rows it
    /// has just read.
    ///
    /// The three above look an id up in a slice because that is what their
    /// callers hold. This one takes the name and the color directly, so a
    /// caller holding the row does not read it a second time to get back the
    /// two fields it is already carrying.
    pub fn held(id: AccountId, text: impl Into<String>, color: Option<AccountColor>) -> Account {
        Account {
            id,
            text: text.into(),
            color,
        }
    }

    pub fn id(&self) -> AccountId {
        self.id
    }

    /// The only reader of an account's text, and it never yields the text
    /// without the color beside it.
    ///
    /// Both sinks go through here: the ratatui cell in `tui::label`, and the
    /// report's HTML. That is what this type is for -- a `format!` that
    /// flattened an account into a `String` is how every uncolored title on
    /// this app got that way, and there is no route to the text that does not
    /// pass the color along with it.
    ///
    /// The color handed over is *resolved*: the owner's choice, or
    /// [`AccountColor::derived`] when there is none. An unset color is a
    /// supported state rather than a gap, so a sink handed the raw `Option`
    /// would be a sink that could forget the fallback.
    ///
    /// This is also the one place a demo has to reach to cover every account
    /// display in the app: `crate::demo::text` stands between the stored text
    /// and every sink, so a screen or the report drawing an account through
    /// here draws its pseudonym without asking.
    pub fn render_with<T>(&self, f: impl FnOnce(&str, AccountColor) -> T) -> T {
        let text = crate::demo::text(&self.text);
        f(
            &text,
            self.color.unwrap_or_else(|| AccountColor::derived(self.id)),
        )
    }

    /// The text, for assertions.
    ///
    /// `pub` only in a test build, so it is not a route to an uncolored
    /// account in one that ships.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// [`Account::look_up`], with the text widened to `Name — Kind` when
    /// another account in the list would draw the very same thing.
    ///
    /// The fallback always separates them, and `UNIQUE (code, kind)` is why:
    /// two accounts one code cannot tell apart share that code, so they
    /// differ in kind. Two that share a name and a kind differ in code, and
    /// so never collide here in the first place.
    ///
    /// One rule over both spellings rather than a third constructor beside
    /// them. Which half of an account a screen shows stays the screen's
    /// business; whether that half says which account is not, and the screen
    /// that got it wrong is the one screen whose list mixes both kinds.
    ///
    /// The widened text stays one segment, for the reason [`Account::labelled`]
    /// gives about its own two halves: they name one account between them, and
    /// splitting the kind off would leave it reading as chrome beside a
    /// colored name.
    fn distinctly(
        accounts: &[account::Account],
        id: AccountId,
        text: impl Fn(&account::Account) -> String,
    ) -> Account {
        let drawn = Account::look_up(accounts, id, &text);
        let Some(row) = accounts.iter().find(|a| a.id == id) else {
            return drawn;
        };
        // Folded, the way `account::by_code` and the index under it fold: a
        // code retyped in another case is one code everywhere else.
        let shared = accounts
            .iter()
            .any(|a| a.id != id && text(a).eq_ignore_ascii_case(&drawn.text));
        match shared {
            true => Account {
                text: format!("{} — {}", elided(row.name.as_str()), row.kind.label()),
                ..drawn
            },
            false => drawn,
        }
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

/// A name cut to [`NAME_CAP`] characters, the last of them a `…`.
///
/// Characters rather than bytes: the cut lands inside a name the owner typed,
/// and a byte index would land inside one of its characters.
fn elided(name: &str) -> String {
    match name.chars().count() > NAME_CAP {
        true => name
            .chars()
            .take(NAME_CAP - 1)
            .chain(std::iter::once('…'))
            .collect(),
        false => name.to_string(),
    }
}

/// A string a view-state type may return that still knows which of its words
/// are accounts.
///
/// A title cannot be a `String` and be tinted: `format!` flattens the account
/// into text and the color is gone before any render function is reached,
/// which is how every uncolored title on this app got that way. It cannot be
/// a ratatui `Line` either -- view-state types hold no ratatui, so
/// `Savings::title` could not return one. So it is neither: a sequence of
/// plain runs and [`Account`]s, which a sink turns into its own medium's
/// spans, and which a test can read without a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label(Vec<Segment>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
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

    /// The runs this label is made of, for a sink that turns them into its
    /// own medium's spans. The only reader outside this module is a sink;
    /// anything else wanting the words wants [`Label::plain_text`], which
    /// says in its own documentation why that is a deliberate flattening.
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    /// The whole label as text, for the assertions that check wording.
    ///
    /// Not a `Display` impl: this type exists to stop an account being
    /// flattened by accident, and a `Display` on the thing that holds one
    /// would put the flattening back within reach of a `format!`. This
    /// method is that flattening, made deliberate rather than accidental --
    /// it drops every account segment's color on the floor, so a caller that
    /// feeds its result to a draw has recreated the exact bug this module
    /// exists to make unwritable. Test assertions are the one legitimate use;
    /// a production render must go through [`Account::render_with`] and its
    /// sink instead.
    pub fn plain_text(&self) -> String {
        self.0
            .iter()
            .map(|s| match s {
                Segment::Plain(text) => text.clone(),
                Segment::Account(account) => account.render_with(|text, _| text.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{AccountColor, Group, Kind};
    use crate::test_support::{cash, credit};

    fn account_row(
        id: i64,
        code: &str,
        name: &str,
        color: Option<AccountColor>,
    ) -> account::Account {
        account::Account {
            id: AccountId(id),
            code: code.into(),
            name: name.into(),
            kind: Kind::Cash,
            sort: 0,
            group: Group::Checking,
            color,
        }
    }

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                group: Group::Checking,
                ..cash(1, "CHK")
            },
            account::Account {
                color: Some(AccountColor::Teal),
                ..cash(2, "NST")
            },
        ]
    }

    /// An unset color is a supported state, not a gap: a freshly imported
    /// database is already distinguishable, and the field is an override
    /// rather than a step the owner has to complete first.
    #[test]
    fn an_account_with_no_color_of_its_own_resolves_to_the_derived_one() {
        let accounts = vec![account_row(5, "CHK", "Everyday", None)];
        let account = Account::named(&accounts, AccountId(5));
        let color = account.render_with(|_, color| color);
        assert_eq!(color, AccountColor::derived(AccountId(5)));
    }

    #[test]
    fn an_account_with_a_chosen_color_resolves_to_that_one() {
        let accounts = vec![account_row(5, "CHK", "Everyday", Some(AccountColor::Rose))];
        let account = Account::named(&accounts, AccountId(5));
        let color = account.render_with(|_, color| color);
        assert_eq!(color, AccountColor::Rose);
    }

    /// A corrupt row is not a reason to stop drawing a screen, and every id
    /// has a shade.
    #[test]
    fn an_id_with_no_account_draws_as_a_question_mark_and_still_takes_a_color() {
        let account = Account::named(&[], AccountId(5));
        let text = account.render_with(|text, _| text.to_string());
        assert_eq!(text, "?");
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
        assert_eq!(
            named.render_with(|_, color| color),
            coded.render_with(|_, color| color)
        );
    }

    /// The three form selectors show both halves in one cell, so both halves
    /// are one segment and take one color -- splitting them would leave the
    /// code reading as chrome in front of a colored name.
    #[test]
    fn a_selectors_label_shows_the_code_and_the_name_as_one_account() {
        assert_eq!(
            Account::labelled(&accounts(), AccountId(1)).text(),
            "CHK — Everyday"
        );
    }

    /// The state every account is imported in: `name = code`, until the owner
    /// renames the row. Both halves are the same word, so the selector says
    /// it once rather than joining it to itself.
    #[test]
    fn a_selectors_label_says_a_name_that_is_still_its_code_once() {
        let mut all = accounts();
        all[0].name = all[0].code.as_str().into();
        assert_eq!(Account::labelled(&all, AccountId(1)).text(), "CHK");
    }

    /// A code and a name differing only in case are one code everywhere else
    /// -- `account::by_code` folds it, and so does the index under it -- so
    /// they are one word here too. The name is what gets drawn: it is the
    /// half the owner typed.
    #[test]
    fn a_selectors_label_says_a_name_that_only_cases_its_code_differently_once() {
        let mut all = accounts();
        all[0].name = "Chk".into();
        assert_eq!(Account::labelled(&all, AccountId(1)).text(), "Chk");
    }

    /// The Recurring Transactions screen is the one list that mixes both
    /// kinds, and `UNIQUE (code, kind)` is per kind -- so one code can name
    /// two rows on it. When it does, neither is drawn by the code they share.
    #[test]
    fn a_code_two_kinds_share_draws_the_name_and_the_kind_instead() {
        let all = vec![cash(1, "CHK"), credit(2, "CHK")];
        assert_eq!(Account::coded(&all, AccountId(1)).text(), "Everyday — Cash");
        assert_eq!(
            Account::coded(&all, AccountId(2)).text(),
            "Everyday Card — Credit"
        );
    }

    /// The widening is per account, not per list: a code no other account
    /// holds is still the tightest thing that names its row.
    #[test]
    fn a_code_only_one_account_holds_is_still_drawn_alone() {
        let all = vec![cash(1, "CHK"), credit(2, "CC1")];
        assert_eq!(Account::coded(&all, AccountId(1)).text(), "CHK");
        assert_eq!(Account::coded(&all, AccountId(2)).text(), "CC1");
    }

    /// Two accounts a code cannot tell apart share that code, and a shared
    /// code means differing kinds -- so the kind always separates them.
    #[test]
    fn a_code_two_kinds_share_case_insensitively_is_still_a_shared_code() {
        let restyled = account::Account {
            code: "chk".into(),
            ..credit(2, "CHK")
        };
        let all = vec![cash(1, "CHK"), restyled];
        assert_eq!(Account::coded(&all, AccountId(1)).text(), "Everyday — Cash");
    }

    /// The state every account is imported in is the state the collapse in
    /// `labelled` hides: `name = code` on both rows leaves one word naming
    /// two accounts, which is the whole reason a selector spanning both kinds
    /// cannot stop at the code and the name.
    #[test]
    fn a_selectors_label_two_kinds_would_share_draws_the_name_and_the_kind_instead() {
        let unnamed = |a: account::Account| account::Account {
            name: a.code.as_str().into(),
            ..a
        };
        let all = vec![unnamed(cash(1, "CHK")), unnamed(credit(2, "CHK"))];
        assert_eq!(Account::labelled(&all, AccountId(1)).text(), "CHK — Cash");
        assert_eq!(Account::labelled(&all, AccountId(2)).text(), "CHK — Credit");
    }

    /// The kind is the half that says which of the two an account is, so it
    /// is the half that has to survive a name too long for the column. Left
    /// to the column's own truncation the suffix goes first, and two rows
    /// sharing a long name come back to sharing a prefix and nothing else --
    /// the state the widening exists to leave.
    #[test]
    fn a_widened_label_elides_the_name_rather_than_losing_the_kind() {
        let all = vec![
            account::Account {
                name: "Everyday Household Spending And Bills".into(),
                ..cash(1, "CHK")
            },
            credit(2, "CHK"),
        ];
        let text = Account::coded(&all, AccountId(1)).text().to_string();
        let (name, kind) = text
            .rsplit_once(" — ")
            .unwrap_or_else(|| panic!("no kind on {text:?}"));
        assert_eq!(kind, "Cash");
        assert_eq!(name.chars().count(), NAME_CAP);
        assert!(name.ends_with('…'), "{name:?}");
    }

    /// Two accounts one code names are still told apart by their names, and
    /// a selector with the width for both halves says so without the kind.
    #[test]
    fn a_selectors_label_no_other_account_would_draw_is_left_alone() {
        let all = vec![cash(1, "CHK"), credit(2, "CHK")];
        assert_eq!(
            Account::labelled(&all, AccountId(1)).text(),
            "CHK — Everyday"
        );
    }

    /// An id with no account is a corrupt row, not a reason to stop drawing
    /// the screen -- so it renders, and it still gets a color.
    #[test]
    fn an_id_with_no_account_still_draws_and_still_has_a_color() {
        let missing = Account::named(&accounts(), AccountId(99));
        assert_eq!(missing.text(), "?");
        assert_eq!(
            missing.render_with(|_, color| color),
            AccountColor::derived(AccountId(99))
        );
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
}
