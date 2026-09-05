//! The two forms the ledgers open, as plain state machines.
//!
//! Beside [`super::ledger`] the way [`super::goal_form`] sits beside
//! [`super::savings`]: the screen is one file, the forms it opens are
//! another, and both are written against the field framework in
//! [`super::form`].
//!
//! No ratatui in any signature here except the two render functions at the
//! bottom: the parsing, the validation, and the suggestion rules are the
//! parts with decisions in them, and they are unit-tested directly.

use super::Account;
use super::Label;
use super::autocomplete::Autocomplete;
use super::form::{DateField, Field, Focused, FormFields, Step, next_in, parse_amount, step_index};
use super::widget::{field_stack, render_fields, render_popup};
use crate::db::account::{self, Kind};
use crate::db::txn::{NewTxn, Suggestion, Txn};
use crate::db::{AccountId, TxnId};
use crate::money::Cents;
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use ratatui::Frame;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TxnField {
    Date,
    Account,
    Amount,
    Description,
}

impl TxnField {
    /// Tab order, and the order the fields render in.
    ///
    /// The account and the date are the two that arrive prefilled -- the
    /// account from the ledger's own filter -- so they lead, and the form
    /// reads as two defaults to scan and then two fields to fill.
    ///
    /// The description comes before the amount because accepting a suggestion
    /// fills the amount: with the amount first, reaching the description
    /// meant tabbing *through* a field the suggestion was about to write
    /// anyway, and tabbing off the description now lands on the figure that
    /// arrived with it.
    pub const ORDER: [TxnField; 4] = [
        TxnField::Account,
        TxnField::Date,
        TxnField::Description,
        TxnField::Amount,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TxnField::Date => "Date",
            TxnField::Account => "Account",
            TxnField::Amount => "Amount",
            TxnField::Description => "Description",
        }
    }
}

/// Adding or editing one transaction. Backs `a` and `e`.
///
/// The account is a selector over the accounts this ledger may write to
/// rather than a text field, so an account that does not exist is
/// unrepresentable.
#[derive(Debug)]
pub struct TxnForm {
    /// `Some` when editing an existing row, `None` when adding one.
    pub editing: Option<TxnId>,
    pub focus: TxnField,
    date: DateField,
    amount: Field,
    description: Field,
    accounts: Vec<account::Account>,
    account: usize,
    account_touched: bool,
}

impl TxnForm {
    /// `preselected` is the account the ledger is filtered to, if any, so `a`
    /// opens on the account being looked at rather than always on the first.
    ///
    /// A filter is the owner saying which account they are entering rows for,
    /// so it arrives as a *choice* rather than a default: an accepted
    /// suggestion fills the description and the amount and leaves the account
    /// alone, the way it already leaves an amount that was typed. Under the
    /// `All` filter nothing has been said, and the selector opens on the first
    /// account as a bare default a suggestion is free to move.
    ///
    /// The date arrives already built rather than as a day to open on,
    /// because a `DateField` carries *two* dates -- the day it shows and the
    /// day its `M/D` shorthand resolves against -- and those part company the
    /// moment the form opens on anything but today. Two adjacent
    /// `NaiveDate` parameters would be one transposition away from reading a
    /// shorthand off the wrong year.
    pub(super) fn add(
        accounts: Vec<account::Account>,
        date: DateField,
        preselected: Option<AccountId>,
    ) -> Result<TxnForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no account of this kind to add a transaction to"
        );
        let filtered = preselected.and_then(|id| accounts.iter().position(|a| a.id == id));
        Ok(TxnForm {
            editing: None,
            focus: TxnField::Description,
            date,
            amount: Field::default(),
            description: Field::default(),
            accounts,
            account: filtered.unwrap_or(0),
            account_touched: filtered.is_some(),
        })
    }

    pub(super) fn edit(
        accounts: Vec<account::Account>,
        today: NaiveDate,
        txn: &Txn,
    ) -> Result<TxnForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no account of this kind to edit a transaction into"
        );
        let account = accounts
            .iter()
            .position(|a| a.id == txn.account_id)
            .unwrap_or(0);
        Ok(TxnForm {
            editing: Some(txn.id),
            focus: TxnField::Description,
            date: DateField::given(today, Some(txn.date)),
            amount: Field::given(txn.cents.to_string()),
            description: Field::given(txn.description.clone()),
            accounts,
            account,
            account_touched: true,
        })
    }

    pub fn description(&self) -> &str {
        self.description.value()
    }

    pub fn display(&self, field: TxnField) -> Label {
        match field {
            TxnField::Date => Label::from(self.date.display(self.focus == TxnField::Date)),
            TxnField::Amount => Label::from(crate::demo::typed(self.amount.value())),
            TxnField::Description => {
                Label::from(crate::demo::text(self.description.value()).into_owned())
            }
            TxnField::Account => match self.accounts.get(self.account) {
                Some(a) => Label::default().account(Account::labelled(&self.accounts, a.id)),
                None => Label::default(),
            },
        }
    }

    pub fn commit(&self) -> Result<NewTxn> {
        let account = self
            .accounts
            .get(self.account)
            .context("no account is selected")?;
        // A blank description is a supported state, not a half-entered row:
        // some rows are worth having for their amount alone. `tui::description`
        // is what draws it. Still trimmed, so nothing downstream has to ask
        // whether a description is empty or merely looks it.
        let description = self.description.value().trim().to_string();
        Ok(NewTxn {
            date: self.date.parse()?,
            cents: parse_amount(self.amount.value())?,
            account_id: account.id,
            description,
            // Hand-entered rows belong to no recurring transaction, and
            // `txn::update` ignores this field rather than detaching an edited
            // row from its own.
            recurring_txn_id: None,
        })
    }
}

impl FormFields for TxnForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&TxnField::ORDER, self.focus, step);
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            TxnField::Date => Focused::Date(&mut self.date),
            TxnField::Amount => Focused::Text(&mut self.amount),
            TxnField::Description => Focused::Text(&mut self.description),
            TxnField::Account => Focused::Selector,
        }
    }

    fn cycle(&mut self, step: Step) {
        self.account = step_index(self.account, self.accounts.len(), step.direction());
        self.account_touched = true;
    }

    fn suggestion_prefix(&self) -> Option<&str> {
        (self.focus == TxnField::Description).then(|| self.description.value())
    }

    /// Take a suggestion: the description always, the account and amount only
    /// if the user has not touched them — and a ledger filtered to one account
    /// counts as touching it.
    ///
    /// `txn::autocomplete` returns all three, and overwriting an amount the
    /// user just typed — because they then edited the description — is
    /// infuriating exactly once a month.
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransferField {
    Date,
    From,
    To,
    Amount,
    Description,
}

impl TransferField {
    /// Tab order, and the order the fields render in. The description sits
    /// ahead of the amount for the reason [`TxnField::ORDER`] gives: a
    /// suggestion accepted in the description fills the amount behind it.
    pub const ORDER: [TransferField; 5] = [
        TransferField::Date,
        TransferField::From,
        TransferField::To,
        TransferField::Description,
        TransferField::Amount,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TransferField::Date => "Date",
            TransferField::From => "From",
            TransferField::To => "To",
            TransferField::Amount => "Amount",
            TransferField::Description => "Description",
        }
    }
}

/// A committed transfer, ready for `txn::insert_transfer`.
///
/// One description, used for both legs: the workbook's 165 transfer rows all
/// carry the same string on both sides, and a second field would be tabbed
/// past every time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub from_account_id: AccountId,
    pub to_account_id: AccountId,
    pub date: NaiveDate,
    pub cents: Cents,
    pub description: String,
}

/// Which key opened the form, and therefore which accounts it offers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TransferKind {
    Transfer,
    Payment,
}

/// Where a `From` selector opens: the default account if the list holds it,
/// and the head of the list otherwise.
///
/// The fallback is what makes an unset key and a stale one behave alike, and
/// deliberately so. This is a lookup rather than a resolution: nothing is
/// spent on the answer, the owner can see which account the selector landed
/// on before pressing Enter, and a form that refused to open over a prefill
/// would take away the one screen the setting is corrected from.
fn opening_index(accounts: &[account::Account], default: Option<AccountId>) -> usize {
    default
        .and_then(|id| accounts.iter().position(|a| a.id == id))
        .unwrap_or_default()
}

/// The two selector lists as one, for [`TransferForm::spelling`], which says
/// why the union rather than either half is what an account is spelled
/// against.
fn spelling(from: &[account::Account], to: &[account::Account]) -> Vec<account::Account> {
    from.iter().chain(to).cloned().collect()
}

/// Moving money between two accounts. Backs `t` and `p`.
#[derive(Debug)]
pub struct TransferForm {
    pub focus: TransferField,
    kind: TransferKind,
    date: DateField,
    amount: Field,
    description: Field,
    from_accounts: Vec<account::Account>,
    from: usize,
    to_accounts: Vec<account::Account>,
    to: usize,
    /// The two lists above as one, and the list `display` spells an account
    /// against.
    ///
    /// They are filtered differently: `t` draws its source from cash and its
    /// destination from every account, and `p` draws them from two lists
    /// disjoint by kind. So a field resolved against its own list is a modal
    /// that can spell one account two ways — `CHK` in the cash-only `From`
    /// and `CHK — Cash` in the mixed `To` — or, under `p`, spell two
    /// accounts one way, since a code names one account per kind and `p`
    /// puts one kind in each field. Both are the ambiguity
    /// `Account::distinctly` exists to remove, arriving through the one door
    /// it cannot see: the collision set is the list it is handed, and here
    /// neither list is the set the owner is reading.
    ///
    /// A row in both lists is left in rather than deduplicated: `distinctly`
    /// compares an account against every id but its own, so a second copy of
    /// the account being spelled is inert.
    spelling: Vec<account::Account>,
}

impl TransferForm {
    /// `t`: cash to any other account, prefilled `Transfer`.
    ///
    /// The source is restricted to cash even though `insert_transfer` signs
    /// each leg from its own account's kind. Money moving *out* of a
    /// card is a cash advance or a balance transfer — real things, but ones
    /// the owner records as such rather than stumbling into from the general
    /// transfer key. Keeping the source cash-only means every `t` reads the
    /// same way: something left an account you hold and arrived somewhere
    /// else.
    ///
    /// `default_from` is [`crate::default_source::Source::Transfer`]'s
    /// account, and the `To` selector then opens on the first account that is
    /// not it. Both halves matter: the destination list is *every* account,
    /// so a `To` left at index zero opens the form on a transfer from an
    /// account to itself — the one pair [`TransferForm::commit`] refuses.
    pub(super) fn transfer(
        accounts: Vec<account::Account>,
        date: DateField,
        default_from: Option<AccountId>,
    ) -> Result<TransferForm> {
        ensure!(
            accounts.len() >= 2,
            "a transfer needs two different accounts"
        );
        let cash: Vec<account::Account> = accounts
            .iter()
            .filter(|a| a.kind == Kind::Cash)
            .cloned()
            .collect();
        ensure!(
            !cash.is_empty(),
            "there is no cash account to transfer from"
        );
        let from = opening_index(&cash, default_from);
        let source = cash[from].id;
        // `accounts.len() >= 2` above is what makes this land somewhere: two
        // accounts cannot both be the source.
        let to = accounts
            .iter()
            .position(|a| a.id != source)
            .unwrap_or_default();
        Ok(TransferForm {
            focus: TransferField::Date,
            kind: TransferKind::Transfer,
            date,
            amount: Field::default(),
            description: Field::prefilled("Transfer"),
            spelling: spelling(&cash, &accounts),
            from_accounts: cash,
            from,
            to_accounts: accounts,
            to,
        })
    }

    /// `p`: cash to a card, prefilled `<CODE> Payment`.
    ///
    /// The destination is restricted to credit accounts and the source to
    /// cash: a "payment" means cash settling a card, and a card-to-card move
    /// is a balance transfer, which is a different thing the owner should not
    /// reach by pressing `p`.
    ///
    /// `default_from` is [`crate::default_source::Source::Payment`]'s
    /// account. The `To` selector takes no such adjustment as `transfer`'s
    /// does: the two lists here are disjoint by kind, so no card the
    /// destination opens on can be the cash account paying it.
    pub(super) fn payment(
        accounts: Vec<account::Account>,
        date: DateField,
        default_from: Option<AccountId>,
    ) -> Result<TransferForm> {
        let cards: Vec<account::Account> = accounts
            .iter()
            .filter(|a| a.kind == Kind::Credit)
            .cloned()
            .collect();
        ensure!(!cards.is_empty(), "there is no credit account to pay");
        let cash: Vec<account::Account> = accounts
            .into_iter()
            .filter(|a| a.kind == Kind::Cash)
            .collect();
        ensure!(!cash.is_empty(), "there is no cash account to pay from");

        let mut form = TransferForm {
            focus: TransferField::Date,
            kind: TransferKind::Payment,
            date,
            amount: Field::default(),
            description: Field::default(),
            from: opening_index(&cash, default_from),
            spelling: spelling(&cash, &cards),
            from_accounts: cash,
            to_accounts: cards,
            to: 0,
        };
        form.refresh_payment_description();
        Ok(form)
    }

    /// What the form's title bar calls it, which is the key that opened it.
    ///
    /// One form backs `t` and `p`, and the two write different things: a
    /// payment is cash settling a card, and a modal titled `Transfer` over a
    /// `<CODE> Payment` description tells the owner they pressed the wrong
    /// key.
    pub fn title(&self) -> &'static str {
        match self.kind {
            TransferKind::Transfer => {
                "Transfer — Tab field · ←/→ account · Enter save · Esc cancel"
            }
            TransferKind::Payment => "Payment — Tab field · ←/→ account · Enter save · Esc cancel",
        }
    }

    /// Keep the prefill pointing at the card actually selected, until the
    /// user edits it. `CC1 Payment`, `CC2 Payment`, and the rest are what the
    /// workbook holds.
    fn refresh_payment_description(&mut self) {
        if self.kind != TransferKind::Payment || self.description.is_touched() {
            return;
        }
        if let Some(card) = self.to_accounts.get(self.to) {
            // prefills a description, not a display of an account
            self.description
                .fill(format!("{} Payment", card.code.as_str()));
        }
    }

    pub fn description(&self) -> &str {
        self.description.value()
    }

    pub fn display(&self, field: TransferField) -> Label {
        let account = |list: &[account::Account], i: usize| match list.get(i) {
            // Spelled against both lists, never against the one the field
            // selects from -- see `TransferForm::spelling`.
            Some(a) => Label::default().account(Account::labelled(&self.spelling, a.id)),
            None => Label::default(),
        };
        match field {
            TransferField::Date => {
                Label::from(self.date.display(self.focus == TransferField::Date))
            }
            TransferField::Amount => Label::from(crate::demo::typed(self.amount.value())),
            TransferField::Description => {
                Label::from(crate::demo::text(self.description.value()).into_owned())
            }
            TransferField::From => account(&self.from_accounts, self.from),
            TransferField::To => account(&self.to_accounts, self.to),
        }
    }

    pub fn commit(&self) -> Result<Transfer> {
        let from = self
            .from_accounts
            .get(self.from)
            .context("no source account is selected")?;
        let to = self
            .to_accounts
            .get(self.to)
            .context("no destination account is selected")?;
        ensure!(
            from.id != to.id,
            "a transfer needs two different accounts, not {} twice",
            // names the source in an error about a transfer to itself
            crate::demo::text(from.code.as_str())
        );
        let cents = parse_amount(self.amount.value())?;
        // `insert_transfer` applies the sign per the destination's kind, so a
        // negative magnitude writes both legs the wrong way round.
        ensure!(
            cents > Cents::ZERO,
            "amount must be positive, got {}",
            crate::demo::figure(cents)
        );
        let description = self.description.value().trim().to_string();
        ensure!(!description.is_empty(), "description must not be empty");
        Ok(Transfer {
            from_account_id: from.id,
            to_account_id: to.id,
            date: self.date.parse()?,
            cents,
            description,
        })
    }
}

impl FormFields for TransferForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&TransferField::ORDER, self.focus, step);
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            TransferField::Date => Focused::Date(&mut self.date),
            TransferField::Amount => Focused::Text(&mut self.amount),
            TransferField::Description => Focused::Text(&mut self.description),
            TransferField::From | TransferField::To => Focused::Selector,
        }
    }

    fn cycle(&mut self, step: Step) {
        match self.focus {
            TransferField::From => {
                self.from = step_index(self.from, self.from_accounts.len(), step.direction())
            }
            TransferField::To => {
                self.to = step_index(self.to, self.to_accounts.len(), step.direction());
                self.refresh_payment_description();
            }
            TransferField::Date | TransferField::Amount | TransferField::Description => {}
        }
    }

    fn suggestion_prefix(&self) -> Option<&str> {
        (self.focus == TransferField::Description).then(|| self.description.value())
    }

    /// Fills only the description and the amount: a suggestion's account is
    /// one side of a one-sided row, and there is no telling which side of a
    /// transfer it belongs on.
    fn apply_suggestion(&mut self, hit: &Suggestion) {
        self.description.fill(hit.description.clone());
        if !self.amount.is_touched() {
            self.amount.fill(hit.cents.to_string());
        }
    }
}

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_txn(frame: &mut Frame, form: &mut TxnForm, popup: &Autocomplete) -> usize {
    let title = if form.editing.is_some() {
        "Edit transaction — Tab field · Enter save · Esc cancel"
    } else {
        "Add transaction — Tab field · Enter save · Esc cancel"
    };
    let caret = form.caret();
    let lines = field_stack(
        &TxnField::ORDER,
        form.focus,
        caret,
        TxnField::label,
        |f| form.display(f),
        &[],
    );
    let area = render_fields(frame, title, lines);
    render_popup(frame, area, popup)
}

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_transfer(frame: &mut Frame, form: &mut TransferForm, popup: &Autocomplete) -> usize {
    let caret = form.caret();
    let lines = field_stack(
        &TransferField::ORDER,
        form.focus,
        caret,
        TransferField::label,
        |f| form.display(f),
        &[],
    );
    let area = render_fields(frame, form.title(), lines);
    render_popup(frame, area, popup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::Group;
    use crate::db::{AccountId, RecurringTxnId};
    use crate::test_support::{cash, credit, day, walk_until};
    use crate::tui::form::{backspace_key, char_key};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::Modifier;

    /// `Shift` with an arrow is the same nudge on the same key, a week at a
    /// time.
    #[test]
    fn shift_steps_a_transaction_date_a_week() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.choice(Step::NEXT_WEEK);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-22");
        form.choice(Step::PREVIOUS_WEEK);
        form.choice(Step::PREVIOUS_WEEK);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-08");
    }

    /// `[`/`]` move a whole month, which is why a step carries a unit rather
    /// than a count of days: August steps to September over 31 of them and
    /// February over 28.
    #[test]
    fn brackets_step_a_transaction_date_a_month() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        assert!(form.step_month(Step::NEXT_MONTH));
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-09-15");
        assert!(form.step_month(Step::PREVIOUS_MONTH));
        assert!(form.step_month(Step::PREVIOUS_MONTH));
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-07-15");
    }

    /// A month is not a fixed length, so the day is clamped into the month it
    /// lands in and stepping back does not return to where it came from.
    /// There is no other answer: the 31st of September is not a date.
    #[test]
    fn a_month_step_onto_a_shorter_month_clamps_the_day_and_does_not_come_back() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 31)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.step_month(Step::NEXT_MONTH);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-09-30");
        form.step_month(Step::PREVIOUS_MONTH);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-30");
    }

    /// The difference from the arrows: a bracket is an ordinary character in
    /// a description, so a form with no date under the caret reports that it
    /// stepped nothing and the handler types the key instead.
    #[test]
    fn a_month_step_off_a_date_is_refused_so_the_bracket_stays_a_character() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Description);
        assert!(!form.step_month(Step::NEXT_MONTH));
        focused(&mut form, TxnField::Account);
        assert!(!form.step_month(Step::NEXT_MONTH));

        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Coffee");
        let committed = form.commit().unwrap();
        assert_eq!(committed.date, day(2026, 8, 15));
        assert_eq!(committed.account_id, AccountId(1));
    }

    /// A selector has no week to move, so it takes the direction and ignores
    /// the size. A modified arrow that did nothing would be a dead key on the
    /// very field the hand reaches for it on.
    #[test]
    fn a_week_step_moves_a_selector_one_choice_like_a_plain_arrow() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Account);
        form.choice(Step::NEXT_WEEK);
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Transfer in");

        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// The date and the account are both reachable by the arrows, so a week
    /// pressed on one must not reach the other.
    #[test]
    fn a_week_step_on_the_date_leaves_the_account_selector_alone() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.choice(Step::NEXT_WEEK);
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Coffee");

        let committed = form.commit().unwrap();
        // The step is what the test is about, and `choice` is a no-op off the
        // date, so a form focused elsewhere would pass this having stepped
        // nothing at all.
        assert_eq!(committed.date, day(2026, 8, 22));
        assert_eq!(committed.account_id, AccountId(1));
    }

    fn accounts() -> Vec<account::Account> {
        vec![cash(1, "CHK"), cash(2, "SAV")]
    }

    fn all_accounts() -> Vec<account::Account> {
        let mut all = accounts();
        all.push(account::Account {
            id: AccountId(3),
            code: "CC1".into(),
            name: "Card One".into(),
            kind: Kind::Credit,
            sort: 0,
            group: Group::Credit,
            color: None,
        });
        all.push(account::Account {
            id: AccountId(4),
            code: "CC2".into(),
            name: "Card Two".into(),
            kind: Kind::Credit,
            sort: 1,
            group: Group::Credit,
            color: None,
        });
        all
    }

    fn today() -> NaiveDate {
        day(2026, 8, 15)
    }

    fn suggestion(description: &str, account_id: AccountId, cents: i64) -> Suggestion {
        Suggestion {
            description: description.to_string(),
            account_id,
            cents: Cents(cents),
            uses: 4,
        }
    }

    /// Tab to `field`. The form opens on its description, so a test about any
    /// other field says which one it means rather than relying on where the
    /// caret happens to start.
    fn focused(form: &mut TxnForm, field: TxnField) {
        walk_until!(form.focus == field, form.next_field());
    }

    fn typed(form: &mut TxnForm, field: TxnField, text: &str) {
        focused(form, field);
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    fn typed_transfer(form: &mut TransferForm, field: TransferField, text: &str) {
        walk_until!(form.focus == field, form.next_field());
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    /// A transaction's account is the same account the ledger's Account column
    /// names behind the form, so it is the same color. The selector shows a
    /// code and a name, and both are that account.
    #[test]
    fn the_account_selector_shows_one_colored_account() {
        let form = TxnForm::add(accounts(), DateField::today(today()), None).unwrap();
        let value = form.display(TxnField::Account);
        assert_eq!(value.plain_text(), "CHK — Everyday");
        assert_eq!(value.accounts().len(), 1);
        assert_eq!(value.accounts()[0].id(), AccountId(1));
    }

    /// A date is not an account and takes no color. The uniform `Label`
    /// return is about having one shape per form, not about tinting every
    /// field.
    #[test]
    fn a_forms_ordinary_fields_name_no_account() {
        let form = TxnForm::add(accounts(), DateField::today(today()), None).unwrap();
        assert!(form.display(TxnField::Date).accounts().is_empty());
        assert!(form.display(TxnField::Amount).accounts().is_empty());
        assert!(form.display(TxnField::Description).accounts().is_empty());
    }

    /// Both ends of a transfer, so money moving between two containers is
    /// readable at a glance rather than by reading two codes.
    #[test]
    fn both_ends_of_a_transfer_name_their_own_account() {
        let form = TransferForm::transfer(all_accounts(), DateField::today(today()), None).unwrap();
        let from = form.display(TransferField::From);
        let to = form.display(TransferField::To);
        assert_eq!(from.accounts().len(), 1);
        assert_eq!(to.accounts().len(), 1);
    }

    /// Every account an import has just written is named after its own
    /// code, which is the state `Account::labelled` collapses to one word --
    /// so it is the state a code both kinds hold leaves two accounts sharing
    /// that word.
    fn imported(a: account::Account) -> account::Account {
        account::Account {
            name: a.code.as_str().into(),
            ..a
        }
    }

    /// `t` filters its source to cash and leaves its destination unfiltered,
    /// so a code both kinds hold collides in `To` and not in `From`.
    /// Resolved against its own list each, the two lines of one modal spell
    /// the same account `CHK` and `CHK — Cash`.
    #[test]
    fn both_ends_of_a_transfer_spell_one_account_the_same_way() {
        let all = vec![imported(cash(1, "CHK")), imported(credit(2, "CHK"))];
        let mut form = TransferForm::transfer(all, DateField::today(today()), None).unwrap();
        // Onto the source, the one account both selectors can name.
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(TransferField::From).plain_text(), "CHK — Cash");
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            form.display(TransferField::From).plain_text()
        );
    }

    /// `p`'s two lists are disjoint by kind, which is the same gap read the
    /// other way round: neither list holds a collision, so a modal resolving
    /// each against its own would spell two *different* accounts `CHK` on
    /// adjacent lines.
    #[test]
    fn a_payment_between_a_code_both_kinds_hold_spells_the_two_apart() {
        let all = vec![imported(cash(1, "CHK")), imported(credit(2, "CHK"))];
        let form = TransferForm::payment(all, DateField::today(today()), None).unwrap();
        assert_eq!(form.display(TransferField::From).plain_text(), "CHK — Cash");
        assert_eq!(form.display(TransferField::To).plain_text(), "CHK — Credit");
    }

    #[test]
    fn add_prefills_todays_date_and_commits_what_was_typed() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-15");

        typed(&mut form, TxnField::Amount, "$1,234.5");
        typed(&mut form, TxnField::Description, "Whole Foods");

        let new = form.commit().unwrap();
        assert_eq!(new.date, day(2026, 8, 15));
        assert_eq!(new.cents, Cents(123_450));
        assert_eq!(new.account_id, AccountId(1));
        assert_eq!(new.description, "Whole Foods");
        assert_eq!(new.recurring_txn_id, None);
    }

    #[test]
    fn a_date_that_is_not_yyyy_mm_dd_is_refused_with_the_text_that_failed() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Coffee");
        focused(&mut form, TxnField::Date);
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        for c in "08/15/2026".chars() {
            form.edit(char_key(c));
        }

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("08/15/2026"), "{err}");
    }

    /// A cash withdrawal or a card charge whose merchant is on the receipt
    /// and nowhere worth retyping. The row is worth having for its amount
    /// alone, and a form that refuses it is a form the owner works around by
    /// typing a placeholder that is worse than the blank.
    #[test]
    fn a_ledger_row_may_be_committed_with_no_description() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Amount, "10");

        let new = form.commit().unwrap();
        assert_eq!(new.description, "");
        assert_eq!(new.cents, Cents(1_000));
    }

    /// Whitespace is stored as the blank it is, so nothing downstream has to
    /// ask whether a description is empty or merely looks it.
    #[test]
    fn a_description_of_nothing_but_spaces_is_stored_as_empty() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "   ");

        assert_eq!(form.commit().unwrap().description, "");
    }

    #[test]
    fn the_account_selector_cycles_through_this_kinds_accounts() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Account);
        form.choice(Step::NEXT);
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Transfer in");

        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// `↑`/`↓` on the description must not spin the account selector, which
    /// is what a shared "cycle" key would do.
    #[test]
    fn cycling_does_nothing_unless_a_selector_is_focused() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Description, "Coffee");
        form.choice(Step::NEXT);
        form.choice(Step::NEXT);
        typed(&mut form, TxnField::Amount, "10");

        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

    #[test]
    fn accepting_a_suggestion_fills_an_untouched_account_and_amount() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        assert_eq!(form.display(TxnField::Description).plain_text(), "Movies");
        assert_eq!(form.display(TxnField::Amount).plain_text(), "14.99");
        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// Overwriting an amount the user just typed, because they then edited
    /// the description, is infuriating exactly once a month.
    #[test]
    fn accepting_a_suggestion_leaves_a_typed_amount_alone() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Amount, "22.50");
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        assert_eq!(form.display(TxnField::Description).plain_text(), "Movies");
        assert_eq!(form.display(TxnField::Amount).plain_text(), "22.50");
        assert_eq!(
            form.commit().unwrap().account_id,
            AccountId(2),
            "an untouched account still takes the suggestion's"
        );
    }

    /// A row already on screen has a real amount in it. Accepting a
    /// suggestion while fixing its description must not rewrite the figure
    /// the user can see and did not ask to change.
    #[test]
    fn editing_a_row_prefills_it_and_protects_its_amount_from_suggestions() {
        let row = Txn {
            id: TxnId(7),
            date: day(2026, 1, 2),
            cents: Cents(499_999),
            account_id: AccountId(2),
            description: "Paychek".to_string(),
            recurring_txn_id: Some(RecurringTxnId(1)),
            edited: false,
        };
        let mut form = TxnForm::edit(accounts(), today(), &row).unwrap();

        assert_eq!(form.editing, Some(TxnId(7)));
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-01-02");
        assert_eq!(form.display(TxnField::Amount).plain_text(), "4,999.99");

        form.apply_suggestion(&suggestion("Paycheck", AccountId(1), 500_000));

        assert_eq!(form.display(TxnField::Description).plain_text(), "Paycheck");
        assert_eq!(form.display(TxnField::Amount).plain_text(), "4,999.99");
        let new = form.commit().unwrap();
        assert_eq!(new.cents, Cents(499_999));
        assert_eq!(new.account_id, AccountId(2));
    }

    /// `a` on a ledger filtered to one account opens on that account.
    #[test]
    fn adding_with_a_preselected_account_opens_on_it() {
        let form = TxnForm::add(
            accounts(),
            DateField::today(day(2026, 8, 15)),
            Some(AccountId(2)),
        )
        .unwrap();
        assert_eq!(
            form.display(TxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
    }

    /// Entering rows from a filtered ledger is a statement about which
    /// account they land in, so a suggestion off another account brings its
    /// description and amount and nothing else.
    #[test]
    fn a_preselected_account_survives_an_accepted_suggestion() {
        let mut form = TxnForm::add(
            accounts(),
            DateField::today(day(2026, 8, 15)),
            Some(AccountId(2)),
        )
        .unwrap();
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(1), 1_499));

        assert_eq!(form.display(TxnField::Amount).plain_text(), "14.99");
        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// The `All` filter names no account, so the selector is still the bare
    /// default a suggestion may move.
    #[test]
    fn an_unfiltered_ledger_still_yields_its_account_to_a_suggestion() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// A filter that names an account this form cannot write to — a stale id,
    /// or the wrong kind — must not leave the selector pointing at nothing.
    #[test]
    fn a_preselected_account_that_is_not_on_offer_falls_back_to_the_first() {
        let form = TxnForm::add(
            accounts(),
            DateField::today(day(2026, 8, 15)),
            Some(AccountId(99)),
        )
        .unwrap();
        assert_eq!(
            form.display(TxnField::Account).plain_text(),
            "CHK — Everyday"
        );
    }

    #[test]
    fn a_form_with_no_accounts_to_write_to_is_refused() {
        let err = TxnForm::add(Vec::new(), DateField::today(day(2026, 8, 15)), None).unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    #[test]
    fn a_transfer_prefills_the_description_both_legs_share() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "Transfer"
        );

        typed_transfer(&mut form, TransferField::Amount, "3,291.00");

        let moved = form.commit().unwrap();
        assert_eq!(moved.from_account_id, AccountId(1));
        assert_eq!(moved.to_account_id, AccountId(2));
        assert_eq!(moved.cents, Cents(329_100));
        assert_eq!(moved.description, "Transfer");
    }

    /// `write_transfer`'s `ensure!`, reached through `insert_transfer`, is a
    /// backstop against writing both legs the wrong way round. The user
    /// should see this in the form, not as an error surfaced from three
    /// layers down.
    #[test]
    fn a_transfer_of_a_non_positive_amount_is_refused_in_the_form() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        typed_transfer(&mut form, TransferField::Amount, "-100");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("amount must be positive"), "{err}");
    }

    /// Reached by cycling `To` back onto the source, which is the only way
    /// to reach it now that the form no longer *opens* there.
    #[test]
    fn a_transfer_to_the_account_it_came_from_is_refused() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::PREVIOUS);
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            form.display(TransferField::From).plain_text()
        );
        typed_transfer(&mut form, TransferField::Amount, "100");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("two different accounts"), "{err}");
    }

    /// The `To` selector opens off the source rather than on it: the
    /// destination list is every account, so index zero is the cash account
    /// the money is leaving, and the form would open on a pair `commit`
    /// refuses.
    #[test]
    fn a_transfer_opens_on_two_different_accounts() {
        let form = TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
            .unwrap();
        let moved = commit_with(form, "100");
        assert_eq!(moved.from_account_id, all_accounts()[0].id);
        assert_ne!(moved.to_account_id, moved.from_account_id);
    }

    /// The account [`crate::default_source::Source::Transfer`] names is where
    /// `From` opens, and `To` steps off *that* account rather than off the
    /// head of the list.
    #[test]
    fn a_transfer_opens_on_the_default_source_and_away_from_it() {
        let default = all_accounts()[1].id;
        let form = TransferForm::transfer(
            all_accounts(),
            DateField::today(day(2026, 8, 31)),
            Some(default),
        )
        .unwrap();
        let moved = commit_with(form, "100");
        assert_eq!(moved.from_account_id, default);
        assert_ne!(moved.to_account_id, default);
    }

    /// An unset key and one naming an account that is gone are the same
    /// state to a prefill: the head of the list, and a form the owner can
    /// still see and correct.
    #[test]
    fn a_transfer_whose_default_source_is_not_a_cash_account_opens_on_the_first() {
        let form = TransferForm::transfer(
            all_accounts(),
            DateField::today(day(2026, 8, 31)),
            Some(AccountId(9_999)),
        )
        .unwrap();
        assert_eq!(
            commit_with(form, "100").from_account_id,
            all_accounts()[0].id
        );
    }

    /// `p`'s own key, and not `t`'s: the card a payment settles and the
    /// account savings leave are two decisions, so the two forms open on
    /// whichever account each was pointed at.
    #[test]
    fn a_payment_opens_on_its_own_default_source() {
        let default = accounts()[1].id;
        let form = TransferForm::payment(
            all_accounts(),
            DateField::today(day(2026, 9, 8)),
            Some(default),
        )
        .unwrap();
        let paid = commit_with(form, "100");
        assert_eq!(paid.from_account_id, default);
    }

    /// A committed transfer, typed into an otherwise untouched form.
    fn commit_with(mut form: TransferForm, amount: &str) -> Transfer {
        typed_transfer(&mut form, TransferField::Amount, amount);
        form.commit().unwrap()
    }

    /// The refusal quotes the account's own code back, and a demo has to
    /// hide it exactly as it hides one drawn anywhere else in the app.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_code_a_same_account_transfer_refusal_quotes() {
        crate::demo::install_with_salt(7);
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        // Back onto the source, which is the only way to the refusal now
        // that the form opens off it.
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::PREVIOUS);
        typed_transfer(&mut form, TransferField::Amount, "100");

        let err = form.commit().unwrap_err().to_string();
        assert!(!err.contains("CHK"), "the code survived: {err}");
        assert!(
            err.contains(&crate::demo::text("CHK").to_string()),
            "no scrambled code found: {err}"
        );
    }

    /// `insert_transfer` already handles the sign — a credit destination
    /// sheds debt, so both legs come out negative — but the form must not
    /// offer a destination that would make that wrong.
    #[test]
    fn a_payment_offers_only_credit_destinations() {
        let form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            "CC1 — Card One"
        );

        let mut form = form;
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            "CC2 — Card Two"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            "CC1 — Card One",
            "the cycle must not reach a cash account"
        );
    }

    /// `t` is for money leaving an account you hold. Moving money *out* of a
    /// card is a cash advance or a balance transfer -- `insert_transfer` signs
    /// those correctly now, but they are deliberate acts, not somewhere the
    /// general transfer key should let you wander by cycling the selector.
    #[test]
    fn a_transfer_offers_only_cash_sources() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        walk_until!(form.focus == TransferField::From, form.next_field());
        let mut seen = Vec::new();
        for _ in 0..all_accounts().len() {
            seen.push(form.display(TransferField::From).plain_text());
            form.choice(Step::NEXT);
        }
        assert_eq!(
            seen,
            vec![
                "CHK — Everyday".to_string(),
                "SAV — Rainy Day".to_string(),
                "CHK — Everyday".to_string(),
                "SAV — Rainy Day".to_string(),
            ],
            "the source cycle must never reach a card"
        );
    }

    /// The destination is deliberately *not* restricted: cash to a card is a
    /// payment, which `t` may write as well as `p`.
    #[test]
    fn a_transfer_still_offers_every_destination() {
        let form = TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
            .unwrap();
        let mut form = form;
        walk_until!(form.focus == TransferField::To, form.next_field());
        let mut seen = Vec::new();
        for _ in 0..all_accounts().len() {
            seen.push(form.display(TransferField::To).plain_text());
            form.choice(Step::NEXT);
        }
        assert!(
            seen.contains(&"CC1 — Card One".to_string()),
            "a card must remain reachable as a destination: {seen:?}"
        );
    }

    /// The ledger's own `a` accepts a blank description; a transfer does not.
    /// Both legs of a transfer are written from one string, and a pair of
    /// unnamed rows in two different accounts is the one shape the owner
    /// cannot reconstruct from the ledger later -- so the prefill this form
    /// arrives with has to be replaced rather than merely cleared.
    #[test]
    fn a_transfer_with_its_prefilled_description_cleared_is_refused() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        typed_transfer(&mut form, TransferField::Amount, "10");
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        // moves the focus without changing the field
        typed_transfer(&mut form, TransferField::Description, "");
        for _ in 0.."Transfer".len() {
            form.edit(backspace_key());
        }

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("description"), "{err}");
    }

    #[test]
    fn a_transfer_with_no_cash_account_to_move_from_is_refused() {
        let cards: Vec<account::Account> = all_accounts()
            .into_iter()
            .filter(|a| a.kind == Kind::Credit)
            .collect();
        let err =
            TransferForm::transfer(cards, DateField::today(day(2026, 8, 31)), None).unwrap_err();
        assert!(err.to_string().contains("cash"), "{err}");
    }

    /// Paying a card from another card writes a negative on both legs,
    /// shedding debt twice and inventing money.
    #[test]
    fn a_payment_offers_only_cash_sources() {
        let mut form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        assert_eq!(
            form.display(TransferField::From).plain_text(),
            "CHK — Everyday"
        );
        walk_until!(form.focus == TransferField::From, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::From).plain_text(),
            "SAV — Rainy Day"
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::From).plain_text(),
            "CHK — Everyday"
        );
    }

    #[test]
    fn a_payments_description_follows_the_card_until_it_is_edited() {
        let mut form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "CC1 Payment"
        );

        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "CC2 Payment"
        );

        typed_transfer(&mut form, TransferField::Description, "!");
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "CC2 Payment!"
        );

        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "CC2 Payment!",
            "an edited description must stop following the card"
        );
    }

    #[test]
    fn a_payment_commits_both_legs_worth_of_detail() {
        let mut form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        walk_until!(form.focus == TransferField::From, form.next_field());
        form.choice(Step::NEXT);
        typed_transfer(&mut form, TransferField::Amount, "450.85");

        let paid = form.commit().unwrap();
        assert_eq!(paid.from_account_id, AccountId(2));
        assert_eq!(paid.to_account_id, AccountId(3));
        assert_eq!(paid.date, day(2026, 9, 8));
        assert_eq!(paid.cents, Cents(45_085));
        assert_eq!(paid.description, "CC1 Payment");
    }

    #[test]
    fn a_payment_with_no_card_to_pay_is_refused() {
        let err =
            TransferForm::payment(accounts(), DateField::today(day(2026, 9, 8)), None).unwrap_err();
        assert!(err.to_string().contains("credit"), "{err}");
    }

    /// A form opens prefilled on an edit, so the field is where the row's own
    /// amount and description would otherwise be published to whoever is
    /// watching. What is in the buffer is untouched -- the form still commits
    /// the real figure and the real text -- and the date beside them is not.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amount_and_description_a_transaction_form_shows() {
        crate::demo::install_with_salt(7);
        let txn = Txn {
            id: TxnId(1),
            date: day(2026, 8, 15),
            cents: Cents(123_456),
            account_id: AccountId(1),
            description: "Groceries".to_string(),
            recurring_txn_id: None::<RecurringTxnId>,
            edited: false,
        };
        let form = TxnForm::edit(accounts(), day(2026, 8, 15), &txn).unwrap();

        let drawn = form.display(TxnField::Amount).plain_text();
        assert_ne!(drawn, "1,234.56");
        assert_eq!(drawn.len(), "1,234.56".len());
        let drawn_description = form.display(TxnField::Description).plain_text();
        assert_ne!(drawn_description, "Groceries");
        assert_eq!(drawn_description, crate::demo::text("Groceries"));
        let committed = form.commit().unwrap();
        assert_eq!(committed.cents, Cents(123_456));
        assert_eq!(committed.description, "Groceries");
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amount_a_transfer_form_shows() {
        crate::demo::install_with_salt(7);
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        typed_transfer(&mut form, TransferField::Amount, "3,291.00");
        let drawn = form.display(TransferField::Amount).plain_text();
        assert_ne!(drawn, "3,291.00");
        assert_eq!(drawn.len(), "3,291.00".len());
    }

    /// A payment's description is prefilled from the card's own code --
    /// `CC1 Payment` -- which is an account code, one of the four categories
    /// banned outright, so a demo has to hide it exactly as it hides a code
    /// drawn anywhere else.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_cards_code_in_a_payments_description() {
        crate::demo::install_with_salt(7);
        let form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        let drawn = form.display(TransferField::Description).plain_text();
        assert_ne!(drawn, "CC1 Payment");
        assert_eq!(drawn, crate::demo::text("CC1 Payment"));
        // The buffer is untouched: Enter still writes the real description.
        assert_eq!(form.description(), "CC1 Payment");
    }

    /// The refusal quotes the amount back, and a status line is on screen as
    /// surely as a column is.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_figure_a_refused_amount_quotes() {
        crate::demo::install_with_salt(7);
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        walk_until!(form.focus == TransferField::To, form.next_field());
        form.choice(Step::NEXT);
        typed_transfer(&mut form, TransferField::Amount, "-500");
        let err = form.commit().unwrap_err().to_string();
        assert!(!err.contains("500"), "the amount survived: {err}");
        assert!(
            err.contains(&crate::demo::figure(Cents::from_dollars(-500))),
            "no scrambled amount found: {err}"
        );
    }

    /// The whole frame, once: `field_line` places a caret it is handed, and
    /// this is what pairs the caret with the field that actually has the
    /// focus. A form that handed over the wrong field's caret would draw one
    /// in the right place on the wrong line.
    #[test]
    fn a_form_draws_its_caret_on_the_focused_field_and_nowhere_else() {
        use crate::tui::MIN_WIDTH;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        form.focus = TxnField::Description;
        for c in "rent".chars() {
            form.edit(char_key(c));
        }
        form.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        form.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        terminal
            .draw(|frame| {
                render_txn(frame, &mut form, &Autocomplete::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let drawn: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        let under_caret: String = buffer
            .content
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .map(|cell| cell.symbol())
            .collect();

        assert!(drawn.contains("Description  rent"), "{drawn}");
        assert_eq!(under_caret, "n", "{drawn}");
    }

    /// The description is where autocomplete lands, and accepting a
    /// suggestion fills the amount: with the amount ahead of it, reaching the
    /// description meant tabbing through a field the suggestion was about to
    /// fill in anyway.
    #[test]
    fn the_description_comes_before_the_amount_a_suggestion_fills() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        typed(&mut form, TxnField::Description, "Mov");
        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        form.next_field();
        assert_eq!(form.focus, TxnField::Amount);
    }

    /// The two fields that arrive prefilled lead, and the two the hand has to
    /// fill follow: the account comes from the ledger's own filter.
    #[test]
    fn the_prefilled_fields_come_before_the_typed_ones() {
        assert_eq!(
            TxnField::ORDER,
            [
                TxnField::Account,
                TxnField::Date,
                TxnField::Description,
                TxnField::Amount,
            ]
        );
    }

    /// `render_txn` maps over `ORDER`, which is what stops the screen and the
    /// tab key from disagreeing -- so the drawn rows are worth reading back.
    #[test]
    fn the_form_draws_its_fields_in_the_order_tab_visits_them() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                render_txn(frame, &mut form, &Autocomplete::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // The border title also carries "transaction", so the labels are
        // found by their own rows rather than by the first line matching.
        let drawn: Vec<&str> = (0..24u16)
            .map(|y| {
                (0..80u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .filter_map(|line| {
                TxnField::ORDER
                    .iter()
                    .map(|f| f.label())
                    .find(|label| line.contains(label))
            })
            .collect();

        assert_eq!(drawn, ["Account", "Date", "Description", "Amount"]);
    }

    /// The account and the date open on defaults worth accepting; the
    /// description is the first field with nothing in it, and the one
    /// autocomplete reads.
    #[test]
    fn the_add_form_opens_on_the_description() {
        let form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        assert_eq!(form.focus, TxnField::Description);
    }

    /// The date a form opens on and the day its `M/D` shorthand resolves
    /// against are two different facts, and prefilling from an earlier row
    /// must not collapse them. `9/10` typed in August is this September
    /// whatever date the field was handed: read from a December prefill it
    /// would land a year out, in silence.
    #[test]
    fn a_date_prefilled_from_an_earlier_row_still_reads_shorthand_from_today() {
        let mut form = TxnForm::add(
            accounts(),
            DateField::on(day(2026, 8, 15), day(2026, 12, 20)),
            None,
        )
        .unwrap();

        focused(&mut form, TxnField::Date);
        for _ in 0.."2026-12-20".len() {
            form.edit(backspace_key());
        }
        typed(&mut form, TxnField::Date, "9/10");
        typed(&mut form, TxnField::Description, "Kite");
        typed(&mut form, TxnField::Amount, "10");

        assert_eq!(form.commit().unwrap().date, day(2026, 9, 10));
    }

    /// One opening position for one form: an edit arrives with every field
    /// filled, so there is no second rule for it to follow.
    #[test]
    fn the_edit_form_opens_on_the_description_too() {
        let row = Txn {
            id: TxnId(1),
            date: day(2026, 1, 2),
            cents: Cents(1_000),
            account_id: AccountId(1),
            description: "Coffee".to_string(),
            recurring_txn_id: None::<RecurringTxnId>,
            edited: false,
        };
        let form = TxnForm::edit(accounts(), day(2026, 8, 15), &row).unwrap();
        assert_eq!(form.focus, TxnField::Description);
    }

    #[test]
    fn the_transfer_description_comes_before_its_amount_too() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        walk_until!(form.focus == TransferField::Description, form.next_field());
        form.next_field();
        assert_eq!(form.focus, TransferField::Amount);
    }

    #[test]
    fn the_arrows_step_a_transaction_date_by_a_day() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-16");
        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-14");
    }

    #[test]
    fn stepping_a_date_crosses_a_month_boundary() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 31)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-09-01");
    }

    /// The arrows are a nudge on a date that is already there, not a way to
    /// conjure one: a half-typed date must not be rewritten under the caret.
    #[test]
    fn the_arrows_leave_a_field_that_is_not_a_date_as_typed() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        for c in "2026-08".chars() {
            form.edit(char_key(c));
        }
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08");
    }

    /// The date and the account are both reachable by `←`/`→`, so each must
    /// stay off the other's field.
    #[test]
    fn stepping_the_date_leaves_the_account_selector_alone() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Date);
        form.choice(Step::NEXT);
        typed(&mut form, TxnField::Description, "Coffee");
        typed(&mut form, TxnField::Amount, "10");

        let committed = form.commit().unwrap();
        assert_eq!(committed.date, day(2026, 8, 16));
        assert_eq!(committed.account_id, AccountId(1));
    }

    #[test]
    fn cycling_the_account_leaves_the_date_alone() {
        let mut form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();
        focused(&mut form, TxnField::Account);
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-15");
    }

    #[test]
    fn the_arrows_step_a_transfer_date_by_a_day() {
        let mut form =
            TransferForm::transfer(all_accounts(), DateField::today(day(2026, 8, 31)), None)
                .unwrap();
        form.choice(Step::NEXT);
        assert_eq!(form.display(TransferField::Date).plain_text(), "2026-09-01");
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(TransferField::Date).plain_text(), "2026-08-31");
    }

    /// The account selectors sit on their own fields; a step on the date must
    /// not reach them, and the description must not follow a card that has
    /// not moved.
    #[test]
    fn stepping_a_transfer_date_moves_neither_account() {
        let mut form =
            TransferForm::payment(all_accounts(), DateField::today(day(2026, 9, 8)), None).unwrap();
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::From).plain_text(),
            "CHK — Everyday"
        );
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            "CC1 — Card One"
        );
    }
}
