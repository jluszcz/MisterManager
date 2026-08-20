//! The two entry forms, as plain state machines.
//!
//! No ratatui in any signature here except the render functions at the
//! bottom: the parsing, the validation, and the suggestion rules are the
//! parts with decisions in them, and they are unit-tested directly.

use crate::db::account::{Account, Kind};
use crate::db::txn::{NewTxn, Suggestion, Txn};
use crate::db::{AccountId, TxnId};
use crate::money::Cents;
use anyhow::{Context, Result, ensure};
use chrono::{NaiveDate, TimeDelta};

/// One text input: its buffer, and whether the user has typed into it.
///
/// `touched` is the whole reason this is a type rather than a `String`. An
/// accepted suggestion fills the fields the user has not touched and leaves
/// the rest alone.
#[derive(Clone, Debug, Default)]
pub(super) struct Field {
    value: String,
    touched: bool,
}

impl Field {
    /// A prefilled, untouched field — a suggestion may still overwrite it.
    pub(super) fn prefilled(value: impl Into<String>) -> Field {
        Field {
            value: value.into(),
            touched: false,
        }
    }

    /// A date field opened on `date`, in the format dates are typed in.
    ///
    /// The untouched half of the pair on purpose: every form that opens on
    /// today opens a date a suggestion may still move.
    pub(super) fn date(date: NaiveDate) -> Field {
        Field::prefilled(iso(date))
    }

    /// A prefilled field that counts as the user's own, so a suggestion
    /// leaves it alone. Editing an existing row uses this: its amount is a
    /// real figure the user can see and did not ask to change.
    pub(super) fn given(value: impl Into<String>) -> Field {
        Field {
            value: value.into(),
            touched: true,
        }
    }

    pub(super) fn value(&self) -> &str {
        &self.value
    }

    pub(super) fn push(&mut self, c: char) {
        self.value.push(c);
        self.touched = true;
    }

    pub(super) fn backspace(&mut self) {
        self.value.pop();
        self.touched = true;
    }

    /// Replace the contents without marking the field touched — how a
    /// suggestion fills a field.
    ///
    /// `pub(super)` alongside [`Field::prefilled`] and [`Field::given`]: a
    /// form living beside its own screen has the same suggestion rules to
    /// obey, and without these two it can only approximate them.
    pub(super) fn fill(&mut self, value: impl Into<String>) {
        self.value = value.into();
    }

    /// Whether the user has typed into this field. An empty field is not the
    /// same thing: an amount typed and then deleted is still the user's.
    pub(super) fn is_touched(&self) -> bool {
        self.touched
    }

    /// Step the date this field holds by `days`, rewriting it in the format
    /// it is stored in. What `←`/`→` do on every date field in the app.
    ///
    /// A field holding something that is not a date is left exactly as it
    /// was: the arrows are a nudge on a date already there, not a way to
    /// conjure one. That is what keeps them off a half-typed date, and off
    /// the empty fields that mean something in their own right -- an undated
    /// goal, a recurring transaction that does not end.
    ///
    /// The step counts as the user's own, the same as a keystroke: a date
    /// arrived at by pressing an arrow is not a prefill for a suggestion to
    /// overwrite.
    pub(super) fn step_date(&mut self, days: i64) {
        let Ok(date) = parse_date(&self.value) else {
            return;
        };
        let Some(stepped) = date.checked_add_signed(TimeDelta::days(days)) else {
            return;
        };
        self.value = iso(stepped);
        self.touched = true;
    }
}

/// Step `focus` around `order` by `step`, wrapping. Every form's tab order is
/// this, written once.
pub(super) fn next_in<T: Copy + PartialEq>(order: &[T], focus: T, step: isize) -> T {
    let len = order.len() as isize;
    let i = order.iter().position(|f| *f == focus).unwrap_or(0) as isize;
    order[((i + step).rem_euclid(len)) as usize]
}

/// Step an index into a list of `len` choices by `step`, wrapping. The
/// counterpart of [`next_in`] for a selector whose choices are values rather
/// than a fixed set of variants -- the accounts a form was handed, the
/// destinations a goal may close out into.
///
/// An empty list has no index to step to, so it stays at zero rather than
/// dividing by zero.
pub(super) fn step_index(index: usize, len: usize, step: isize) -> usize {
    if len == 0 {
        return 0;
    }
    ((index as isize + step).rem_euclid(len as isize)) as usize
}

/// A date written the way a field holds it, which is the way it is typed.
fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Dates are typed in the format they are stored in.
pub(super) fn parse_date(raw: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .with_context(|| format!("not a YYYY-MM-DD date: {:?}", raw.trim()))
}

/// The amount as `Cents::from_str` reads it: `$`, commas, and `.5` all work.
pub(super) fn parse_amount(raw: &str) -> Result<Cents> {
    Ok(raw.trim().parse::<Cents>()?)
}

/// The same amount, refused unless it lands on a whole dollar.
///
/// Goal figures are whole dollars -- a target, a recurring goal's base, and
/// the allocations booked against them -- so the cents a goal drifts by only
/// ever come from interest and rounding, and they collect in the container's
/// unallocated remainder where the Savings footer reports them.
///
/// Refused rather than floored: `1800.5` typed for `1800.50` is a typo, and
/// quietly booking $1,800 for it hides the slip in a figure that looks
/// deliberate. The forms surface the error on the status line.
pub(super) fn parse_whole_amount(raw: &str) -> Result<Cents> {
    let cents = parse_amount(raw)?;
    ensure!(
        cents.0 % 100 == 0,
        "amount must be a whole number of dollars: {:?}",
        raw.trim()
    );
    Ok(cents)
}

/// A whole amount, or a fraction of `pot` written `/N`.
///
/// `pot` is the container's unallocated remainder, and `/6` is a sixth of it:
/// the allocation form's answer to splitting a remainder across goals without
/// reaching for a calculator. Text rather than a keystroke so the divisor can
/// run past nine -- `/12` is a month's worth.
///
/// The fraction floors to a whole dollar where a *typed* `12.50` is refused,
/// and the two are not in tension: cents typed into a whole-dollar field are a
/// typo, while cents left over from a division are arithmetic. They stay in
/// the remainder, which is where [`parse_whole_amount`] says the drift
/// collects.
pub(super) fn parse_share(raw: &str, pot: Cents) -> Result<Cents> {
    let raw = raw.trim();
    let Some(divisor) = raw.strip_prefix('/') else {
        return parse_whole_amount(raw);
    };
    let n: i64 = divisor
        .trim()
        .parse()
        .with_context(|| format!("not a whole divisor: {raw:?}"))?;
    super::share_of(pot, n)
}

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
    /// The description comes before the amount because accepting a suggestion
    /// fills the amount: with the amount first, reaching the description
    /// meant tabbing *through* a field the suggestion was about to write
    /// anyway, and tabbing off the description now lands on the figure that
    /// arrived with it.
    pub const ORDER: [TxnField; 4] = [
        TxnField::Date,
        TxnField::Account,
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

/// What `App`'s shared modal key handler needs from a form.
///
/// The two autocomplete methods are defaulted off: only the transaction and
/// transfer forms have a description field, and a form without one must never
/// open the suggestion popup.
pub trait FormFields {
    fn next_field(&mut self);
    fn previous_field(&mut self);
    fn next_choice(&mut self);
    fn previous_choice(&mut self);
    fn type_char(&mut self, c: char);
    fn backspace(&mut self);

    /// The text to autocomplete against, when the description field has
    /// focus. `None` closes the popup.
    fn suggestion_prefix(&self) -> Option<&str> {
        None
    }

    fn apply_suggestion(&mut self, _hit: &Suggestion) {}
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
    date: Field,
    amount: Field,
    description: Field,
    accounts: Vec<Account>,
    account: usize,
    account_touched: bool,
}

impl TxnForm {
    /// `preselected` is the account the ledger is filtered to, if any, so `a`
    /// opens on the account being looked at rather than always on the first.
    ///
    /// It is a *default*, not the user's own choice: like the prefilled date,
    /// it stays untouched, so an accepted suggestion may still move it.
    pub fn add(
        accounts: Vec<Account>,
        today: NaiveDate,
        preselected: Option<AccountId>,
    ) -> Result<TxnForm> {
        ensure!(
            !accounts.is_empty(),
            "there is no account of this kind to add a transaction to"
        );
        let account = preselected
            .and_then(|id| accounts.iter().position(|a| a.id == id))
            .unwrap_or(0);
        Ok(TxnForm {
            editing: None,
            focus: TxnField::Date,
            date: Field::date(today),
            amount: Field::default(),
            description: Field::default(),
            accounts,
            account,
            account_touched: false,
        })
    }

    pub fn edit(accounts: Vec<Account>, txn: &Txn) -> Result<TxnForm> {
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
            focus: TxnField::Date,
            date: Field::given(txn.date.format("%Y-%m-%d").to_string()),
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

    pub fn display(&self, field: TxnField) -> String {
        match field {
            TxnField::Date => self.date.value().to_string(),
            TxnField::Amount => self.amount.value().to_string(),
            TxnField::Description => self.description.value().to_string(),
            TxnField::Account => self
                .accounts
                .get(self.account)
                // FIXME(task 4): a Label with one Account segment, so the selector is tinted.
                .map(|a| format!("{} — {}", a.code.as_str(), a.name.as_str()))
                .unwrap_or_default(),
        }
    }

    pub fn commit(&self) -> Result<NewTxn> {
        let account = self
            .accounts
            .get(self.account)
            .context("no account is selected")?;
        let description = self.description.value().trim().to_string();
        ensure!(!description.is_empty(), "description must not be empty");
        Ok(NewTxn {
            date: parse_date(self.date.value())?,
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
    fn next_field(&mut self) {
        self.focus = next_in(&TxnField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&TxnField::ORDER, self.focus, -1);
    }

    /// Cycle the selector or step the date, whichever is focused. A no-op on
    /// the other two: `←`/`→` on the description must not silently change the
    /// account.
    fn next_choice(&mut self) {
        match self.focus {
            TxnField::Date => self.date.step_date(1),
            TxnField::Account => {
                self.account = step_index(self.account, self.accounts.len(), 1);
                self.account_touched = true;
            }
            TxnField::Amount | TxnField::Description => {}
        }
    }

    fn previous_choice(&mut self) {
        match self.focus {
            TxnField::Date => self.date.step_date(-1),
            TxnField::Account => {
                self.account = step_index(self.account, self.accounts.len(), -1);
                self.account_touched = true;
            }
            TxnField::Amount | TxnField::Description => {}
        }
    }

    fn type_char(&mut self, c: char) {
        match self.focus {
            TxnField::Date => self.date.push(c),
            TxnField::Amount => self.amount.push(c),
            TxnField::Description => self.description.push(c),
            TxnField::Account => {}
        }
    }

    fn backspace(&mut self) {
        match self.focus {
            TxnField::Date => self.date.backspace(),
            TxnField::Amount => self.amount.backspace(),
            TxnField::Description => self.description.backspace(),
            TxnField::Account => {}
        }
    }

    fn suggestion_prefix(&self) -> Option<&str> {
        (self.focus == TxnField::Description).then(|| self.description.value())
    }

    /// Take a suggestion: the description always, the account and amount only
    /// if the user has not touched them.
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

/// One labelled, prefilled field. Backs the Planning screen's `e`.
///
/// It does not parse: the caller knows which constant is being edited and
/// therefore how its text reads, and `Target::write` is where that lives. This
/// form only collects the characters.
#[derive(Debug)]
pub struct ValueForm {
    label: String,
    field: Field,
    /// Whether the one field holds a date, and so whether `←`/`→` step it.
    /// The caller is the only one who knows: a figure that happens to read as
    /// a date is still a figure.
    is_date: bool,
}

impl ValueForm {
    pub fn new(label: &str, prefill: &str) -> ValueForm {
        ValueForm {
            label: label.to_string(),
            // `given`, not `prefilled`: the text on screen is a real figure
            // the user can see. Nothing here takes suggestions, but the
            // distinction is the one `Field` exists to make.
            field: Field::given(prefill),
            is_date: false,
        }
    }

    /// The same form over a date -- the Funds screen's birth-date prompt.
    /// `←`/`→` step it a day, as they do on every other date field.
    pub fn date(label: &str, prefill: &str) -> ValueForm {
        ValueForm {
            is_date: true,
            ..ValueForm::new(label, prefill)
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn value(&self) -> &str {
        self.field.value()
    }

    pub fn title(&self) -> String {
        format!("Edit {} — Enter save · Esc cancel", self.label.trim())
    }
}

impl FormFields for ValueForm {
    // One field, so there is nowhere to tab to.
    fn next_field(&mut self) {}
    fn previous_field(&mut self) {}

    // Nothing to cycle either, unless the field is a date, which steps.
    fn next_choice(&mut self) {
        if self.is_date {
            self.field.step_date(1);
        }
    }

    fn previous_choice(&mut self) {
        if self.is_date {
            self.field.step_date(-1);
        }
    }

    fn type_char(&mut self, c: char) {
        self.field.push(c);
    }

    fn backspace(&mut self) {
        self.field.backspace();
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

/// Moving money between two accounts. Backs `t` and `p`.
#[derive(Debug)]
pub struct TransferForm {
    pub focus: TransferField,
    kind: TransferKind,
    date: Field,
    amount: Field,
    description: Field,
    from_accounts: Vec<Account>,
    from: usize,
    to_accounts: Vec<Account>,
    to: usize,
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
    pub fn transfer(accounts: Vec<Account>, today: NaiveDate) -> Result<TransferForm> {
        ensure!(
            accounts.len() >= 2,
            "a transfer needs two different accounts"
        );
        let cash: Vec<Account> = accounts
            .iter()
            .filter(|a| a.kind == Kind::Cash)
            .cloned()
            .collect();
        ensure!(
            !cash.is_empty(),
            "there is no cash account to transfer from"
        );
        Ok(TransferForm {
            focus: TransferField::Date,
            kind: TransferKind::Transfer,
            date: Field::date(today),
            amount: Field::default(),
            description: Field::prefilled("Transfer"),
            from_accounts: cash,
            from: 0,
            to_accounts: accounts,
            to: 0,
        })
    }

    /// `p`: cash to a card, prefilled `<CODE> Payment`.
    ///
    /// The destination is restricted to credit accounts and the source to
    /// cash: a "payment" means cash settling a card, and a card-to-card move
    /// is a balance transfer, which is a different thing the owner should not
    /// reach by pressing `p`.
    pub fn payment(accounts: Vec<Account>, today: NaiveDate) -> Result<TransferForm> {
        let cards: Vec<Account> = accounts
            .iter()
            .filter(|a| a.kind == Kind::Credit)
            .cloned()
            .collect();
        ensure!(!cards.is_empty(), "there is no credit account to pay");
        let cash: Vec<Account> = accounts
            .into_iter()
            .filter(|a| a.kind == Kind::Cash)
            .collect();
        ensure!(!cash.is_empty(), "there is no cash account to pay from");

        let mut form = TransferForm {
            focus: TransferField::Date,
            kind: TransferKind::Payment,
            date: Field::date(today),
            amount: Field::default(),
            description: Field::default(),
            from_accounts: cash,
            from: 0,
            to_accounts: cards,
            to: 0,
        };
        form.refresh_payment_description();
        Ok(form)
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

    pub fn display(&self, field: TransferField) -> String {
        let account = |list: &[Account], i: usize| {
            list.get(i)
                // FIXME(task 4): a Label with one Account segment, so the selector is tinted.
                .map(|a| format!("{} — {}", a.code.as_str(), a.name.as_str()))
                .unwrap_or_default()
        };
        match field {
            TransferField::Date => self.date.value().to_string(),
            TransferField::Amount => self.amount.value().to_string(),
            TransferField::Description => self.description.value().to_string(),
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
            from.code.as_str()
        );
        let cents = parse_amount(self.amount.value())?;
        // `insert_transfer` applies the sign per the destination's kind, so a
        // negative magnitude writes both legs the wrong way round.
        ensure!(cents > Cents::ZERO, "amount must be positive, got {cents}");
        let description = self.description.value().trim().to_string();
        ensure!(!description.is_empty(), "description must not be empty");
        Ok(Transfer {
            from_account_id: from.id,
            to_account_id: to.id,
            date: parse_date(self.date.value())?,
            cents,
            description,
        })
    }
}

impl FormFields for TransferForm {
    fn next_field(&mut self) {
        self.focus = next_in(&TransferField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&TransferField::ORDER, self.focus, -1);
    }

    fn next_choice(&mut self) {
        match self.focus {
            TransferField::Date => self.date.step_date(1),
            TransferField::From => self.from = step_index(self.from, self.from_accounts.len(), 1),
            TransferField::To => {
                self.to = step_index(self.to, self.to_accounts.len(), 1);
                self.refresh_payment_description();
            }
            TransferField::Amount | TransferField::Description => {}
        }
    }

    fn previous_choice(&mut self) {
        match self.focus {
            TransferField::Date => self.date.step_date(-1),
            TransferField::From => self.from = step_index(self.from, self.from_accounts.len(), -1),
            TransferField::To => {
                self.to = step_index(self.to, self.to_accounts.len(), -1);
                self.refresh_payment_description();
            }
            TransferField::Amount | TransferField::Description => {}
        }
    }

    fn type_char(&mut self, c: char) {
        match self.focus {
            TransferField::Date => self.date.push(c),
            TransferField::Amount => self.amount.push(c),
            TransferField::Description => self.description.push(c),
            TransferField::From | TransferField::To => {}
        }
    }

    fn backspace(&mut self) {
        match self.focus {
            TransferField::Date => self.date.backspace(),
            TransferField::Amount => self.amount.backspace(),
            TransferField::Description => self.description.backspace(),
            TransferField::From | TransferField::To => {}
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

use super::autocomplete::Autocomplete;
use super::style::Color;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

/// How wide every form is drawn. One number, because a form that opened at a
/// different width than the one beside it would move its fields under the
/// hand that is already typing into them.
pub(super) const FORM_WIDTH: u16 = 64;

/// A centered rectangle, clamped to `area`.
pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// One labelled input line; the focused one carries a caret.
pub(super) fn field_line(label: &str, value: String, focused: bool) -> TextLine<'static> {
    field_line_noted(label, value, focused, "")
}

/// The same, with a note past the caret -- what the field comes to, where its
/// text is an expression rather than the figure itself. An empty note draws
/// nothing, trailing space included.
pub(super) fn field_line_noted(
    label: &str,
    value: String,
    focused: bool,
    note: &str,
) -> TextLine<'static> {
    field_line_parts(label, value, focused, note, None)
}

/// The same, with the *value* drawn in a color -- the one field whose text
/// is a name for something the form cannot otherwise show. The Accounts
/// screen's `Color` selector cycles eight names, and a name is not a color:
/// drawing `Teal` in teal is what makes the choice answerable without
/// saving it and looking.
///
/// Only the value is tinted. The label and the caret are chrome and belong
/// to the form rather than to the field's content.
pub(super) fn field_line_tinted(
    label: &str,
    value: String,
    focused: bool,
    color: Color,
) -> TextLine<'static> {
    field_line_parts(label, value, focused, "", Some(color))
}

fn field_line_parts(
    label: &str,
    value: String,
    focused: bool,
    note: &str,
    color: Option<Color>,
) -> TextLine<'static> {
    let caret = if focused { "▌" } else { "" };
    let note = if note.is_empty() {
        String::new()
    } else {
        format!("  {note}")
    };
    let value = match color {
        Some(color) => Span::styled(value, Style::default().fg(color)),
        None => Span::raw(value),
    };
    TextLine::from(vec![
        Span::raw(format!("{label:>12}  ")),
        value,
        Span::raw(format!("{caret}{note}")),
    ])
}

/// Draw a form: the centered box, its border and title, and one line per
/// row. Returns the area it took, for the forms that hang an
/// autocomplete popup off the bottom of it.
///
/// The height is the lines themselves plus the border's two rows, which is
/// what lets one function serve a fixed field order, a variable one
/// (`FundForm::fields`), and the forms that add a line of their own past
/// the fields.
pub(super) fn render_fields(
    frame: &mut Frame,
    title: impl Into<String>,
    lines: Vec<TextLine<'static>>,
) -> Rect {
    let area = centered(frame.area(), FORM_WIDTH, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title.into())),
        area,
    );
    area
}

pub fn render_value(frame: &mut Frame, form: &ValueForm) {
    let lines = vec![field_line(
        form.label().trim(),
        form.value().to_string(),
        true,
    )];
    render_fields(frame, form.title(), lines);
}

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_txn(frame: &mut Frame, form: &TxnForm, popup: &Autocomplete) -> usize {
    let title = if form.editing.is_some() {
        "Edit transaction — Tab field · Enter save · Esc cancel"
    } else {
        "Add transaction — Tab field · Enter save · Esc cancel"
    };
    let lines: Vec<TextLine> = TxnField::ORDER
        .iter()
        .map(|f| field_line(f.label(), form.display(*f), form.focus == *f))
        .collect();
    let area = render_fields(frame, title, lines);
    render_popup(frame, area, popup)
}

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_transfer(frame: &mut Frame, form: &TransferForm, popup: &Autocomplete) -> usize {
    let lines: Vec<TextLine> = TransferField::ORDER
        .iter()
        .map(|f| field_line(f.label(), form.display(*f), form.focus == *f))
        .collect();
    let area = render_fields(
        frame,
        "Transfer — Tab field · ←/→ account · Enter save · Esc cancel",
        lines,
    );
    render_popup(frame, area, popup)
}

/// The suggestion list, drawn under the form. Returns how many suggestion rows
/// it actually drew.
///
/// The form is centered, so on a short terminal this hangs off the bottom and
/// is clipped — at ten rows or fewer only the borders survive. The count is
/// what confines the cursor to the rows the user can see: a suggestion that
/// was not drawn must not be selectable, because applying one writes a
/// description, an amount and an account the user never read.
pub(super) fn render_popup(frame: &mut Frame, form_area: Rect, popup: &Autocomplete) -> usize {
    if !popup.is_open() {
        return 0;
    }
    let area = Rect {
        x: form_area.x,
        y: form_area.y + form_area.height,
        width: form_area.width,
        height: popup.suggestions().len() as u16 + 2,
    }
    .intersection(frame.area());
    // The border spends a row at the top and one at the bottom; whatever is
    // left is how many suggestions the paragraph inside can show.
    let drawn = popup
        .suggestions()
        .len()
        .min(usize::from(area.height.saturating_sub(2)));
    if drawn == 0 {
        // Drawing an empty box titled "Enter or Tab accepts" would advertise
        // keys that, correctly, now do nothing.
        return 0;
    }
    frame.render_widget(Clear, area);
    let lines: Vec<TextLine> = popup
        .suggestions()
        .iter()
        .take(drawn)
        .enumerate()
        .map(|(i, s)| {
            let marker = if i == popup.selected_index() {
                ">"
            } else {
                " "
            };
            TextLine::from(format!(
                "{marker} {}   {}   ×{}",
                s.description, s.cents, s.uses
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("↑/↓ · Enter or Tab accepts · Esc closes")),
        area,
    );
    drawn
}

#[cfg(test)]
mod tests {
    use super::FormFields;
    use super::*;
    use crate::db::account::Group;
    use crate::db::{AccountId, RecurringTxnId};

    #[test]
    fn stepping_an_index_wraps_at_both_ends() {
        assert_eq!(step_index(0, 3, 1), 1);
        assert_eq!(step_index(2, 3, 1), 0);
        assert_eq!(step_index(0, 3, -1), 2);
        assert_eq!(step_index(1, 3, -1), 0);
    }

    /// A form handed no choices at all still answers the arrow keys: the
    /// selector has nowhere to go, and a modulo by zero would take the whole
    /// app down with it.
    #[test]
    fn stepping_an_index_into_an_empty_list_stays_at_zero() {
        assert_eq!(step_index(0, 0, 1), 0);
        assert_eq!(step_index(0, 0, -1), 0);
    }

    #[test]
    fn a_whole_amount_takes_every_shape_that_lands_on_a_dollar() {
        assert_eq!(parse_whole_amount("140").unwrap(), Cents(14_000));
        assert_eq!(parse_whole_amount("$1,400.00").unwrap(), Cents(140_000));
        assert_eq!(parse_whole_amount(" -25 ").unwrap(), Cents(-2_500));
        assert_eq!(parse_whole_amount("0").unwrap(), Cents::ZERO);
    }

    /// Refused rather than floored, so a typo cannot pass as a deliberate
    /// figure. The message quotes what was typed.
    #[test]
    fn a_whole_amount_refuses_cents() {
        let err = parse_whole_amount("12.50").unwrap_err().to_string();
        assert!(err.contains("12.50"), "{err}");
        assert!(parse_whole_amount("1800.5").is_err());
        assert!(parse_whole_amount("-0.01").is_err());
        assert!(parse_whole_amount(".5").is_err());
    }

    #[test]
    fn a_whole_amount_still_refuses_what_is_not_an_amount() {
        assert!(parse_whole_amount("").is_err());
        assert!(parse_whole_amount("abc").is_err());
    }

    /// The pot is the container's unallocated remainder, and it carries the
    /// cents the goals have drifted by. Dividing floors them away.
    #[test]
    fn a_share_reads_a_fraction_of_the_pot() {
        let pot = Cents(260_017);
        assert_eq!(parse_share("/2", pot).unwrap(), Cents::from_dollars(1300));
        assert_eq!(parse_share("/6", pot).unwrap(), Cents::from_dollars(433));
    }

    /// The divisor is text rather than a keystroke precisely so it can run
    /// past nine.
    #[test]
    fn a_share_takes_a_divisor_of_more_than_one_digit() {
        assert_eq!(
            parse_share("/12", Cents(260_017)).unwrap(),
            Cents::from_dollars(216)
        );
    }

    #[test]
    fn a_share_ignores_the_space_around_it() {
        assert_eq!(
            parse_share(" /2 ", Cents::from_dollars(100)).unwrap(),
            Cents::from_dollars(50)
        );
    }

    /// Not a fraction, so the field means what it has always meant.
    #[test]
    fn an_amount_with_no_slash_parses_as_a_whole_amount() {
        assert_eq!(parse_share("140", Cents::ZERO).unwrap(), Cents(14_000));
        assert!(parse_share("12.50", Cents::ZERO).is_err());
    }

    #[test]
    fn a_share_refuses_a_divisor_that_is_not_a_positive_number() {
        let pot = Cents::from_dollars(100);
        assert!(parse_share("/0", pot).is_err());
        assert!(parse_share("/-3", pot).is_err());
        assert!(parse_share("/", pot).is_err());
        assert!(parse_share("/x", pot).is_err());
    }

    /// The message quotes what was typed, the way every other parse error on
    /// these forms does.
    #[test]
    fn a_refused_divisor_says_what_was_typed() {
        let err = parse_share("/x", Cents::ZERO).unwrap_err().to_string();
        assert!(err.contains("/x"), "{err}");
    }

    fn accounts() -> Vec<Account> {
        vec![
            Account {
                id: AccountId(1),
                code: "CHK".into(),
                name: "Everyday".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            Account {
                id: AccountId(2),
                code: "SAV".into(),
                name: "Rainy Day".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: None,
            },
        ]
    }

    fn all_accounts() -> Vec<Account> {
        let mut all = accounts();
        all.push(Account {
            id: AccountId(3),
            code: "CC1".into(),
            name: "Card One".into(),
            kind: Kind::Credit,
            sort: 0,
            group: Group::Credit,
            color: None,
        });
        all.push(Account {
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

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn suggestion(description: &str, account_id: AccountId, cents: i64) -> Suggestion {
        Suggestion {
            description: description.to_string(),
            account_id,
            cents: Cents(cents),
            uses: 4,
        }
    }

    fn typed(form: &mut TxnForm, field: TxnField, text: &str) {
        while form.focus != field {
            form.next_field();
        }
        for c in text.chars() {
            form.type_char(c);
        }
    }

    fn typed_transfer(form: &mut TransferForm, field: TransferField, text: &str) {
        while form.focus != field {
            form.next_field();
        }
        for c in text.chars() {
            form.type_char(c);
        }
    }

    #[test]
    fn add_prefills_todays_date_and_commits_what_was_typed() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        assert_eq!(form.display(TxnField::Date), "2026-08-15");

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
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Coffee");
        while form.focus != TxnField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.backspace();
        }
        for c in "08/15/2026".chars() {
            form.type_char(c);
        }

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("08/15/2026"), "{err}");
    }

    #[test]
    fn an_empty_description_is_refused() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Amount, "10");
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("description"), "{err}");
    }

    #[test]
    fn the_account_selector_cycles_through_this_kinds_accounts() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        while form.focus != TxnField::Account {
            form.next_field();
        }
        form.next_choice();
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Transfer in");

        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// `↑`/`↓` on the description must not spin the account selector, which
    /// is what a shared "cycle" key would do.
    #[test]
    fn cycling_does_nothing_unless_a_selector_is_focused() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Description, "Coffee");
        form.next_choice();
        form.next_choice();
        typed(&mut form, TxnField::Amount, "10");

        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

    #[test]
    fn accepting_a_suggestion_fills_an_untouched_account_and_amount() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        assert_eq!(form.display(TxnField::Description), "Movies");
        assert_eq!(form.display(TxnField::Amount), "14.99");
        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// Overwriting an amount the user just typed, because they then edited
    /// the description, is infuriating exactly once a month.
    #[test]
    fn accepting_a_suggestion_leaves_a_typed_amount_alone() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Amount, "22.50");
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        assert_eq!(form.display(TxnField::Description), "Movies");
        assert_eq!(form.display(TxnField::Amount), "22.50");
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
        let mut form = TxnForm::edit(accounts(), &row).unwrap();

        assert_eq!(form.editing, Some(TxnId(7)));
        assert_eq!(form.display(TxnField::Date), "2026-01-02");
        assert_eq!(form.display(TxnField::Amount), "4,999.99");

        form.apply_suggestion(&suggestion("Paycheck", AccountId(1), 500_000));

        assert_eq!(form.display(TxnField::Description), "Paycheck");
        assert_eq!(form.display(TxnField::Amount), "4,999.99");
        let new = form.commit().unwrap();
        assert_eq!(new.cents, Cents(499_999));
        assert_eq!(new.account_id, AccountId(2));
    }

    /// `a` on a ledger filtered to one account opens on that account.
    #[test]
    fn adding_with_a_preselected_account_opens_on_it() {
        let form = TxnForm::add(accounts(), day(2026, 8, 15), Some(AccountId(2))).unwrap();
        assert_eq!(form.display(TxnField::Account), "SAV — Rainy Day");
    }

    /// The preselection is a default, not the user's own choice, so it obeys
    /// the same untouched-field rule the prefilled date does.
    #[test]
    fn a_preselected_account_still_yields_to_an_accepted_suggestion() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), Some(AccountId(2))).unwrap();
        typed(&mut form, TxnField::Description, "Mov");

        form.apply_suggestion(&suggestion("Movies", AccountId(1), 1_499));

        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

    /// A filter that names an account this form cannot write to — a stale id,
    /// or the wrong kind — must not leave the selector pointing at nothing.
    #[test]
    fn a_preselected_account_that_is_not_on_offer_falls_back_to_the_first() {
        let form = TxnForm::add(accounts(), day(2026, 8, 15), Some(AccountId(99))).unwrap();
        assert_eq!(form.display(TxnField::Account), "CHK — Everyday");
    }

    #[test]
    fn a_form_with_no_accounts_to_write_to_is_refused() {
        let err = TxnForm::add(Vec::new(), day(2026, 8, 15), None).unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    #[test]
    fn a_transfer_prefills_the_description_both_legs_share() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        assert_eq!(form.display(TransferField::Description), "Transfer");

        while form.focus != TransferField::To {
            form.next_field();
        }
        form.next_choice();
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
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        while form.focus != TransferField::To {
            form.next_field();
        }
        form.next_choice();
        typed_transfer(&mut form, TransferField::Amount, "-100");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("amount must be positive"), "{err}");
    }

    #[test]
    fn a_transfer_to_the_account_it_came_from_is_refused() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        typed_transfer(&mut form, TransferField::Amount, "100");

        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("two different accounts"), "{err}");
    }

    /// `insert_transfer` already handles the sign — a credit destination
    /// sheds debt, so both legs come out negative — but the form must not
    /// offer a destination that would make that wrong.
    #[test]
    fn a_payment_offers_only_credit_destinations() {
        let form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        assert_eq!(form.display(TransferField::To), "CC1 — Card One");

        let mut form = form;
        while form.focus != TransferField::To {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(TransferField::To), "CC2 — Card Two");
        form.next_choice();
        assert_eq!(
            form.display(TransferField::To),
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
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        while form.focus != TransferField::From {
            form.next_field();
        }
        let mut seen = Vec::new();
        for _ in 0..all_accounts().len() {
            seen.push(form.display(TransferField::From));
            form.next_choice();
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
        let form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        let mut form = form;
        while form.focus != TransferField::To {
            form.next_field();
        }
        let mut seen = Vec::new();
        for _ in 0..all_accounts().len() {
            seen.push(form.display(TransferField::To));
            form.next_choice();
        }
        assert!(
            seen.contains(&"CC1 — Card One".to_string()),
            "a card must remain reachable as a destination: {seen:?}"
        );
    }

    #[test]
    fn a_transfer_with_no_cash_account_to_move_from_is_refused() {
        let cards: Vec<Account> = all_accounts()
            .into_iter()
            .filter(|a| a.kind == Kind::Credit)
            .collect();
        let err = TransferForm::transfer(cards, day(2026, 8, 31)).unwrap_err();
        assert!(err.to_string().contains("cash"), "{err}");
    }

    /// Paying a card from another card writes a negative on both legs,
    /// shedding debt twice and inventing money.
    #[test]
    fn a_payment_offers_only_cash_sources() {
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        assert_eq!(form.display(TransferField::From), "CHK — Everyday");
        while form.focus != TransferField::From {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(TransferField::From), "SAV — Rainy Day");
        form.next_choice();
        assert_eq!(form.display(TransferField::From), "CHK — Everyday");
    }

    #[test]
    fn a_payments_description_follows_the_card_until_it_is_edited() {
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        assert_eq!(form.display(TransferField::Description), "CC1 Payment");

        while form.focus != TransferField::To {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(TransferField::Description), "CC2 Payment");

        typed_transfer(&mut form, TransferField::Description, "!");
        assert_eq!(form.display(TransferField::Description), "CC2 Payment!");

        while form.focus != TransferField::To {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(
            form.display(TransferField::Description),
            "CC2 Payment!",
            "an edited description must stop following the card"
        );
    }

    #[test]
    fn a_payment_commits_both_legs_worth_of_detail() {
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        while form.focus != TransferField::From {
            form.next_field();
        }
        form.next_choice();
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
        let err = TransferForm::payment(accounts(), day(2026, 9, 8)).unwrap_err();
        assert!(err.to_string().contains("credit"), "{err}");
    }

    /// The Planning screen's `e`: the caller already knows the label and how
    /// to parse the text, so the form is one prefilled field and nothing else.
    #[test]
    fn a_value_form_opens_prefilled_and_returns_what_was_typed() {
        let mut form = ValueForm::new("Target", "13,500.00");
        assert_eq!(form.label(), "Target");
        assert_eq!(form.value(), "13,500.00");

        for _ in 0..9 {
            form.backspace();
        }
        for c in "9000".chars() {
            form.type_char(c);
        }
        assert_eq!(form.value(), "9000");
    }

    /// One field means `Tab` has nowhere to go and `←`/`→` have nothing to
    /// cycle. Both must be no-ops rather than doing something surprising.
    #[test]
    fn a_value_forms_navigation_keys_do_nothing() {
        let mut form = ValueForm::new("Target", "26");
        form.next_field();
        form.previous_field();
        form.next_choice();
        form.previous_choice();
        assert_eq!(form.value(), "26");
    }

    /// A form with no description field must never open the suggestion popup.
    #[test]
    fn a_value_form_offers_no_autocomplete() {
        let form = ValueForm::new("Target", "26");
        assert_eq!(form.suggestion_prefix(), None);
    }

    /// A date form's one field is a date, so `←`/`→` step it -- the same
    /// meaning they carry on every other date field in the app.
    #[test]
    fn a_date_value_form_steps_its_field_by_a_day() {
        let mut form = ValueForm::date("Birth Date", "1990-03-04");
        form.next_choice();
        assert_eq!(form.value(), "1990-03-05");
        form.previous_choice();
        form.previous_choice();
        assert_eq!(form.value(), "1990-03-03");
    }

    #[test]
    fn a_value_form_that_is_not_a_date_form_still_ignores_the_arrows() {
        let mut form = ValueForm::new("Target", "2026-08-15");
        form.next_choice();
        assert_eq!(
            form.value(),
            "2026-08-15",
            "a figure that happens to read as a date must not step"
        );
    }

    /// The description is where autocomplete lands, and accepting a
    /// suggestion fills the amount: with the amount ahead of it, reaching the
    /// description meant tabbing through a field the suggestion was about to
    /// fill in anyway.
    #[test]
    fn the_description_comes_before_the_amount_a_suggestion_fills() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        typed(&mut form, TxnField::Description, "Mov");
        form.apply_suggestion(&suggestion("Movies", AccountId(2), 1_499));

        form.next_field();
        assert_eq!(form.focus, TxnField::Amount);
    }

    #[test]
    fn the_transfer_description_comes_before_its_amount_too() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        while form.focus != TransferField::Description {
            form.next_field();
        }
        form.next_field();
        assert_eq!(form.focus, TransferField::Amount);
    }

    #[test]
    fn the_arrows_step_a_transaction_date_by_a_day() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        form.next_choice();
        assert_eq!(form.display(TxnField::Date), "2026-08-16");
        form.previous_choice();
        form.previous_choice();
        assert_eq!(form.display(TxnField::Date), "2026-08-14");
    }

    #[test]
    fn stepping_a_date_crosses_a_month_boundary() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 31), None).unwrap();
        form.next_choice();
        assert_eq!(form.display(TxnField::Date), "2026-09-01");
    }

    /// The arrows are a nudge on a date that is already there, not a way to
    /// conjure one: a half-typed date must not be rewritten under the caret.
    #[test]
    fn the_arrows_leave_a_field_that_is_not_a_date_as_typed() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        while form.focus != TxnField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.backspace();
        }
        for c in "2026-08".chars() {
            form.type_char(c);
        }
        form.next_choice();
        assert_eq!(form.display(TxnField::Date), "2026-08");
    }

    /// The date and the account are both reachable by `←`/`→`, so each must
    /// stay off the other's field.
    #[test]
    fn stepping_the_date_leaves_the_account_selector_alone() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        form.next_choice();
        typed(&mut form, TxnField::Description, "Coffee");
        typed(&mut form, TxnField::Amount, "10");
        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

    #[test]
    fn cycling_the_account_leaves_the_date_alone() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        while form.focus != TxnField::Account {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(TxnField::Date), "2026-08-15");
    }

    #[test]
    fn the_arrows_step_a_transfer_date_by_a_day() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        form.next_choice();
        assert_eq!(form.display(TransferField::Date), "2026-09-01");
        form.previous_choice();
        assert_eq!(form.display(TransferField::Date), "2026-08-31");
    }

    /// The account selectors sit on their own fields; a step on the date must
    /// not reach them, and the description must not follow a card that has
    /// not moved.
    #[test]
    fn stepping_a_transfer_date_moves_neither_account() {
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        form.next_choice();
        assert_eq!(form.display(TransferField::From), "CHK — Everyday");
        assert_eq!(form.display(TransferField::To), "CC1 — Card One");
    }

    /// Eleven forms had this arithmetic written out, each with its own `+ 2`,
    /// `+ 3` or `+ 4`. Deriving the height from the lines is what makes those
    /// the same number: a form that adds a line past its fields gets a box a
    /// row taller without saying so.
    #[test]
    fn a_form_is_as_tall_as_its_lines_plus_its_border() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for count in [1usize, 3, 6] {
            let lines: Vec<TextLine> = (0..count)
                .map(|i| field_line("Label", i.to_string(), false))
                .collect();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut drawn = Rect::default();
            terminal
                .draw(|frame| drawn = render_fields(frame, "Title", lines.clone()))
                .unwrap();

            assert_eq!(drawn.height, count as u16 + 2, "{count} lines");
            assert_eq!(drawn.width, FORM_WIDTH);
            // Centered: the margins either side are equal.
            assert_eq!(drawn.x, (80 - FORM_WIDTH) / 2);

            let rendered = terminal.backend().to_string();
            assert!(rendered.contains("Title"), "the border lost its title");
            assert!(
                rendered.contains(&(count - 1).to_string()),
                "the last line was not drawn"
            );
        }
    }
}
