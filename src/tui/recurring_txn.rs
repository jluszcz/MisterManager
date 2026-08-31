//! The Recurring Transactions screen: one list of the rows whose amount and
//! date are known in advance, and the form that edits them.
//!
//! View state only -- no ratatui above the render functions at the bottom, and
//! no `Db` on the type. `App` runs the queries and hands the results in.

use super::Label;
use super::cursor::{Cursor, Viewport, impl_scroll};
use super::form::{
    Caret, DateField, Field, Focused, FormFields, Step, next_in, parse_amount, step_index,
};
use crate::db::RecurringTxnId;
use crate::db::account::Account;
use crate::db::recurring_txn::{Cadence, NewRecurringTxn, RecurringTxn};
use crate::db::txn::Suggestion;
use crate::money::Cents;
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use std::collections::HashMap;

/// What the Account column is headed, and what [`RecurringTxns::acct_width`]
/// measures against -- one string, so the width cannot be computed for a
/// header the table does not draw.
const ACCT_HEADER: &str = "Acct";

/// One recurring transaction as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub recurring_txn_id: RecurringTxnId,
    pub description: String,
    /// The account this row belongs to, as the Account column shows it: its
    /// code, since the other columns pin the row down already -- or its name
    /// and kind, where the code alone names two of them. This is the one
    /// screen whose list mixes both kinds, so it is the one screen where
    /// `UNIQUE (code, kind)` leaves a code naming more than one row.
    pub account: super::Account,
    pub cents: Cents,
    pub cadence: Cadence,
    pub anchor_date: NaiveDate,
    /// The furthest-dated ledger row this recurring transaction owns, and
    /// `None` until the first `g` gives it one. The screen shows how far the
    /// rows actually reach rather than the horizon they are allowed to,
    /// because that is the number `x` moves.
    pub last_owned: Option<NaiveDate>,
    pub is_paycheck: bool,
    /// How many `txn` rows this recurring transaction owns, at any date.
    /// Adoption is invisible otherwise.
    pub owned: i64,
}

pub struct RecurringTxns {
    accounts: Vec<Account>,
    rows: Vec<Row>,
    cursor: Cursor,
}

impl RecurringTxns {
    pub fn new(accounts: Vec<Account>) -> RecurringTxns {
        RecurringTxns {
            accounts,
            rows: Vec::new(),
            cursor: Cursor::new(),
        }
    }

    pub fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
    }

    pub fn set_recurring_txns(
        &mut self,
        txns: Vec<RecurringTxn>,
        owned: HashMap<RecurringTxnId, i64>,
        last_owned: HashMap<RecurringTxnId, NaiveDate>,
    ) {
        self.rows = txns
            .into_iter()
            .map(|txn| Row {
                account: super::Account::coded(&self.accounts, txn.account_id),
                owned: owned.get(&txn.id).copied().unwrap_or(0),
                last_owned: last_owned.get(&txn.id).copied(),
                recurring_txn_id: txn.id,
                description: txn.description,
                cents: txn.cents,
                cadence: txn.cadence,
                anchor_date: txn.anchor_date,
                is_paycheck: txn.is_paycheck,
            })
            .collect();
        self.cursor.clamp(self.rows.len());
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.cursor.index())
    }

    pub fn title(&self) -> String {
        format!("Recurring Transactions · {}", self.rows.len())
    }

    /// How wide the Account column has to be: its header, or the widest label
    /// drawn under it.
    ///
    /// Derived rather than fixed, because a code is not the only thing that
    /// column holds. A row whose code names two accounts falls back to
    /// `Name — Kind`, and a width chosen for a code would truncate exactly
    /// the label that exists to say more than one.
    pub fn acct_width(&self) -> u16 {
        let width = |text: &str| text.chars().count();
        self.rows
            .iter()
            .map(|r| r.account.render_with(|text, _| width(text)))
            .chain(std::iter::once(width(ACCT_HEADER)))
            .max()
            .unwrap_or_else(|| width(ACCT_HEADER)) as u16
    }
}

impl_scroll!(RecurringTxns, rows);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecurringTxnField {
    Description,
    Amount,
    Account,
    Cadence,
    Anchor,
    Horizon,
}

impl RecurringTxnField {
    pub const ORDER: [RecurringTxnField; 6] = [
        RecurringTxnField::Description,
        RecurringTxnField::Amount,
        RecurringTxnField::Account,
        RecurringTxnField::Cadence,
        RecurringTxnField::Anchor,
        RecurringTxnField::Horizon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RecurringTxnField::Description => "Description",
            RecurringTxnField::Amount => "Amount",
            RecurringTxnField::Account => "Account",
            RecurringTxnField::Cadence => "Cadence",
            RecurringTxnField::Anchor => "Anchor",
            RecurringTxnField::Horizon => "Horizon",
        }
    }
}

/// Adding or editing one recurring transaction. Backs `a` and `e`.
///
/// The account and the cadence are selectors rather than text fields, so
/// neither an account that does not exist nor a cadence the schema's `CHECK`
/// would refuse is representable.
///
/// `is_paycheck` is not a field: at most one recurring transaction carries it,
/// and `P` is where it moves.
#[derive(Debug)]
pub struct RecurringTxnForm {
    pub editing: Option<RecurringTxnId>,
    pub focus: RecurringTxnField,
    description: Field,
    amount: Field,
    anchor: DateField,
    horizon: DateField,
    accounts: Vec<Account>,
    account: usize,
    account_touched: bool,
    cadence: usize,
}

impl RecurringTxnForm {
    pub fn add(accounts: Vec<Account>, today: NaiveDate) -> Result<RecurringTxnForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no account to write a recurring transaction against"
        );
        Ok(RecurringTxnForm {
            editing: None,
            focus: RecurringTxnField::Description,
            description: Field::default(),
            amount: Field::default(),
            anchor: DateField::today(today),
            horizon: DateField::blank(today),
            accounts,
            account: 0,
            account_touched: false,
            cadence: 0,
        })
    }

    pub fn edit(
        accounts: Vec<Account>,
        today: NaiveDate,
        txn: &RecurringTxn,
    ) -> Result<RecurringTxnForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no account to write a recurring transaction against"
        );
        let account = accounts
            .iter()
            .position(|a| a.id == txn.account_id)
            .unwrap_or(0);
        Ok(RecurringTxnForm {
            editing: Some(txn.id),
            focus: RecurringTxnField::Description,
            description: Field::given(txn.description.clone()),
            amount: Field::given(txn.cents.to_string()),
            anchor: DateField::given(today, Some(txn.anchor_date)),
            horizon: DateField::given(today, txn.horizon),
            accounts,
            account,
            // The recurring transaction's own account, which the user can see
            // and did not ask to change: a suggestion leaves it alone, as
            // `Field::given` does for the text fields beside it.
            account_touched: true,
            cadence: Cadence::ALL
                .iter()
                .position(|c| *c == txn.cadence)
                .unwrap_or(0),
        })
    }

    pub fn description(&self) -> &str {
        self.description.value()
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => {
                "Edit recurring transaction — Tab field · ←/→ selector · Enter save · Esc cancel"
            }
            None => {
                "Add recurring transaction — Tab field · ←/→ selector · Enter save · Esc cancel"
            }
        }
    }

    pub fn display(&self, field: RecurringTxnField) -> Label {
        match field {
            RecurringTxnField::Description => {
                Label::from(crate::demo::text(self.description.value()).into_owned())
            }
            RecurringTxnField::Amount => Label::from(crate::demo::typed(self.amount.value())),
            RecurringTxnField::Anchor => {
                Label::from(self.anchor.display(self.focus == RecurringTxnField::Anchor))
            }
            RecurringTxnField::Horizon => Label::from(
                self.horizon
                    .display(self.focus == RecurringTxnField::Horizon),
            ),
            RecurringTxnField::Account => match self.accounts.get(self.account) {
                Some(a) => Label::default().account(super::Account::labelled(&self.accounts, a.id)),
                None => Label::default(),
            },
            RecurringTxnField::Cadence => Label::from(Cadence::ALL[self.cadence].as_str()),
        }
    }

    pub fn commit(&self) -> Result<NewRecurringTxn> {
        let account = self
            .accounts
            .get(self.account)
            .map(|a| a.id)
            .ok_or_else(|| anyhow::anyhow!("no account is selected"))?;
        let description = self.description.value().trim().to_string();
        ensure!(!description.is_empty(), "description must not be empty");
        Ok(NewRecurringTxn {
            description,
            // Signed exactly as the ledger is, and `parse_amount` refuses an
            // empty field: a zero-amount recurring transaction writes a row
            // that changes nothing, every fortnight, forever.
            cents: parse_amount(self.amount.value())?,
            account_id: account,
            cadence: Cadence::ALL[self.cadence],
            anchor_date: self.anchor.parse()?,
            // An empty horizon is a recurring transaction that does not end --
            // the paycheck.
            horizon: self.horizon.parse_opt()?,
        })
    }
}

impl FormFields for RecurringTxnForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&RecurringTxnField::ORDER, self.focus, step);
    }

    fn cycle(&mut self, step: Step) {
        match self.focus {
            RecurringTxnField::Account => {
                self.account = step_index(self.account, self.accounts.len(), step.direction());
                self.account_touched = true;
            }
            RecurringTxnField::Cadence => {
                self.cadence = step_index(self.cadence, Cadence::ALL.len(), step.direction())
            }
            RecurringTxnField::Description
            | RecurringTxnField::Amount
            | RecurringTxnField::Anchor
            | RecurringTxnField::Horizon => {}
        }
    }

    /// Both dates step on `←`/`→`; an empty horizon has none, and stepping
    /// one out of nothing would give a rule that does not end an end date
    /// nobody typed.
    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            RecurringTxnField::Description => Focused::Text(&mut self.description),
            RecurringTxnField::Amount => Focused::Text(&mut self.amount),
            RecurringTxnField::Anchor => Focused::Date(&mut self.anchor),
            RecurringTxnField::Horizon => Focused::Date(&mut self.horizon),
            RecurringTxnField::Account | RecurringTxnField::Cadence => Focused::Selector,
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            RecurringTxnField::Description => Caret::in_field(&self.description),
            RecurringTxnField::Amount => Caret::in_field(&self.amount),
            RecurringTxnField::Anchor => Caret::in_field(self.anchor.text()),
            RecurringTxnField::Horizon => Caret::in_field(self.horizon.text()),
            RecurringTxnField::Account | RecurringTxnField::Cadence => Caret::End,
        }
    }

    fn suggestion_prefix(&self) -> Option<&str> {
        (self.focus == RecurringTxnField::Description).then(|| self.description.value())
    }

    /// The description always, the amount and the account only if the user
    /// has not touched them — `TxnForm`'s rule, for the same reason. A
    /// recurring transaction's whole point is that it repeats what the ledger
    /// already holds, so the suggestion is usually exactly right; overwriting
    /// an amount the user typed and then cleared is not.
    fn apply_suggestion(&mut self, hit: &Suggestion) {
        self.description.fill(hit.description.clone());
        if !self.amount.is_touched() {
            self.amount.fill(hit.cents.to_string());
        }
        if !self.account_touched
            && let Some(i) = self.accounts.iter().position(|a| a.id == hit.account_id)
        {
            self.account = i;
        }
    }
}

use super::autocomplete::Autocomplete;
use super::form::{field_line, render_fields, render_popup};
use super::{Chrome, account_cell, amount, render_table, right_header};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Cell, Row as TableRow};

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_form(frame: &mut Frame, form: &RecurringTxnForm, popup: &Autocomplete) -> usize {
    let lines: Vec<TextLine> = RecurringTxnField::ORDER
        .iter()
        .map(|f| {
            field_line(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
            )
        })
        .collect();
    let area = render_fields(frame, form.title(), lines);
    render_popup(frame, area, popup)
}

/// One row per recurring transaction. Returns the [`Viewport`] it drew: the
/// height `PageUp`/`PageDown` move by, and the row the next draw starts from.
pub(super) fn render(frame: &mut Frame, area: Rect, list: &RecurringTxns) -> Viewport {
    let optional = |value: Option<String>| {
        Cell::from(TextLine::from(value.unwrap_or_else(|| "—".to_string())))
    };
    let rows: Vec<TableRow> = list
        .rows()
        .iter()
        .map(|r| {
            TableRow::new(vec![
                Cell::from(if r.is_paycheck { "$" } else { " " }),
                Cell::from(crate::demo::text(&r.description).into_owned()),
                account_cell(&r.account),
                amount(r.cents),
                Cell::from(r.cadence.as_str()),
                Cell::from(r.anchor_date.to_string()),
                optional(r.last_owned.map(|d| d.to_string())),
                Cell::from(TextLine::from(r.owned.to_string()).right_aligned()),
            ])
        })
        .collect();

    // `Amount` and `Rows` are the right-aligned columns. `Last` is not: its
    // cells go through the local `optional` above, which does not right-align.
    let header = TableRow::new(vec![
        Cell::from(" "),
        Cell::from("Description"),
        Cell::from(ACCT_HEADER),
        right_header("Amount"),
        Cell::from("Cadence"),
        Cell::from("Anchor"),
        Cell::from("Last"),
        right_header("Rows"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Length(1),
        Constraint::Min(16),
        Constraint::Length(list.acct_width()),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(5),
    ];

    render_table(
        frame,
        area,
        list,
        Chrome::titled(list.title()).header(header),
        &widths,
        rows,
        list.rows().len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::test_support::{cash, credit, day, walk_until};
    use crate::tui::MIN_WIDTH;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::{backspace_key, char_key};

    fn today() -> NaiveDate {
        day(2026, 8, 16)
    }

    fn accounts() -> Vec<Account> {
        vec![cash(1, "CHK"), cash(2, "SAV")]
    }

    fn recurring_txn(
        id: i64,
        description: &str,
        cents: i64,
        cadence: Cadence,
        paycheck: bool,
    ) -> RecurringTxn {
        RecurringTxn {
            id: RecurringTxnId(id),
            description: description.to_string(),
            cents: Cents(cents),
            account_id: AccountId(1),
            cadence,
            anchor_date: day(2026, 8, 28),
            horizon: Some(day(2026, 12, 18)),
            generate_through: None,
            is_paycheck: paycheck,
        }
    }

    /// The workbook's recurring transactions.
    fn screen() -> RecurringTxns {
        let mut list = RecurringTxns::new(accounts());
        list.set_recurring_txns(
            vec![
                recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true),
                recurring_txn(2, "Mortgage", -120_000, Cadence::Monthly, false),
                recurring_txn(3, "HOA", -30_000, Cadence::Monthly, false),
                recurring_txn(4, "Phone", -4_500, Cadence::Monthly, false),
            ],
            HashMap::from([(RecurringTxnId(1), 9), (RecurringTxnId(2), 4)]),
            HashMap::from([
                (RecurringTxnId(1), day(2026, 12, 18)),
                (RecurringTxnId(2), day(2026, 11, 1)),
            ]),
        );
        list
    }

    /// "adopted 4" is how the owner sees that the first regenerate claimed the
    /// imported rows instead of duplicating them, and this column is where it
    /// stays visible afterwards.
    #[test]
    fn each_row_carries_how_many_ledger_rows_its_rule_owns() {
        let list = screen();
        let owned: Vec<i64> = list.rows().iter().map(|r| r.owned).collect();
        assert_eq!(owned, vec![9, 4, 0, 0]);
    }

    /// The column says how far the ledger actually carries each recurring
    /// transaction, which is the number `x` moves. A recurring transaction
    /// that has never been regenerated owns no rows and so has no last date.
    #[test]
    fn each_row_carries_the_last_date_its_rule_reaches() {
        let list = screen();
        let last: Vec<Option<NaiveDate>> = list.rows().iter().map(|r| r.last_owned).collect();
        assert_eq!(
            last,
            vec![Some(day(2026, 12, 18)), Some(day(2026, 11, 1)), None, None]
        );
    }

    #[test]
    fn each_row_names_its_rules_account_by_code() {
        let list = screen();
        assert!(list.rows().iter().all(|r| r.account.text() == "CHK"));
    }

    /// This is the one list that mixes both kinds, so it is the one screen
    /// where a code can name two rows -- and neither row may be drawn by the
    /// code they share.
    #[test]
    fn a_rule_on_a_code_both_kinds_hold_is_named_by_its_kind_instead() {
        let mut list = RecurringTxns::new(vec![cash(1, "CHK"), credit(2, "CHK")]);
        list.set_recurring_txns(
            vec![recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true)],
            HashMap::new(),
            HashMap::new(),
        );
        assert_eq!(list.rows()[0].account.text(), "Everyday — Cash");
    }

    /// The widened label says less than the code did if the column truncates
    /// it, so the column is as wide as the widest thing drawn in it.
    #[test]
    fn the_account_column_widens_to_fit_a_label_a_code_could_not_carry() {
        let mut list = RecurringTxns::new(vec![cash(1, "CHK"), credit(2, "CHK")]);
        list.set_recurring_txns(
            vec![recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true)],
            HashMap::new(),
            HashMap::new(),
        );
        assert_eq!(list.acct_width(), "Everyday — Cash".chars().count() as u16);
    }

    /// No code names two accounts here, so every row is drawn by its code and
    /// the header is the widest thing in the column.
    #[test]
    fn the_account_column_is_no_wider_than_its_header_when_no_code_is_shared() {
        assert_eq!(screen().acct_width(), "Acct".chars().count() as u16);
    }

    /// An account id with no account is a corrupt row, not a reason to stop
    /// drawing the screen.
    #[test]
    fn a_rule_on_an_account_that_is_gone_still_renders() {
        let mut list = RecurringTxns::new(Vec::new());
        list.set_recurring_txns(
            vec![recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true)],
            HashMap::new(),
            HashMap::new(),
        );
        assert_eq!(list.rows()[0].account.text(), "?");
    }

    #[test]
    fn exactly_one_row_is_marked_as_the_paycheck_rule() {
        let list = screen();
        let flagged: Vec<&str> = list
            .rows()
            .iter()
            .filter(|r| r.is_paycheck)
            .map(|r| r.description.as_str())
            .collect();
        assert_eq!(flagged, ["Salary"]);
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut list = screen();
        assert_eq!(list.selected().unwrap().recurring_txn_id, RecurringTxnId(1));
        list.select_previous();
        assert_eq!(list.selected().unwrap().recurring_txn_id, RecurringTxnId(1));

        list.select_last();
        assert_eq!(list.selected().unwrap().recurring_txn_id, RecurringTxnId(4));
        list.select_next();
        assert_eq!(list.selected().unwrap().recurring_txn_id, RecurringTxnId(4));

        list.select_first();
        assert_eq!(list.selected_index(), 0);
    }

    /// `d` releases rows rather than deleting them, so a cursor left past the
    /// end would make `g`, `P` and `d` act on nothing -- or on whatever later
    /// lands at that index.
    #[test]
    fn a_shrinking_list_moves_the_selection_into_bounds() {
        let mut list = screen();
        list.select_last();

        list.set_recurring_txns(
            vec![recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true)],
            HashMap::new(),
            HashMap::new(),
        );

        assert_eq!(list.selected_index(), 0);
        assert_eq!(list.selected().unwrap().recurring_txn_id, RecurringTxnId(1));
    }

    #[test]
    fn an_empty_list_has_nothing_selected() {
        let mut list = RecurringTxns::new(accounts());
        list.set_recurring_txns(Vec::new(), HashMap::new(), HashMap::new());
        assert!(list.rows().is_empty());
        assert!(list.selected().is_none());
    }

    /// The form's selector names the same account its Account column does, in
    /// the same color -- the column shows a code and the form shows both
    /// halves, but they are one account.
    #[test]
    fn the_recurring_transaction_selector_shows_one_colored_account() {
        let form = RecurringTxnForm::add(accounts(), today()).unwrap();
        let value = form.display(RecurringTxnField::Account);
        assert_eq!(value.plain_text(), "CHK — Everyday");
        assert_eq!(value.accounts().len(), 1);
    }

    #[test]
    fn a_rule_form_commits_what_was_typed() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        assert_eq!(form.editing, None);
        assert_eq!(
            form.display(RecurringTxnField::Anchor).plain_text(),
            "2026-08-16"
        );

        for c in "Mortgage".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "-1,200.00".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        form.choice(Step::NEXT);
        form.next_field();
        form.choice(Step::NEXT);
        form.next_field();
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        for c in "2026-09-01".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "2026-12-01".chars() {
            form.edit(char_key(c));
        }

        let new = form.commit().unwrap();
        assert_eq!(new.description, "Mortgage");
        assert_eq!(new.cents, Cents(-120_000));
        assert_eq!(new.account_id, AccountId(2));
        assert_eq!(new.cadence, Cadence::Monthly);
        assert_eq!(new.anchor_date, day(2026, 9, 1));
        assert_eq!(new.horizon, Some(day(2026, 12, 1)));
    }

    /// A recurring transaction that does not end leaves the field blank -- the
    /// monthlies stop at year end, the paycheck does not.
    #[test]
    fn an_empty_horizon_is_a_rule_that_does_not_end() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        for c in "Salary".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "5000.00".chars() {
            form.edit(char_key(c));
        }
        assert_eq!(form.commit().unwrap().horizon, None);
    }

    /// The amount is money and the description is the owner's own word for
    /// the rule, so a demo hides both; the account, cadence and anchor date
    /// are the rule itself.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amount_and_the_description_and_keeps_the_rule() {
        crate::demo::install_with_salt(7);
        let form = RecurringTxnForm::edit(
            accounts(),
            today(),
            &recurring_txn(2, "Mortgage", -120_000, Cadence::Monthly, false),
        )
        .unwrap();
        let drawn = form.display(RecurringTxnField::Amount).plain_text();
        assert_ne!(drawn, "-1,200.00");
        assert_eq!(drawn.len(), "-1,200.00".len());
        let drawn_description = form.display(RecurringTxnField::Description).plain_text();
        assert_ne!(drawn_description, "Mortgage");
        assert_eq!(drawn_description, crate::demo::text("Mortgage"));
        assert_eq!(
            form.display(RecurringTxnField::Anchor).plain_text(),
            "2026-08-28"
        );
    }

    #[test]
    fn a_rule_form_opened_on_a_rule_prefills_every_field() {
        let form = RecurringTxnForm::edit(
            accounts(),
            today(),
            &recurring_txn(2, "Mortgage", -120_000, Cadence::Monthly, false),
        )
        .unwrap();
        assert_eq!(form.editing, Some(RecurringTxnId(2)));
        assert_eq!(
            form.display(RecurringTxnField::Description).plain_text(),
            "Mortgage"
        );
        assert_eq!(
            form.display(RecurringTxnField::Amount).plain_text(),
            "-1,200.00"
        );
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "monthly"
        );
        assert_eq!(
            form.display(RecurringTxnField::Anchor).plain_text(),
            "2026-08-28"
        );
        assert_eq!(
            form.display(RecurringTxnField::Horizon).plain_text(),
            "2026-12-18"
        );
    }

    #[test]
    fn an_empty_description_is_refused() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        form.next_field();
        for c in "100".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("description"), "{err}");
    }

    #[test]
    fn a_horizon_that_is_not_a_date_is_refused_with_the_text_that_failed() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        for c in "Mortgage".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "100".chars() {
            form.edit(char_key(c));
        }
        walk_until!(form.focus == RecurringTxnField::Horizon, form.next_field());
        for c in "12/01/2026".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("12/01/2026"), "{err}");
    }

    /// A recurring transaction's amount is signed exactly as the ledger is, so
    /// `0` would write a row that changes nothing every fortnight forever.
    #[test]
    fn a_rule_with_no_amount_is_refused() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        for c in "Mortgage".chars() {
            form.edit(char_key(c));
        }
        assert!(form.commit().is_err());
    }

    #[test]
    fn a_form_with_no_account_to_write_to_is_refused() {
        let err = RecurringTxnForm::add(Vec::new(), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    #[test]
    fn opening_a_form_to_edit_with_no_account_to_write_to_is_refused() {
        let err = RecurringTxnForm::edit(
            Vec::new(),
            today(),
            &recurring_txn(2, "Mortgage", -120_000, Cadence::Monthly, false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    fn suggestion(description: &str, account_id: AccountId, cents: i64) -> Suggestion {
        Suggestion {
            description: description.to_string(),
            account_id,
            cents: Cents(cents),
            uses: 4,
        }
    }

    fn typed(form: &mut RecurringTxnForm, field: RecurringTxnField, text: &str) {
        walk_until!(form.focus == field, form.next_field());
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    /// A recurring transaction repeats what the ledger already holds, so the
    /// suggestion is usually exactly right.
    #[test]
    fn accepting_a_suggestion_fills_an_untouched_amount_and_account() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        typed(&mut form, RecurringTxnField::Description, "Mort");

        form.apply_suggestion(&suggestion("Mortgage", AccountId(2), -120_000));

        assert_eq!(
            form.display(RecurringTxnField::Description).plain_text(),
            "Mortgage"
        );
        assert_eq!(
            form.display(RecurringTxnField::Amount).plain_text(),
            "-1,200.00"
        );
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
    }

    /// Touched is not the same as non-empty: an amount typed and then
    /// cleared is still the user's, and a suggestion must leave it alone --
    /// `TxnForm`'s rule, and the reason `Field` carries a flag at all.
    #[test]
    fn accepting_a_suggestion_leaves_an_amount_typed_and_then_cleared_alone() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        typed(&mut form, RecurringTxnField::Amount, "22");
        form.edit(backspace_key());
        form.edit(backspace_key());
        assert_eq!(form.display(RecurringTxnField::Amount).plain_text(), "");
        typed(&mut form, RecurringTxnField::Description, "Mort");

        form.apply_suggestion(&suggestion("Mortgage", AccountId(2), -120_000));

        assert_eq!(
            form.display(RecurringTxnField::Description).plain_text(),
            "Mortgage"
        );
        assert_eq!(form.display(RecurringTxnField::Amount).plain_text(), "");
    }

    /// An account the user chose is not the suggestion's to move.
    #[test]
    fn accepting_a_suggestion_leaves_a_chosen_account_alone() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        walk_until!(form.focus == RecurringTxnField::Account, form.next_field());
        form.choice(Step::NEXT);
        typed(&mut form, RecurringTxnField::Description, "Mort");

        form.apply_suggestion(&suggestion("Mortgage", AccountId(1), -120_000));

        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
    }

    /// An existing recurring transaction's amount and account are real figures
    /// the user can see and did not ask to change.
    #[test]
    fn editing_a_rule_protects_its_amount_and_account_from_suggestions() {
        let mut form = RecurringTxnForm::edit(
            accounts(),
            today(),
            &recurring_txn(2, "Mortgag", -120_000, Cadence::Monthly, false),
        )
        .unwrap();

        form.apply_suggestion(&suggestion("Mortgage", AccountId(2), -300_000));

        assert_eq!(
            form.display(RecurringTxnField::Description).plain_text(),
            "Mortgage"
        );
        assert_eq!(
            form.display(RecurringTxnField::Amount).plain_text(),
            "-1,200.00"
        );
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
    }

    /// Only the description field opens the suggestion popup.
    #[test]
    fn autocomplete_is_offered_on_the_description_and_nowhere_else() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        assert_eq!(form.suggestion_prefix(), Some(""));
        form.next_field();
        assert_eq!(form.suggestion_prefix(), None);
    }

    #[test]
    fn the_account_selector_cycles_both_ways() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        walk_until!(form.focus == RecurringTxnField::Account, form.next_field());
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
    }

    #[test]
    fn the_cadence_selector_cycles_both_ways() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        walk_until!(form.focus == RecurringTxnField::Cadence, form.next_field());
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "biweekly"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "monthly"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "biweekly"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "monthly"
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "biweekly"
        );
    }

    /// Both dates on this form step a day at a time, the same as every other
    /// date field in the app.
    #[test]
    fn the_arrows_step_the_anchor_and_the_horizon_by_a_day() {
        let mut form = RecurringTxnForm::edit(
            accounts(),
            today(),
            &recurring_txn(2, "Mortgage", -120_000, Cadence::Monthly, false),
        )
        .unwrap();

        walk_until!(form.focus == RecurringTxnField::Anchor, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Anchor).plain_text(),
            "2026-08-29"
        );
        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Anchor).plain_text(),
            "2026-08-27"
        );

        walk_until!(form.focus == RecurringTxnField::Horizon, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Horizon).plain_text(),
            "2026-12-19"
        );
    }

    /// An empty horizon is a rule that does not end. Seeding it from an arrow
    /// press would give the paycheck an end date nobody typed.
    #[test]
    fn the_arrows_leave_an_empty_horizon_empty() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        walk_until!(form.focus == RecurringTxnField::Horizon, form.next_field());
        form.choice(Step::NEXT);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(RecurringTxnField::Horizon).plain_text(), "");
    }

    /// `←`/`→` on a text field must not silently change the account or the
    /// cadence. One press each way, never two: every selector here has two
    /// options, so a second press would cycle a broken one straight back to
    /// where it started and the assertion would pass either way.
    #[test]
    fn cycling_does_nothing_unless_a_selector_is_focused() {
        let mut form = RecurringTxnForm::add(accounts(), day(2026, 8, 16)).unwrap();
        assert_eq!(form.focus, RecurringTxnField::Description);

        form.choice(Step::NEXT);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "biweekly"
        );

        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(RecurringTxnField::Account).plain_text(),
            "CHK — Everyday"
        );
        assert_eq!(
            form.display(RecurringTxnField::Cadence).plain_text(),
            "biweekly"
        );
    }

    /// Every column at `MIN_WIDTH`, all read for one test.
    fn drawn(list: &RecurringTxns) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), list);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..8)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// The bug this pins: a `Cell`'s own style covers its whole area, and a
    /// table's `row_highlight_style` is patched over the row *after* the
    /// cells draw -- so `REVERSED` used to turn each tinted cell's padding
    /// into a solid block of background the full width of its column. `Acct`
    /// is its header's four wide holding three characters and `Amount`
    /// twelve holding nine, and the two sit side by side here with nothing
    /// between them, so
    /// a negative row read as one wide column of the wrong color. Only
    /// negative rows showed it, because a negative amount is the only figure
    /// in that column carrying a foreground at all.
    ///
    /// Stated as "no space is colored", which is the general fact: it holds
    /// for the columns that abut and for the ones that do not.
    #[test]
    fn the_cursor_row_tints_its_characters_and_not_its_padding() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;

        let mut list = screen();
        // Mortgage: the negative row, and the one the screenshots showed.
        list.select_next();
        assert!(list.selected().unwrap().cents < Cents::ZERO);

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &list);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // Row 0 is the border and row 1 the header, so the cursor sits on
        // row 3 -- the second data row.
        let y = 3;
        let row: String = (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect();
        assert!(row.contains("Mortgage"), "{row:?}");

        for x in 0..MIN_WIDTH {
            let cell = &buffer[(x, y)];
            if cell.symbol() == " " {
                assert_eq!(
                    cell.fg,
                    Color::Reset,
                    "padding at column {x} is tinted: {row:?}"
                );
            }
        }

        // And the characters themselves still carry their colors -- the fix
        // narrows the tint rather than dropping it.
        let colored = |needle: &str, color: Color| {
            assert_eq!(
                buffer[(super::super::column_of(&row, needle), y)].fg,
                color,
                "{needle:?} is not tinted in {row:?}"
            );
        };
        colored(
            "CHK",
            super::super::style::account_color(AccountId(1), None),
        );
        colored("-1,200.00", super::super::style::NEGATIVE);
    }

    /// `Amount` and `Rows` are the two right-aligned columns, so they are the
    /// two headers that must end where their figures do. `Last` is left with
    /// its cells, which the local `optional` does not right-align.
    #[test]
    fn the_right_aligned_headers_end_where_their_own_columns_do() {
        let lines = drawn(&screen());
        let header = super::super::ends_in_order(
            &lines[1],
            &[
                "Description",
                "Acct",
                "Amount",
                "Cadence",
                "Anchor",
                "Last",
                "Rows",
            ],
        );
        let row = super::super::ends_in_order(
            &lines[2],
            &[
                "Salary",
                "CHK",
                "5,000.00",
                "biweekly",
                "2026-08-28",
                "2026-12-18",
                "9",
            ],
        );
        assert_eq!(header[2], row[2], "Amount over {:?}", lines[2]);
        assert_eq!(header[6], row[6], "Rows over {:?}", lines[2]);
    }

    /// The widest this screen's `Acct` column goes: a code both kinds hold,
    /// so both rows fall back to a name and a kind. Widening a column is
    /// paid for out of `Description`, the one `Min` on the screen, and the
    /// right-aligned columns lose their *leading* characters when they are
    /// cut -- a wrong figure rather than a visible ellipsis.
    #[test]
    fn a_widened_account_column_is_drawn_whole_at_the_minimum_width() {
        let mut list = RecurringTxns::new(vec![cash(1, "CHK"), credit(2, "CHK")]);
        list.set_recurring_txns(
            vec![recurring_txn(1, "Salary", 500_000, Cadence::Biweekly, true)],
            HashMap::from([(RecurringTxnId(1), 9)]),
            HashMap::from([(RecurringTxnId(1), day(2026, 12, 18))]),
        );
        let lines = drawn(&list);
        assert!(lines[2].contains("Everyday — Cash"), "{:?}", lines[2]);
        let header = super::super::ends_in_order(&lines[1], &["Amount", "Rows"]);
        let row = super::super::ends_in_order(&lines[2], &["5,000.00", "9"]);
        assert_eq!(header, row, "{:?}", lines[2]);
    }
}
