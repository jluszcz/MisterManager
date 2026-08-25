//! The Accounts screen: what each account the workbook names is called here,
//! which Overview band it sits in, in what order, how an interest posting
//! against it is divided, and which block of the `Savings` sheet it is the
//! container for.
//!
//! The workbook carries a short code per account and nothing else. Everything
//! on this screen is therefore the owner's rather than the importer's, and
//! `account` is not an imported table, so none of it is lost to a
//! `--replace`. The last of the five is the one the import *reads*: until
//! both `Savings` blocks have been pointed at a container here, `mm import`
//! writes the accounts and stops.
//!
//! View state only -- no ratatui above the render functions at the bottom,
//! and no `Db` on the type. `App` runs the queries and hands the results in.

use super::Label;
use super::cursor::{Cursor, Viewport, impl_scroll};
use super::form::{Caret, Field, Focused, FormFields, Step, next_in, step_index};
use crate::db::AccountId;
use crate::db::account::{Account, AccountColor, Group, InterestPolicy, Kind};
use crate::savings_block::Block as SavingsBlock;
use anyhow::{Result, ensure};

/// How wide a code may be: the `Code` column's width, and the bound `a` holds
/// a typed one to.
///
/// One constant for the two, because a code wider than its column is cut in
/// the only place a code is ever drawn -- leaving the owner unable to read
/// the string the next import matches this row against, which is the one
/// thing the field is for. The column is sized by the codes rather than the
/// codes by the column, so widening it is what makes room for a longer one.
const CODE_WIDTH: u16 = 8;

/// One account as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub account: super::Account,
    pub code: String,
    pub kind: Kind,
    pub group: Group,
    pub policy: InterestPolicy,
    /// Which block of the `Savings` sheet this account is the container for,
    /// if either. The one thing on this screen that the import *reads* rather
    /// than merely leaves alone: without both blocks pointed somewhere,
    /// `mm import` stops after the accounts.
    pub block: Option<SavingsBlock>,
}

pub struct Accounts {
    rows: Vec<Row>,
    cursor: Cursor,
}

impl Accounts {
    pub fn new() -> Accounts {
        Accounts {
            rows: Vec::new(),
            cursor: Cursor::new(),
        }
    }

    /// Take every account, in `account::list` order -- which is the order the
    /// Overview stacks them in, so this screen is a reading of that one.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.cursor.clamp(self.rows.len());
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.cursor.index())
    }

    /// Where the selected account sits among the accounts of its own kind,
    /// and how many there are -- what the form's `Order` selector cycles.
    ///
    /// Derived from the rows the screen already holds rather than re-queried:
    /// `set_rows` takes `account::list` order, which is `kind, sort, code`,
    /// so the accounts of one kind are contiguous and in position order.
    pub fn position_of(&self, id: AccountId) -> Option<(usize, usize)> {
        let row = self.rows.iter().find(|r| r.account.id() == id)?;
        let of_kind: Vec<&Row> = self.rows.iter().filter(|r| r.kind == row.kind).collect();
        let position = of_kind.iter().position(|r| r.account.id() == id)?;
        Some((position, of_kind.len()))
    }

    pub fn title(&self) -> String {
        format!("Accounts · {}", self.rows.len())
    }
}

impl Default for Accounts {
    fn default() -> Accounts {
        Accounts::new()
    }
}

impl_scroll!(Accounts, rows);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccountField {
    /// `a` only. The short code the workbook names an account by, and what
    /// `account::by_code` matches the next import against -- which is why it
    /// is asked once, here, and never offered to `e`.
    Code,
    /// `a` only, for `Code`'s reason: the other half of what `by_code`
    /// matches on.
    Kind,
    Name,
    Color,
    Band,
    Order,
    Interest,
    Savings,
}

impl AccountField {
    pub fn label(self) -> &'static str {
        match self {
            AccountField::Code => "Code",
            AccountField::Kind => "Kind",
            AccountField::Name => "Name",
            AccountField::Color => "Color",
            AccountField::Band => "Band",
            AccountField::Order => "Order",
            AccountField::Interest => "Interest",
            AccountField::Savings => "Savings",
        }
    }
}

/// The `Savings` selector's choices: neither block, then one per block.
///
/// Built from `SavingsBlock::ALL` rather than written out, so a third block
/// added to the sheet reaches this screen without anyone remembering to come
/// here.
/// One selector per account rather than one per block is what makes an
/// account holding *both* blocks unrepresentable -- the field has one value.
fn savings_choices() -> Vec<Option<SavingsBlock>> {
    let mut choices = vec![None];
    choices.extend(SavingsBlock::ALL.map(Some));
    choices
}

/// The `Color` selector's choices: no color, then one per name.
///
/// `None` first because it is the state every account is stored in until
/// somebody picks -- it is not "no color" so much as "the shade the id
/// derives", which `AccountColor::derived` is what decides and
/// `style::account_color` what draws. The form does not *open* on it (see
/// [`AccountForm::edit`]); it is here so an owner who wants the derivation
/// back can cycle to it.
///
/// Built from `AccountColor::ALL` rather than written out, so a color added
/// to the palette reaches this screen without anyone remembering to come
/// here.
fn color_choices() -> Vec<Option<AccountColor>> {
    let mut choices = vec![None];
    choices.extend(AccountColor::ALL.map(Some));
    choices
}

/// What `e` commits: the whole of what the owner may say about an account.
///
/// The code and the kind are absent on purpose. Both come from the workbook
/// and are what `account::by_code` matches the next import against, so
/// editing either would orphan the row from the sheet that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountEdit {
    pub name: String,
    /// The color the name draws in, or `None` for the one its id derives.
    pub color: Option<AccountColor>,
    pub group: Group,
    /// Where among the accounts of this kind it should sit.
    pub position: usize,
    pub policy: InterestPolicy,
    /// Which `Savings` block this account holds, if either. `None` also
    /// *clears* a block this account used to hold -- the field has one value,
    /// so moving it off a block is the only way that block loses its
    /// container from here.
    pub block: Option<SavingsBlock>,
}

/// What `a` commits: an account the workbook has not named.
///
/// The code and the kind are the whole of it beyond a name, because they are
/// the two an account cannot be given later -- everything else on this screen
/// is a *placement*, and `e` is where an account is placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAccount {
    pub code: String,
    pub name: String,
    pub kind: Kind,
}

/// One account: `a` creating one, or `e` saying the rest about one that
/// exists. `editing` is which, the same shape `FundForm` and
/// `RecurringTxnForm` carry.
///
/// The two modes ask disjoint questions, and that is the point rather than an
/// accident. `a` asks the two things an account cannot be given afterwards --
/// the code and the kind, which are what `account::by_code` matches the next
/// import against, so editing either would orphan the row from the sheet that
/// produced it. `e` asks the six the workbook does not carry, none of which a
/// new account needs before it exists.
///
/// All but the name and the code are selectors rather than text, so a band
/// the schema's `CHECK` would refuse, a position off the end, a policy that is
/// not a policy, a kind that is not a kind, and an account claiming both
/// `Savings` blocks at once are all unrepresentable. Which fields an *edit*
/// shows depends on the kind, exactly as `FundForm`'s do on the target:
/// credit does not split into bands, so there is nothing for a band selector
/// to cycle, and only a cash account holds the goals an interest posting is
/// divided among or a `Savings` block fills.
#[derive(Debug)]
pub struct AccountForm {
    /// The account this form saves back to, or `None` for one `a` will
    /// insert.
    pub editing: Option<AccountId>,
    pub focus: AccountField,
    /// `a` only: `e` never shows the field, and the account's own code is
    /// what it would hold.
    code: Field,
    /// An index into [`Kind::ALL`] rather than a `Kind`, because `a` cycles
    /// it. An edit form opens on the account's own and never shows the field.
    kind: usize,
    name: Field,
    color: usize,
    band: usize,
    position: usize,
    /// How many accounts of this kind there are, which bounds `position`.
    of_kind: usize,
    policy: usize,
    block: usize,
}

impl AccountForm {
    /// A blank form for an account that does not exist yet.
    ///
    /// It opens on `Code` rather than on `Name`, which is where every other
    /// form opens: the code is the field the owner cannot come back and fix,
    /// so it is the one the caret is handed.
    pub fn add() -> AccountForm {
        AccountForm {
            editing: None,
            focus: AccountField::Code,
            code: Field::default(),
            kind: 0,
            name: Field::default(),
            // None of the six is on an add form. They are what `e` asks, and
            // an account takes its kind's defaults until it is asked: the
            // default band, no color, appended last, `NULL` interest policy,
            // and no `Savings` block.
            color: 0,
            band: 0,
            position: 0,
            of_kind: 1,
            policy: 0,
            block: 0,
        }
    }

    /// `position` and `of_kind` come from [`Accounts::position_of`].
    pub fn edit(
        account: &Account,
        policy: InterestPolicy,
        position: usize,
        of_kind: usize,
        block: Option<SavingsBlock>,
    ) -> AccountForm {
        AccountForm {
            editing: Some(account.id),
            focus: AccountField::Name,
            // Blank, not seeded: the code is not a field this form shows,
            // because it is what the next import matches the row against and
            // `a` is where it is said. Reading it here to fill a field nobody
            // sees would be an account's text taken as a bare string, which
            // is how one reaches a screen with no color on it. The *kind* is
            // still read, because it decides which of the six fields below
            // are worth asking about.
            code: Field::default(),
            kind: Kind::ALL
                .iter()
                .position(|k| *k == account.kind)
                .unwrap_or(0),
            // seeds the form's editable Name field, not a display of an account
            name: Field::given(account.name.as_str().to_string()),
            // The color the account is *being drawn in*, which for one
            // nobody has picked for is the shade its id derives rather than
            // `—`. Opening on `—` meant the first `→` jumped to the head of
            // the palette from a row that was visibly some other color, so
            // the selector started somewhere the screen behind it contradicted
            // -- and stepping off a color is only a nudge if you start on it.
            //
            // The cost is that opening this form and pressing Enter writes
            // the derived shade down as a choice. Nothing on screen changes,
            // because it is the shade that was already being drawn; what it
            // gives up is the distinction between "not chosen" and "chosen to
            // be exactly what it already was", which no screen shows anyway.
            // `—` stays in the cycle for an owner who wants it back.
            color: color_choices()
                .iter()
                .position(|c| {
                    *c == Some(account.color.unwrap_or(AccountColor::derived(account.id)))
                })
                .unwrap_or(0),
            band: Group::bands(account.kind)
                .iter()
                .position(|g| *g == account.group)
                .unwrap_or(0),
            position,
            of_kind: of_kind.max(1),
            policy: InterestPolicy::ALL
                .iter()
                .position(|p| *p == policy)
                .unwrap_or(0),
            block: savings_choices()
                .iter()
                .position(|b| *b == block)
                .unwrap_or(0),
        }
    }

    /// The kind this form is working in: the account's own, or the one `a`'s
    /// selector is sitting on.
    pub fn kind(&self) -> Kind {
        Kind::ALL[self.kind]
    }

    /// The fields this form shows, which is also its tab order.
    ///
    /// An add form asks three and stops, and the kind decides none of them:
    /// the five it would decide are all *placements*, and an account is
    /// placed by `e` once it exists.
    pub fn fields(&self) -> Vec<AccountField> {
        if self.editing.is_none() {
            return vec![AccountField::Code, AccountField::Kind, AccountField::Name];
        }
        // Both kinds, unlike the three below it: a card is named on the
        // Credit ledger and on Recurring Transactions, so it is tinted there
        // too, and the choice belongs to every account rather than to the
        // cash ones. Beside the name because the two are one decision --
        // what this account looks like wherever it is named.
        let mut fields = vec![AccountField::Name, AccountField::Color];
        if Group::bands(self.kind()).len() > 1 {
            fields.push(AccountField::Band);
        }
        fields.push(AccountField::Order);
        if self.kind() == Kind::Cash {
            fields.push(AccountField::Interest);
            fields.push(AccountField::Savings);
        }
        fields
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => "Edit account — Tab field · ←/→ choice · Enter save · Esc cancel",
            None => "Add account — Tab field · ←/→ choice · Enter save · Esc cancel",
        }
    }

    pub fn display(&self, field: AccountField) -> Label {
        Label::plain(match field {
            AccountField::Code => crate::demo::text(self.code.value()).into_owned(),
            AccountField::Kind => self.kind().label().to_string(),
            AccountField::Name => crate::demo::text(self.name.value()).into_owned(),
            AccountField::Color => match color_choices()[self.color] {
                None => "—".to_string(),
                Some(color) => color.label().to_string(),
            },
            AccountField::Band => Group::bands(self.kind())[self.band].label().to_string(),
            // One-based, because it is a place in a list rather than an index.
            AccountField::Order => format!("{} of {}", self.position + 1, self.of_kind),
            AccountField::Interest => InterestPolicy::ALL[self.policy].label().to_string(),
            AccountField::Savings => match savings_choices()[self.block] {
                None => "—".to_string(),
                Some(block) => block.label().to_string(),
            },
        })
    }

    /// Cycle one particular field's selector, whatever the focus is. The
    /// tests use it to reach a field without pressing Tab first.
    pub fn next_choice_on(&mut self, field: AccountField) {
        let was = self.focus;
        self.focus = field;
        self.choice(Step::NEXT);
        self.focus = was;
    }

    /// The color choice the selector is sitting on, which is what the form
    /// draws its value in. `None` is `—`, and drawing *that* in the shade the
    /// id derives is the point rather than a fallback: what the row will look
    /// like is the question the field is asking, and `—` has an answer to it.
    pub fn color_choice(&self) -> Option<AccountColor> {
        color_choices()[self.color]
    }

    /// What `a` writes. Two fields and a selector, all three of them things
    /// an account cannot be given after the fact.
    pub fn commit_new(&self) -> Result<NewAccount> {
        let code = self.code.value().trim().to_string();
        ensure!(!code.is_empty(), "code must not be empty");
        ensure!(
            code.chars().count() <= usize::from(CODE_WIDTH),
            "code must be at most {CODE_WIDTH} characters"
        );
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        Ok(NewAccount {
            code,
            name,
            kind: self.kind(),
        })
    }

    pub fn commit(&self) -> Result<AccountEdit> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        Ok(AccountEdit {
            name,
            color: color_choices()[self.color],
            group: Group::bands(self.kind())[self.band],
            position: self.position,
            policy: InterestPolicy::ALL[self.policy],
            block: savings_choices()[self.block],
        })
    }
}

impl FormFields for AccountForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&self.fields(), self.focus, step);
    }

    fn cycle(&mut self, step: Step) {
        self.step_choice(step.direction());
    }

    // Selectors take no keystroke of their own, the same as the goal form's
    // Interest field: a choice spelled out one letter at a time can sit at
    // something that means nothing in between.
    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            AccountField::Code => Focused::Text(&mut self.code),
            AccountField::Name => Focused::Text(&mut self.name),
            AccountField::Kind
            | AccountField::Color
            | AccountField::Band
            | AccountField::Order
            | AccountField::Interest
            | AccountField::Savings => Focused::Selector,
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            AccountField::Code => Caret::in_field(&self.code),
            AccountField::Name => Caret::in_field(&self.name),
            AccountField::Kind
            | AccountField::Color
            | AccountField::Band
            | AccountField::Order
            | AccountField::Interest
            | AccountField::Savings => Caret::End,
        }
    }
}

impl AccountForm {
    fn step_choice(&mut self, step: isize) {
        match self.focus {
            AccountField::Code | AccountField::Name => {}
            AccountField::Kind => {
                self.kind = step_index(self.kind, Kind::ALL.len(), step);
            }
            AccountField::Color => {
                self.color = step_index(self.color, color_choices().len(), step);
            }
            AccountField::Band => {
                self.band = step_index(self.band, Group::bands(self.kind()).len(), step);
            }
            AccountField::Order => {
                self.position = step_index(self.position, self.of_kind, step);
            }
            AccountField::Interest => {
                self.policy = step_index(self.policy, InterestPolicy::ALL.len(), step);
            }
            AccountField::Savings => {
                self.block = step_index(self.block, savings_choices().len(), step);
            }
        }
    }
}

use super::form::{field_line, field_line_tinted, render_fields};
use super::{Chrome, account_cell, render_table};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Cell, Row as TableRow};

pub fn render_form(frame: &mut Frame, form: &AccountForm) {
    let lines: Vec<TextLine> = form
        .fields()
        .iter()
        .map(|f| {
            let value = form.display(*f);
            let focused = form.focus == *f;
            match f {
                // A name is not a color. Drawing the choice in the shade it
                // names is what makes the field answerable here rather than
                // by saving it and looking at the table behind the modal --
                // and it goes through `account_color` with the account's own
                // id, so `—` shows the derived shade the row will actually
                // take rather than nothing at all.
                // Only ever an edit form's field: the shade a `—` stands for
                // is derived from the account's id, and an account `a` has
                // not written yet has none to derive from. `fields()` is
                // what makes that so, and the fallback is what says it here.
                AccountField::Color => match form.editing {
                    Some(id) => field_line_tinted(
                        f.label(),
                        value.plain_text(),
                        focused,
                        super::style::account_color(id, form.color_choice()),
                    ),
                    None => field_line(f.label(), value, focused.then(|| form.caret())),
                },
                _ => field_line(f.label(), value, focused.then(|| form.caret())),
            }
        })
        .collect();
    render_fields(frame, form.title(), lines);
}

/// One row per account. Returns the [`Viewport`] it drew: the height
/// `PageUp`/`PageDown` move by, and the row the next draw starts from.
pub(super) fn render(frame: &mut Frame, area: Rect, accounts: &Accounts) -> Viewport {
    let mut rows: Vec<TableRow> = accounts
        .rows()
        .iter()
        .map(|r| {
            TableRow::new(vec![
                Cell::from(crate::demo::text(&r.code).into_owned()),
                account_cell(&r.account),
                Cell::from(r.kind.label()),
                Cell::from(r.group.label()),
                // Only a cash account holds goals, so only a cash account has
                // an interest posting to divide.
                Cell::from(match r.kind {
                    Kind::Cash => r.policy.label(),
                    Kind::Credit => "—",
                }),
                Cell::from(match r.block {
                    Some(block) => block.label(),
                    None => "—",
                }),
            ])
        })
        .collect();

    if rows.is_empty() {
        // Behind a blank `Code` cell, so the message lands in `Account` --
        // the one `Min` column, and the only one wide enough to hold a
        // sentence. A single cell would sit in `Code` and be cut to
        // `CODE_WIDTH`, which is a message that reports its own truncation.
        rows.push(TableRow::new(vec![
            Cell::from(""),
            Cell::from("no accounts yet — press a, or run mm import"),
        ]));
    }

    let header = TableRow::new(vec![
        Cell::from("Code"),
        Cell::from("Account"),
        Cell::from("Kind"),
        Cell::from("Band"),
        Cell::from("Interest"),
        Cell::from("Savings"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Length(CODE_WIDTH),
        Constraint::Min(16),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(8),
    ];

    // The placeholder an empty list draws is not a row the cursor may rest
    // on, so the drawn count is the list's own length rather than `rows`.
    render_table(
        frame,
        area,
        accounts,
        Chrome::titled(accounts.title()).header(header),
        &widths,
        rows,
        accounts.rows().len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::walk_until;
    use crate::tui::MIN_WIDTH;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::{backspace_key, char_key};

    fn row(id: i64, code: &str, name: &str, kind: Kind, group: Group) -> Row {
        let accounts = [Account {
            id: AccountId(id),
            code: code.into(),
            name: name.into(),
            kind,
            sort: 0,
            group,
            color: None,
        }];
        Row {
            account: super::super::Account::named(&accounts, AccountId(id)),
            code: code.to_string(),
            kind,
            group,
            policy: InterestPolicy::Manual,
            block: None,
        }
    }

    /// `account::list` order: kind, then sort, then code -- so the cash
    /// accounts are contiguous and in position order, which is what
    /// `position_of` reads.
    fn screen() -> Accounts {
        let mut accounts = Accounts::new();
        accounts.set_rows(vec![
            row(1, "CHK", "Everyday", Kind::Cash, Group::Checking),
            row(2, "SAV", "Rainy Day", Kind::Cash, Group::Savings),
            row(3, "BKR", "Brokerage", Kind::Cash, Group::Savings),
            row(4, "CC1", "Card One", Kind::Credit, Group::Credit),
            row(5, "CHK", "Everyday Card", Kind::Credit, Group::Credit),
        ]);
        accounts
    }

    fn account(id: i64, name: &str, kind: Kind, group: Group) -> Account {
        Account {
            id: AccountId(id),
            code: "CHK".into(),
            name: name.into(),
            kind,
            sort: 0,
            group,
            color: None,
        }
    }

    /// A position is a place among the accounts of one *kind*: the cash
    /// accounts count from zero and so do the cards, because `sort` is a
    /// per-kind column and the Overview bands are applied on top of it.
    #[test]
    fn a_position_counts_from_zero_within_its_own_kind() {
        let accounts = screen();
        assert_eq!(accounts.position_of(AccountId(1)), Some((0, 3)));
        assert_eq!(accounts.position_of(AccountId(3)), Some((2, 3)));
        assert_eq!(accounts.position_of(AccountId(5)), Some((1, 2)));
        assert_eq!(accounts.position_of(AccountId(99)), None);
    }

    /// Credit does not split into bands, so there is nothing for a band
    /// selector to cycle -- and only a cash account holds the goals an
    /// interest posting is divided among.
    #[test]
    fn the_form_offers_a_band_and_a_policy_only_where_they_mean_something() {
        let cash = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        assert_eq!(
            cash.fields(),
            vec![
                AccountField::Name,
                AccountField::Color,
                AccountField::Band,
                AccountField::Order,
                AccountField::Interest,
                AccountField::Savings
            ]
        );

        let card = AccountForm::edit(
            &account(4, "Card One", Kind::Credit, Group::Credit),
            InterestPolicy::Manual,
            0,
            2,
            None,
        );
        assert_eq!(
            card.fields(),
            vec![AccountField::Name, AccountField::Color, AccountField::Order]
        );
    }

    /// The selector offers exactly the bands `account::set_group` accepts, so
    /// a pick here cannot be refused by the write it leads to.
    #[test]
    fn the_band_selector_cycles_only_its_kinds_bands() {
        let mut form = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        assert_eq!(form.display(AccountField::Band).plain_text(), "Checking");
        form.next_choice_on(AccountField::Band);
        assert_eq!(form.display(AccountField::Band).plain_text(), "Savings");
        assert_eq!(form.commit().unwrap().group, Group::Savings);
        // Two bands, so a second step wraps back rather than reaching Credit.
        form.next_choice_on(AccountField::Band);
        assert_eq!(form.commit().unwrap().group, Group::Checking);
    }

    /// One-based on the form, zero-based in the write: it is a place in a
    /// list to whoever reads it and an index to `account::reorder`.
    #[test]
    fn the_order_selector_reads_one_based_and_commits_zero_based() {
        let mut form = AccountForm::edit(
            &account(3, "Brokerage", Kind::Cash, Group::Savings),
            InterestPolicy::Manual,
            2,
            3,
            None,
        );
        assert_eq!(form.display(AccountField::Order).plain_text(), "3 of 3");
        assert_eq!(form.commit().unwrap().position, 2);

        form.next_choice_on(AccountField::Order);
        assert_eq!(form.display(AccountField::Order).plain_text(), "1 of 3");
        assert_eq!(form.commit().unwrap().position, 0);

        form.choice(Step::PREVIOUS);
        // Focus is still Name, so the arrow must not have moved the order.
        assert_eq!(form.display(AccountField::Order).plain_text(), "1 of 3");
    }

    /// `—` first, then one entry per name: the choice the owner takes back
    /// has to be reachable, and it is where every account starts.
    #[test]
    fn the_color_selector_offers_no_color_and_then_each_one() {
        let choices = color_choices();
        assert_eq!(choices.len(), AccountColor::ALL.len() + 1);
        assert_eq!(choices[0], None);
        for color in AccountColor::ALL {
            assert!(choices.contains(&Some(color)), "{color:?} is not offered");
        }
    }

    /// `—` is still in the cycle, one step back from where an unset account
    /// opens: the derivation has to be recoverable for an owner who picked a
    /// color and wants it undone.
    #[test]
    fn the_color_selector_cycles_round_to_no_color() {
        let mut form = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        let opening = form.commit().unwrap().color;
        assert_eq!(opening, Some(AccountColor::derived(AccountId(1))));

        // One choice per name plus `—`, so a full cycle returns to the start
        // and passes through `—` on the way.
        let mut seen = vec![opening];
        for _ in 0..AccountColor::ALL.len() {
            form.next_choice_on(AccountField::Color);
            seen.push(form.commit().unwrap().color);
        }
        assert!(seen.contains(&None), "`—` is unreachable: {seen:?}");
        form.next_choice_on(AccountField::Color);
        assert_eq!(
            form.commit().unwrap().color,
            opening,
            "a full cycle must come back to where it started"
        );
    }

    /// Pressing Enter straight away must not repaint an account the owner
    /// only opened the form to rename.
    #[test]
    fn the_form_opens_on_the_color_the_account_already_holds() {
        let mut account = account(2, "Rainy Day", Kind::Cash, Group::Savings);
        account.color = Some(AccountColor::Violet);
        let form = AccountForm::edit(&account, InterestPolicy::Manual, 1, 3, None);
        assert_eq!(form.display(AccountField::Color).plain_text(), "Violet");
        assert_eq!(form.commit().unwrap().color, Some(AccountColor::Violet));
    }

    #[test]
    fn the_interest_selector_round_trips_every_policy() {
        let mut form = AccountForm::edit(
            &account(2, "Rainy Day", Kind::Cash, Group::Savings),
            InterestPolicy::ProRata,
            1,
            3,
            None,
        );
        assert_eq!(form.commit().unwrap().policy, InterestPolicy::ProRata);
        for _ in 0..InterestPolicy::ALL.len() {
            form.next_choice_on(AccountField::Interest);
        }
        assert_eq!(
            form.commit().unwrap().policy,
            InterestPolicy::ProRata,
            "a full cycle must come back to where it started"
        );
    }

    /// An account with no name would draw as a blank row on four screens.
    #[test]
    fn an_empty_name_is_refused() {
        let mut form = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        for _ in 0.."Everyday".len() {
            form.edit(backspace_key());
        }
        assert!(form.commit().is_err());
    }

    #[test]
    fn typing_reaches_the_name_and_no_selector() {
        let mut form = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        form.edit(char_key('!'));
        assert_eq!(form.commit().unwrap().name, "Everyday!");

        form.focus = AccountField::Band;
        form.edit(char_key('X'));
        assert_eq!(form.commit().unwrap().name, "Everyday!");
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut accounts = screen();
        assert_eq!(accounts.selected().unwrap().account.id(), AccountId(1));
        accounts.select_previous();
        assert_eq!(accounts.selected().unwrap().account.id(), AccountId(1));

        accounts.select_last();
        assert_eq!(accounts.selected().unwrap().account.id(), AccountId(5));
        accounts.select_next();
        assert_eq!(accounts.selected().unwrap().account.id(), AccountId(5));
    }

    #[test]
    fn an_empty_table_has_nothing_selected() {
        let mut accounts = Accounts::new();
        accounts.set_rows(Vec::new());
        assert!(accounts.selected().is_none());
    }

    /// An empty screen is what a database before its first import looks
    /// like, and there are two ways out of it now: the import that names
    /// every account the workbook carries, and `a` for one it does not.
    /// Naming only the import would send an owner to a workbook they may not
    /// have.
    #[test]
    fn an_empty_screen_names_both_ways_out_of_it() {
        let mut accounts = Accounts::new();
        accounts.set_rows(Vec::new());
        let drawn = drawn(&accounts).join("\n");
        assert!(drawn.contains("mm import"), "{drawn}");
        assert!(drawn.contains("press a"), "{drawn}");
    }

    fn drawn(accounts: &Accounts) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 10)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), accounts);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..10)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// Every column's widest plausible content, whole. A truncated cell here
    /// would report an account as being in a band it is not in.
    #[test]
    fn every_column_fits_the_minimum_width() {
        let mut accounts = screen();
        let mut rows = accounts.rows().to_vec();
        rows[1].policy = InterestPolicy::ProRata;
        rows[1].block = Some(SavingsBlock::Goals);
        rows[2].block = Some(SavingsBlock::Buckets);
        accounts.set_rows(rows);
        let table = drawn(&accounts).join("\n");

        for expected in [
            "Code",
            "Account",
            "Kind",
            "Band",
            "Interest",
            "Savings",
            "CHK",
            "Everyday Card",
            "Rainy Day",
            "Checking",
            "Credit",
            "Cash",
            "by balance",
            "like last time",
            SavingsBlock::Goals.label(),
            SavingsBlock::Buckets.label(),
        ] {
            assert!(
                table.contains(expected),
                "{expected:?} missing from\n{table}"
            );
        }
    }

    /// The value the `Color` selector shows is drawn in the shade it names,
    /// so the choice is answerable in the modal rather than by saving it and
    /// looking at the table behind. Only the value: the label and the caret
    /// are the form's chrome.
    #[test]
    fn the_color_field_draws_its_value_in_the_color_it_names() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut account = account(2, "Rainy Day", Kind::Cash, Group::Savings);
        account.color = Some(AccountColor::Violet);
        let form = AccountForm::edit(&account, InterestPolicy::Manual, 1, 3, None);

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 16)).unwrap();
        terminal
            .draw(|frame| {
                render_form(frame, &form);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let (y, line) = (0..16u16)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains("Violet"))
            .expect("the Color field was not drawn");

        let expected = crate::tui::style::account_color(account.id, Some(AccountColor::Violet));
        let at = crate::tui::column_of(&line, "Violet");
        assert_eq!(buffer[(at, y)].fg, expected, "{line:?}");

        // The label beside it is chrome and stays plain.
        let label = crate::tui::column_of(&line, "Color");
        assert_eq!(
            buffer[(label, y)].fg,
            crate::tui::style::Color::Reset,
            "{line:?}"
        );
    }

    /// An account nobody has picked for opens on the color it is *already
    /// being drawn in*, named. Opening on `—` put the selector somewhere the
    /// row behind the modal contradicted, and made the first `→` a jump to
    /// the head of the palette rather than a step off the current color.
    #[test]
    fn an_unset_account_opens_on_the_color_it_is_already_drawn_in() {
        let form = AccountForm::edit(
            &account(2, "Rainy Day", Kind::Cash, Group::Savings),
            InterestPolicy::Manual,
            1,
            3,
            None,
        );
        let derived = AccountColor::derived(AccountId(2));
        assert_eq!(
            form.display(AccountField::Color).plain_text(),
            derived.label()
        );
        assert_eq!(form.color_choice(), Some(derived));
        // And it is the same shade the row was drawn in before the form
        // opened -- the choice names what was there, it does not change it.
        assert_eq!(
            crate::tui::style::account_color(AccountId(2), form.color_choice()),
            crate::tui::style::account_color(AccountId(2), None),
        );
    }

    /// `—` still means the derivation, and still draws as it, for the owner
    /// who cycles back to it.
    #[test]
    fn the_no_color_choice_draws_in_the_shade_the_id_derives() {
        let mut form = AccountForm::edit(
            &account(2, "Rainy Day", Kind::Cash, Group::Savings),
            InterestPolicy::Manual,
            1,
            3,
            None,
        );
        walk_until!(
            form.color_choice().is_none(),
            form.next_choice_on(AccountField::Color)
        );
        assert_eq!(form.display(AccountField::Color).plain_text(), "—");
        assert_eq!(
            crate::tui::style::account_color(AccountId(2), form.color_choice()),
            crate::tui::style::account_color(AccountId(2), None),
        );
    }

    /// A card has no interest posting to divide, so the cell says so rather
    /// than naming a policy nothing reads.
    #[test]
    fn a_credit_row_shows_no_interest_policy() {
        let rows = drawn(&screen());
        let card = rows
            .iter()
            .find(|l| l.contains("Card One"))
            .expect("no Card One row");
        assert!(card.contains('—'), "{card:?}");
        assert!(!card.contains("like last time"), "{card:?}");
    }

    /// `a` asks the two things `e` refuses to touch -- the code and the kind,
    /// which are what `account::by_code` matches the next import against --
    /// and the one thing every account needs to draw as a row. The other five
    /// are `e`'s, because they are what the account is *placed* by rather
    /// than what it is.
    #[test]
    fn an_add_form_asks_for_a_code_a_kind_and_a_name_and_nothing_else() {
        let form = AccountForm::add();
        assert_eq!(
            form.fields(),
            vec![AccountField::Code, AccountField::Kind, AccountField::Name]
        );
    }

    /// The kind decides an *edit* form's fields -- credit has no band to
    /// cycle and holds no goals. It decides nothing here: an account is
    /// placed after it exists, so there is no band, policy or block on this
    /// form for a kind to add or take away.
    #[test]
    fn the_kind_selector_does_not_grow_an_add_forms_field_list() {
        let mut form = AccountForm::add();
        let cash = form.fields();
        form.next_choice_on(AccountField::Kind);
        assert_eq!(form.display(AccountField::Kind).plain_text(), "Credit");
        assert_eq!(form.fields(), cash);
    }

    /// Both kinds, in the order the selector cycles them, and a full cycle
    /// comes back to where it started.
    #[test]
    fn the_kind_selector_round_trips_every_kind() {
        let mut form = AccountForm::add();
        assert_eq!(form.display(AccountField::Kind).plain_text(), "Cash");
        for _ in 0..Kind::ALL.len() {
            form.next_choice_on(AccountField::Kind);
        }
        assert_eq!(
            form.display(AccountField::Kind).plain_text(),
            "Cash",
            "a full cycle must come back to where it started"
        );
    }

    /// The code is what the next import matches this row against, and `e`
    /// cannot fix one: a blank is refused here or nowhere.
    #[test]
    fn an_add_form_refuses_an_empty_code() {
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Name, "Rainy Day");
        let err = form.commit_new().unwrap_err();
        assert!(err.to_string().contains("code"), "{err}");
    }

    /// An account with no name would draw as a blank row on four screens.
    #[test]
    fn an_add_form_refuses_an_empty_name() {
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Code, "SAV");
        let err = form.commit_new().unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    /// A code the `Code` column cannot hold is refused rather than stored and
    /// drawn cut: the column is the only place a code is ever shown, so a
    /// truncated one leaves the owner unable to read what the next import
    /// matches the row against.
    #[test]
    fn an_add_form_refuses_a_code_wider_than_its_column() {
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Code, "ABCDEFGHI");
        type_into(&mut form, AccountField::Name, "Rainy Day");
        let err = form.commit_new().unwrap_err();
        assert!(err.to_string().contains("code"), "{err}");

        // One character narrower is the widest the column can draw whole.
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Code, "ABCDEFGH");
        type_into(&mut form, AccountField::Name, "Rainy Day");
        assert_eq!(form.commit_new().unwrap().code, "ABCDEFGH");
    }

    /// Both text fields are trimmed, the way every other form's are: a code
    /// with a stray space is a code no import will ever match.
    #[test]
    fn an_add_form_commits_a_trimmed_code_name_and_kind() {
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Code, " CC1 ");
        type_into(&mut form, AccountField::Name, " Card One ");
        form.next_choice_on(AccountField::Kind);
        let new = form.commit_new().unwrap();
        assert_eq!(new.code, "CC1");
        assert_eq!(new.name, "Card One");
        assert_eq!(new.kind, Kind::Credit);
    }

    /// An account code is one of the four categories `CLAUDE.md` bans from a
    /// tracked file outright, and the Accounts screen's own table already
    /// masks it -- the add form's own `Code` field is the same text, typed
    /// rather than read off a row, and a demo must hide it just the same.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_typed_account_code() {
        crate::demo::install_with_salt(7);
        let mut form = AccountForm::add();
        type_into(&mut form, AccountField::Code, "CHK");
        let drawn = form.display(AccountField::Code).plain_text();
        assert_ne!(drawn, "CHK");
        assert_eq!(drawn, crate::demo::text("CHK"));
        // The buffer is untouched: Enter still writes the real code.
        type_into(&mut form, AccountField::Name, "Everyday");
        assert_eq!(form.commit_new().unwrap().code, "CHK");
    }

    /// Focus a field and type into it, the way the modal's key handler does.
    fn type_into(form: &mut AccountForm, field: AccountField, text: &str) {
        form.focus = field;
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }
}

#[cfg(test)]
mod savings_block_tests {
    use super::*;

    fn account(id: i64, name: &str, kind: Kind, group: Group) -> Account {
        Account {
            id: AccountId(id),
            code: "CHK".into(),
            name: name.into(),
            kind,
            sort: 0,
            group,
            color: None,
        }
    }

    /// One selector per account, and it holds one value -- so an account
    /// claiming both blocks is unrepresentable rather than validated away.
    #[test]
    fn the_savings_selector_offers_no_block_and_then_each_one() {
        let choices = savings_choices();
        assert_eq!(choices.len(), SavingsBlock::ALL.len() + 1);
        assert_eq!(choices[0], None);
        for block in SavingsBlock::ALL {
            assert!(choices.contains(&Some(block)), "{block:?} is not offered");
        }
    }

    #[test]
    fn the_savings_selector_cycles_back_to_no_block() {
        let mut form = AccountForm::edit(
            &account(1, "Everyday", Kind::Cash, Group::Checking),
            InterestPolicy::Manual,
            0,
            3,
            None,
        );
        assert_eq!(form.display(AccountField::Savings).plain_text(), "—");
        assert_eq!(form.commit().unwrap().block, None);

        form.next_choice_on(AccountField::Savings);
        assert_eq!(form.commit().unwrap().block, Some(SavingsBlock::Goals));

        for _ in 0..SavingsBlock::ALL.len() {
            form.next_choice_on(AccountField::Savings);
        }
        assert_eq!(
            form.commit().unwrap().block,
            None,
            "a full cycle must come back to holding neither block"
        );
    }

    /// A card holds no goals, so it can be neither container.
    #[test]
    fn a_card_is_never_offered_a_savings_block() {
        let card = AccountForm::edit(
            &account(4, "Card One", Kind::Credit, Group::Credit),
            InterestPolicy::Manual,
            0,
            2,
            None,
        );
        assert!(!card.fields().contains(&AccountField::Savings));
    }

    /// The form opens on what the account already holds, so pressing Enter
    /// straight away cannot silently move a block off it.
    #[test]
    fn the_form_opens_on_the_block_the_account_already_holds() {
        let form = AccountForm::edit(
            &account(2, "Rainy Day", Kind::Cash, Group::Savings),
            InterestPolicy::Manual,
            1,
            3,
            Some(SavingsBlock::Buckets),
        );
        assert_eq!(
            form.display(AccountField::Savings).plain_text(),
            SavingsBlock::Buckets.label()
        );
        assert_eq!(form.commit().unwrap().block, Some(SavingsBlock::Buckets));
    }
}
