//! The two entry forms, as plain state machines.
//!
//! No ratatui in any signature here except the render functions at the
//! bottom: the parsing, the validation, and the suggestion rules are the
//! parts with decisions in them, and they are unit-tested directly.

use super::Account;
use crate::db::account::{self, Kind};
use crate::db::txn::{NewTxn, Suggestion, Txn};
use crate::db::{AccountId, TxnId};
use crate::money::Cents;
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{Datelike, NaiveDate, TimeDelta};

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

    /// Replace the contents so that they count as the user's own -- the same
    /// as having typed them. The counterpart of [`Field::fill`], and what an
    /// arrow-stepped date is written back with.
    pub(super) fn retype(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.touched = true;
    }

    /// Whether the user has typed into this field. An empty field is not the
    /// same thing: an amount typed and then deleted is still the user's.
    pub(super) fn is_touched(&self) -> bool {
        self.touched
    }
}

/// A date field: the text, and how that text is read back as a date.
///
/// A date is the one kind of field whose reading depends on *when* it is
/// being typed, and every form used to pair a bare [`Field`] with a free
/// `parse_date` to say so. Pairing them here means the buffer and the reading
/// cannot come apart, and that a form asks for a date rather than assembling
/// one out of two halves it has to keep in step itself.
#[derive(Clone, Debug)]
pub(super) struct DateField {
    field: Field,
    /// The day the `M/D` shorthand resolves against. `None` is a field that
    /// takes `YYYY-MM-DD` and nothing else -- a birth date has no
    /// present-or-future reading, so a shorthand there could only ever land
    /// decades wrong in silence.
    shorthand_from: Option<NaiveDate>,
}

impl DateField {
    /// A field opened on `date`, untouched, so a suggestion may still move it.
    pub(super) fn on(today: NaiveDate, date: NaiveDate) -> DateField {
        DateField {
            field: Field::prefilled(iso(date)),
            shorthand_from: Some(today),
        }
    }

    /// A field opened on today, which is what almost every form's date does.
    pub(super) fn today(today: NaiveDate) -> DateField {
        DateField::on(today, today)
    }

    /// A blank field, for the two dates where blank means something in its
    /// own right: an undated goal, and a recurring transaction that does not
    /// end.
    pub(super) fn blank(today: NaiveDate) -> DateField {
        DateField {
            field: Field::prefilled(""),
            shorthand_from: Some(today),
        }
    }

    /// An existing row's date, which counts as the user's own. `None` is the
    /// blank that row already carried.
    pub(super) fn given(today: NaiveDate, date: Option<NaiveDate>) -> DateField {
        DateField {
            field: Field::given(date.map(iso).unwrap_or_default()),
            shorthand_from: Some(today),
        }
    }

    /// A field that takes `YYYY-MM-DD` and nothing else. It needs no `today`,
    /// which is the distinction made visible: the shorthand is the reading
    /// that depends on when you are.
    pub(super) fn iso_only(prefill: &str) -> DateField {
        DateField {
            field: Field::given(prefill),
            shorthand_from: None,
        }
    }

    pub(super) fn value(&self) -> &str {
        self.field.value()
    }

    /// What the field shows: the text as typed while the caret is in it, and
    /// the date that text means once the caret leaves.
    ///
    /// `YYYY-MM-DD` is the display date everywhere in the app, so a shorthand
    /// has to resolve on screen somewhere or the owner never sees what they
    /// asked for until the row is written. Computed rather than written back,
    /// so there is no blur hook for the next form to forget -- and so a
    /// half-typed date is never rewritten under the cursor by the keystroke
    /// still finishing it.
    pub(super) fn display(&self, focused: bool) -> String {
        match self.parse() {
            Ok(date) if !focused => iso(date),
            _ => self.field.value().to_string(),
        }
    }

    pub(super) fn push(&mut self, c: char) {
        self.field.push(c);
    }

    pub(super) fn backspace(&mut self) {
        self.field.backspace();
    }

    /// Step the date by `days`, rewriting it as the date it now means. What
    /// `←`/`→` do on every date field in the app, and `Shift` with them a
    /// week at a time.
    ///
    /// A field holding something that is not a date is left exactly as it
    /// was: the arrows are a nudge on a date already there, not a way to
    /// conjure one. That is what keeps them off a half-typed date, and off
    /// the blank fields that mean something in their own right -- an undated
    /// goal, and a recurring transaction that does not end.
    ///
    /// The step counts as the user's own, the same as a keystroke: a date
    /// arrived at by pressing an arrow is not a prefill for a suggestion to
    /// overwrite.
    pub(super) fn step(&mut self, days: i64) {
        let Ok(date) = self.parse() else {
            return;
        };
        let Some(stepped) = date.checked_add_signed(TimeDelta::days(days)) else {
            return;
        };
        self.field.retype(iso(stepped));
    }

    /// The date this field means, when the text does not already say it --
    /// what a single-field modal shows beside the field.
    ///
    /// `display`'s "as typed under the caret, as the date it means once focus
    /// leaves" needs somewhere for focus to go, and a modal with one field
    /// has nowhere. `None` for a date typed out in full, which would only be
    /// echoed back, and for text that is not a date at all, which has nothing
    /// to resolve to -- a guess there would be a date the dialog never writes.
    pub(super) fn resolved(&self) -> Option<String> {
        let text = iso(self.parse().ok()?);
        (text != self.field.value().trim()).then_some(text)
    }

    /// The date this field holds, or the error naming the text that would not
    /// parse.
    pub(super) fn parse(&self) -> Result<NaiveDate> {
        let raw = self.field.value().trim();
        match self.shorthand_from {
            Some(today) if raw.contains('/') => parse_shorthand(raw, today),
            _ => parse_date(raw),
        }
    }

    /// The same, where blank is a supported answer rather than a refusal --
    /// an undated goal, a rule that does not end.
    pub(super) fn parse_opt(&self) -> Result<Option<NaiveDate>> {
        if self.field.value().trim().is_empty() {
            return Ok(None);
        }
        self.parse().map(Some)
    }
}

/// How far one arrow press moves what it is pressed on: a day, or -- with
/// `Shift` -- a week.
///
/// One value rather than a direction and a magnitude, so a form's answer to
/// the arrows is one match on its focus rather than two near-identical ones
/// that have to be kept in step by hand. `Shift` is then the same nudge with
/// a bigger step, by construction, rather than by a second code path beside
/// the first.
///
/// A selector has no week to move, so it reads the direction and ignores the
/// size: a modified arrow that did nothing would be a dead key on the very
/// fields the hand reaches for it on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Step(i64);

impl Step {
    /// `→`, and `←`.
    pub const NEXT: Step = Step(1);
    pub const PREVIOUS: Step = Step(-1);
    /// `Shift` with them.
    pub const NEXT_WEEK: Step = Step(super::WEEK);
    pub const PREVIOUS_WEEK: Step = Step(-super::WEEK);

    /// The step in days, which is what a date moves by.
    pub fn days(self) -> i64 {
        self.0
    }

    /// Which way, which is all a selector takes.
    pub fn direction(self) -> isize {
        self.0.signum() as isize
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

/// `M/D` -- a month and a day, taking the next year that month occurs in.
///
/// The year turns on the **month** alone, never on the whole date: `8/1`
/// typed in August is the first of this August, a fortnight back, rather than
/// next year's. Backdating a ledger row a week or two is the commonest thing
/// the shorthand is typed for, and a rule that always resolved forward could
/// not express it at all -- while a month already behind has no reading but
/// the year ahead, which is the case the roll exists for.
fn parse_shorthand(raw: &str, today: NaiveDate) -> Result<NaiveDate> {
    let raw = raw.trim();
    let malformed = || anyhow!("not a M/D date: {raw:?}");
    let (month, day) = raw.split_once('/').ok_or_else(malformed)?;
    let month: u32 = month.trim().parse().map_err(|_| malformed())?;
    let day: u32 = day.trim().parse().map_err(|_| malformed())?;
    let year = if month >= today.month() {
        today.year()
    } else {
        today.year() + 1
    };
    NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| anyhow!("no such date: {raw:?}"))
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
///
/// What it quotes back is masked, because this is the one refusal in the crate
/// whose subject is *guaranteed* to be a real figure: it fires only on text
/// that already parsed as money, and a form's amount field is prefilled from
/// the row it opened on. [`parse_amount`]'s own error cannot leak the same way
/// -- it fires only on text no reading of which is a figure.
pub(super) fn parse_whole_amount(raw: &str) -> Result<Cents> {
    let cents = parse_amount(raw)?;
    ensure!(
        cents.0 % 100 == 0,
        "amount must be a whole number of dollars: {:?}",
        crate::demo::typed(raw.trim())
    );
    Ok(cents)
}

/// Whether an amount field is holding a `/N` fraction rather than a figure.
///
/// Beside [`parse_share`], which is what decides it: the allocation form asks
/// twice over -- once to resolve the fraction for the line under the field,
/// and once to keep the mask off a divisor -- and a second spelling of the
/// same prefix test is how those two would come to disagree.
pub(super) fn is_share(raw: &str) -> bool {
    raw.trim().starts_with('/')
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
    /// Move whatever has focus by `step`: a date by that many days, a
    /// selector by that many choices in that direction.
    ///
    /// One method rather than a next and a previous, because the field under
    /// the caret is what decides either way and two methods only ever meant
    /// two matches to keep in step.
    fn choice(&mut self, step: Step);
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
    /// It is a *default*, not the user's own choice: like the prefilled date,
    /// it stays untouched, so an accepted suggestion may still move it.
    pub fn add(
        accounts: Vec<account::Account>,
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
            date: DateField::today(today),
            amount: Field::default(),
            description: Field::default(),
            accounts,
            account,
            account_touched: false,
        })
    }

    pub fn edit(accounts: Vec<account::Account>, today: NaiveDate, txn: &Txn) -> Result<TxnForm> {
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
            TxnField::Description => Label::from(self.description.value()),
            TxnField::Account => match self.accounts.get(self.account) {
                Some(a) => Label::default().account(Account::labelled(a)),
                None => Label::default(),
            },
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
    fn next_field(&mut self) {
        self.focus = next_in(&TxnField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&TxnField::ORDER, self.focus, -1);
    }

    /// Cycle the selector or step the date, whichever is focused. A no-op on
    /// the other two: `←`/`→` on the description must not silently change the
    /// account.
    fn choice(&mut self, step: Step) {
        match self.focus {
            TxnField::Date => self.date.step(step.days()),
            TxnField::Account => {
                self.account = step_index(self.account, self.accounts.len(), step.direction());
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
    label: Label,
    entry: Entry,
}

/// The one field a [`ValueForm`] collects, and which reading it is.
///
/// An enum rather than a `Field` beside a flag: a figure and a date are two
/// readings of one buffer, and a separate flag is a second place for this
/// form to say which it is collecting -- one the two could disagree on.
#[derive(Debug)]
enum Entry {
    /// A figure, which this form does not parse: the caller knows which
    /// constant is being edited and therefore how its text reads.
    ///
    /// `given`, not `prefilled`: the text on screen is a real figure the user
    /// can see. Nothing here takes suggestions, but the distinction is the
    /// one `Field` exists to make.
    Figure(Field),
    /// A figure in dollars, which a demo blocks out.
    ///
    /// A third reading rather than a flag beside `Figure`, for the reason
    /// this is an enum at all: which reading the caller opened the form on is
    /// one fact, and a flag would be a second place to say it. The two are
    /// not interchangeable -- the Planning screen edits a pay-period count
    /// and a split percentage through this same modal, and neither of those
    /// is money.
    Money(Field),
    /// A date, which `←`/`→` step like every other date in the app.
    Date(DateField),
}

impl Entry {
    fn value(&self) -> &str {
        match self {
            Entry::Figure(field) | Entry::Money(field) => field.value(),
            Entry::Date(date) => date.value(),
        }
    }
}

impl ValueForm {
    pub fn new(label: impl Into<Label>, prefill: &str) -> ValueForm {
        ValueForm {
            label: label.into(),
            entry: Entry::Figure(Field::given(prefill)),
        }
    }

    /// The same form over an amount -- a Planning constant, a bill, a fund's
    /// value, a reconciliation target. What separates it from [`ValueForm::new`]
    /// is only that a demo blocks what it shows.
    pub fn money(label: impl Into<Label>, prefill: &str) -> ValueForm {
        ValueForm {
            label: label.into(),
            entry: Entry::Money(Field::given(prefill)),
        }
    }

    /// The same form over a date -- the Funds screen's birth-date prompt.
    /// `←`/`→` step it, as they do on every other date field.
    ///
    /// `iso_only`: every reading of the `M/D` shorthand is present or future,
    /// and a birth date is decades past, so a shorthand here could only ever
    /// be a wrong year that nothing refuses.
    pub fn date(label: impl Into<Label>, prefill: &str) -> ValueForm {
        ValueForm {
            label: label.into(),
            entry: Entry::Date(DateField::iso_only(prefill)),
        }
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    pub fn value(&self) -> &str {
        self.entry.value()
    }

    /// What the field shows, which is [`ValueForm::value`] itself unless this
    /// form is collecting money in a demo. The buffer is untouched either
    /// way: what is committed is what was typed.
    pub fn display(&self) -> String {
        match &self.entry {
            Entry::Money(field) => crate::demo::typed(field.value()),
            other => other.value().to_string(),
        }
    }

    /// The label, wherever it was built -- with an account segment, on the
    /// Reconcile modal -- carries straight through into the border: `prepend`
    /// and `text` are the whole of what a `Label` can do to itself, which is
    /// what keeps the color in place all the way to the screen.
    pub fn title(&self) -> Label {
        self.label
            .clone()
            .prepend("Edit ")
            .text(" — Enter save · Esc cancel")
    }
}

impl FormFields for ValueForm {
    // One field, so there is nowhere to tab to.
    fn next_field(&mut self) {}
    fn previous_field(&mut self) {}

    // Nothing to cycle either, unless the field is a date, which steps.
    fn choice(&mut self, step: Step) {
        if let Entry::Date(date) = &mut self.entry {
            date.step(step.days());
        }
    }

    fn type_char(&mut self, c: char) {
        match &mut self.entry {
            Entry::Figure(field) | Entry::Money(field) => field.push(c),
            Entry::Date(date) => date.push(c),
        }
    }

    fn backspace(&mut self) {
        match &mut self.entry {
            Entry::Figure(field) | Entry::Money(field) => field.backspace(),
            Entry::Date(date) => date.backspace(),
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
    pub fn transfer(accounts: Vec<account::Account>, today: NaiveDate) -> Result<TransferForm> {
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
        Ok(TransferForm {
            focus: TransferField::Date,
            kind: TransferKind::Transfer,
            date: DateField::today(today),
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
    pub fn payment(accounts: Vec<account::Account>, today: NaiveDate) -> Result<TransferForm> {
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
            date: DateField::today(today),
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

    pub fn display(&self, field: TransferField) -> Label {
        let account = |list: &[account::Account], i: usize| match list.get(i) {
            Some(a) => Label::default().account(Account::labelled(a)),
            None => Label::default(),
        };
        match field {
            TransferField::Date => {
                Label::from(self.date.display(self.focus == TransferField::Date))
            }
            TransferField::Amount => Label::from(crate::demo::typed(self.amount.value())),
            TransferField::Description => Label::from(self.description.value()),
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
    fn next_field(&mut self) {
        self.focus = next_in(&TransferField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&TransferField::ORDER, self.focus, -1);
    }

    fn choice(&mut self, step: Step) {
        match self.focus {
            TransferField::Date => self.date.step(step.days()),
            TransferField::From => {
                self.from = step_index(self.from, self.from_accounts.len(), step.direction())
            }
            TransferField::To => {
                self.to = step_index(self.to, self.to_accounts.len(), step.direction());
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
use super::{Label, label_line};
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
pub(super) fn field_line(label: &str, value: Label, focused: bool) -> TextLine<'static> {
    field_line_noted(label, value, focused, "")
}

/// The same, with a note past the caret -- what the field comes to, where its
/// text is an expression rather than the figure itself. An empty note draws
/// nothing, trailing space included.
pub(super) fn field_line_noted(
    label: &str,
    value: Label,
    focused: bool,
    note: &str,
) -> TextLine<'static> {
    let mut spans = vec![Span::raw(format!("{label:>12}  "))];
    spans.extend(label_line(&value).spans);
    spans.push(Span::raw(trailer(focused, note)));
    TextLine::from(spans)
}

/// The same, with the *value* drawn in a color -- the one field whose text is
/// a name for something the form cannot otherwise show. The Accounts screen's
/// `Color` selector cycles eight names, and a name is not a color: drawing
/// `Teal` in teal is what makes the choice answerable without saving it and
/// looking.
///
/// Only the value is tinted. The label and the caret are chrome and belong to
/// the form rather than to the field's content.
pub(super) fn field_line_tinted(
    label: &str,
    value: String,
    focused: bool,
    color: Color,
) -> TextLine<'static> {
    TextLine::from(vec![
        Span::raw(format!("{label:>12}  ")),
        Span::styled(value, Style::default().fg(color)),
        Span::raw(trailer(focused, "")),
    ])
}

/// The same as [`field_line_noted`], but the label is itself a [`Label`]
/// rather than a plain `&str` -- the Reconcile modal's field label, which
/// carries the same colored account segment its border does two lines above.
///
/// The pad is measured off the label's flattened character count, which is
/// the count `format!("{label:>12}  ")` pads to, so a colored label sits in
/// the same column an uncolored one does and only the color differs.
///
/// Measured off exactly the text that is then drawn, rather than off a
/// trimmed copy of it: a label carrying surrounding space would otherwise be
/// padded for one width and drawn at another, and the two would disagree for
/// whoever wrote that label rather than for whoever wrote this.
pub(super) fn field_line_labeled(label: &Label, value: Label, focused: bool) -> TextLine<'static> {
    let width = label.plain_text().chars().count();
    let mut spans = vec![Span::raw(" ".repeat(12usize.saturating_sub(width)))];
    spans.extend(label_line(label).spans);
    spans.push(Span::raw("  "));
    spans.extend(label_line(&value).spans);
    spans.push(Span::raw(trailer(focused, "")));
    TextLine::from(spans)
}

/// The caret and the note that follow every field's value.
fn trailer(focused: bool, note: &str) -> String {
    let caret = if focused { "▌" } else { "" };
    if note.is_empty() {
        caret.to_string()
    } else {
        format!("{caret}  {note}")
    }
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
    title: impl Into<Label>,
    lines: Vec<TextLine<'static>>,
) -> Rect {
    let area = centered(frame.area(), FORM_WIDTH, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(label_line(&title.into()))),
        area,
    );
    area
}

pub fn render_value(frame: &mut Frame, form: &ValueForm) {
    let lines = vec![field_line_labeled(
        form.label(),
        Label::from(form.display()),
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
                s.description,
                crate::demo::figure(s.cents),
                s.uses
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

    /// The three worked readings of the shorthand, from one August. A month
    /// still ahead is this year's; a month already behind is next year's.
    #[test]
    fn a_month_day_shorthand_takes_the_next_year_that_month_occurs_in() {
        let today = day(2026, 8, 21);
        let read = |raw: &str| {
            let mut f = DateField::blank(today);
            for c in raw.chars() {
                f.push(c);
            }
            f.parse().unwrap()
        };

        assert_eq!(read("9/10"), day(2026, 9, 10));
        assert_eq!(read("10/03"), day(2026, 10, 3));
        assert_eq!(read("3/4"), day(2027, 3, 4));
    }

    /// The year turns on the month alone, so a day already past in the
    /// current month reads as that day rather than rolling a year forward.
    /// Backdating a ledger row a fortnight is the commonest thing the
    /// shorthand is typed for, and a rule that could not express it would
    /// send the owner back to typing the year out.
    #[test]
    fn a_shorthand_in_the_current_month_stays_in_it_even_once_the_day_has_passed() {
        let mut f = DateField::blank(day(2026, 8, 21));
        for c in "8/1".chars() {
            f.push(c);
        }
        assert_eq!(f.parse().unwrap(), day(2026, 8, 1));
    }

    /// A day the month does not have is a typo, not a date to round.
    #[test]
    fn a_shorthand_day_its_month_does_not_have_is_refused_with_the_text_that_failed() {
        let mut f = DateField::blank(day(2026, 8, 21));
        for c in "2/30".chars() {
            f.push(c);
        }
        let err = f.parse().unwrap_err().to_string();
        assert!(err.contains("2/30"), "{err}");
    }

    /// `YYYY-MM-DD` is what a field is written back as, so it must read back
    /// the same whichever kind of field it lands in.
    #[test]
    fn an_iso_date_reads_the_same_with_the_shorthand_on_or_off() {
        let mut shorthand = DateField::blank(day(2026, 8, 21));
        let mut iso = DateField::iso_only("");
        for c in "2026-11-27".chars() {
            shorthand.push(c);
            iso.push(c);
        }
        assert_eq!(shorthand.parse().unwrap(), day(2026, 11, 27));
        assert_eq!(iso.parse().unwrap(), day(2026, 11, 27));
    }

    /// The Funds screen's birth-date prompt. Every reading of `M/D` is
    /// present-or-future and a birth date is decades past, so the shorthand
    /// there could only ever be a wrong year nothing refuses.
    #[test]
    fn a_field_that_takes_no_shorthand_refuses_one() {
        let mut f = DateField::iso_only("");
        for c in "3/4".chars() {
            f.push(c);
        }
        let err = f.parse().unwrap_err().to_string();
        assert!(err.contains("3/4"), "{err}");
    }

    /// `YYYY-MM-DD` is always the display date. What was typed stands while
    /// the caret is in the field -- rewriting under the cursor would fight
    /// the typing -- and the date it means is shown the moment focus leaves.
    #[test]
    fn a_shorthand_shows_as_typed_under_the_caret_and_as_the_date_it_means_once_focus_leaves() {
        let mut f = DateField::blank(day(2026, 8, 21));
        for c in "9/10".chars() {
            f.push(c);
        }
        assert_eq!(f.display(true), "9/10");
        assert_eq!(f.display(false), "2026-09-10");
    }

    /// Text that is not a date has no date to show, so it stands as typed
    /// wherever the caret is: a half-typed date must not vanish because the
    /// user tabbed away to check something.
    #[test]
    fn text_that_is_not_a_date_is_shown_as_typed_whether_or_not_it_has_focus() {
        let mut f = DateField::blank(day(2026, 8, 21));
        for c in "2026-1".chars() {
            f.push(c);
        }
        assert_eq!(f.display(true), "2026-1");
        assert_eq!(f.display(false), "2026-1");
        assert_eq!(DateField::blank(day(2026, 8, 21)).display(false), "");
    }

    /// The arrows nudge whatever the field means, so a shorthand steps from
    /// the date it resolves to and is written back the way every date is.
    #[test]
    fn stepping_a_shorthand_rewrites_it_as_the_iso_date_it_meant() {
        let mut f = DateField::blank(day(2026, 8, 21));
        for c in "9/10".chars() {
            f.push(c);
        }
        f.step(1);
        assert_eq!(f.value(), "2026-09-11");
    }

    /// Blank means an undated goal and a rule that does not end. Neither is
    /// a date an arrow may conjure, and neither is a parse failure.
    #[test]
    fn a_blank_date_field_is_no_date_and_no_arrow_dates_it() {
        let mut f = DateField::blank(day(2026, 8, 21));
        assert_eq!(f.parse_opt().unwrap(), None);
        f.step(1);
        f.step(-7);
        assert_eq!(f.value(), "");
        assert_eq!(f.parse_opt().unwrap(), None);
    }

    /// An existing row's date is the user's own, so it opens on that date
    /// rather than on today.
    #[test]
    fn a_given_date_opens_on_the_date_it_was_given() {
        let f = DateField::given(day(2026, 8, 21), Some(day(2026, 11, 27)));
        assert_eq!(f.value(), "2026-11-27");
        assert_eq!(
            DateField::given(day(2026, 8, 21), None)
                .parse_opt()
                .unwrap(),
            None
        );
    }

    /// `Shift` with an arrow is the same nudge on the same key, a week at a
    /// time.
    #[test]
    fn shift_steps_a_transaction_date_a_week() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        form.choice(Step::NEXT_WEEK);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-22");
        form.choice(Step::PREVIOUS_WEEK);
        form.choice(Step::PREVIOUS_WEEK);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-08");
    }

    /// A selector has no week to move, so it takes the direction and ignores
    /// the size. A modified arrow that did nothing would be a dead key on the
    /// very field the hand reaches for it on.
    #[test]
    fn a_week_step_moves_a_selector_one_choice_like_a_plain_arrow() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        while form.focus != TxnField::Account {
            form.next_field();
        }
        form.choice(Step::NEXT_WEEK);
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Transfer in");

        assert_eq!(form.commit().unwrap().account_id, AccountId(2));
    }

    /// The date and the account are both reachable by the arrows, so a week
    /// pressed on one must not reach the other.
    #[test]
    fn a_week_step_on_the_date_leaves_the_account_selector_alone() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        form.choice(Step::NEXT_WEEK);
        typed(&mut form, TxnField::Amount, "10");
        typed(&mut form, TxnField::Description, "Coffee");

        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

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

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "CHK".into(),
                name: "Everyday".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            account::Account {
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

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
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

    /// A transaction's account is the same account the ledger's Account column
    /// names behind the form, so it is the same color. The selector shows a
    /// code and a name, and both are that account.
    #[test]
    fn the_account_selector_shows_one_colored_account() {
        let form = TxnForm::add(accounts(), today(), None).unwrap();
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
        let form = TxnForm::add(accounts(), today(), None).unwrap();
        assert!(form.display(TxnField::Date).accounts().is_empty());
        assert!(form.display(TxnField::Amount).accounts().is_empty());
        assert!(form.display(TxnField::Description).accounts().is_empty());
    }

    /// Both ends of a transfer, so money moving between two containers is
    /// readable at a glance rather than by reading two codes.
    #[test]
    fn both_ends_of_a_transfer_name_their_own_account() {
        let form = TransferForm::transfer(all_accounts(), today()).unwrap();
        let from = form.display(TransferField::From);
        let to = form.display(TransferField::To);
        assert_eq!(from.accounts().len(), 1);
        assert_eq!(to.accounts().len(), 1);
    }

    #[test]
    fn add_prefills_todays_date_and_commits_what_was_typed() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
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
        form.choice(Step::NEXT);
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
        form.choice(Step::NEXT);
        form.choice(Step::NEXT);
        typed(&mut form, TxnField::Amount, "10");

        assert_eq!(form.commit().unwrap().account_id, AccountId(1));
    }

    #[test]
    fn accepting_a_suggestion_fills_an_untouched_account_and_amount() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
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
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
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
        let form = TxnForm::add(accounts(), day(2026, 8, 15), Some(AccountId(2))).unwrap();
        assert_eq!(
            form.display(TxnField::Account).plain_text(),
            "SAV — Rainy Day"
        );
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
        assert_eq!(
            form.display(TxnField::Account).plain_text(),
            "CHK — Everyday"
        );
    }

    #[test]
    fn a_form_with_no_accounts_to_write_to_is_refused() {
        let err = TxnForm::add(Vec::new(), day(2026, 8, 15), None).unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    #[test]
    fn a_transfer_prefills_the_description_both_legs_share() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "Transfer"
        );

        while form.focus != TransferField::To {
            form.next_field();
        }
        form.choice(Step::NEXT);
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
        form.choice(Step::NEXT);
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
        assert_eq!(
            form.display(TransferField::To).plain_text(),
            "CC1 — Card One"
        );

        let mut form = form;
        while form.focus != TransferField::To {
            form.next_field();
        }
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
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        while form.focus != TransferField::From {
            form.next_field();
        }
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
        let form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        let mut form = form;
        while form.focus != TransferField::To {
            form.next_field();
        }
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

    #[test]
    fn a_transfer_with_no_cash_account_to_move_from_is_refused() {
        let cards: Vec<account::Account> = all_accounts()
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
        assert_eq!(
            form.display(TransferField::From).plain_text(),
            "CHK — Everyday"
        );
        while form.focus != TransferField::From {
            form.next_field();
        }
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
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
            "CC1 Payment"
        );

        while form.focus != TransferField::To {
            form.next_field();
        }
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

        while form.focus != TransferField::To {
            form.next_field();
        }
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(TransferField::Description).plain_text(),
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
        let err = TransferForm::payment(accounts(), day(2026, 9, 8)).unwrap_err();
        assert!(err.to_string().contains("credit"), "{err}");
    }

    /// The Planning screen's `e`: the caller already knows the label and how
    /// to parse the text, so the form is one prefilled field and nothing else.
    #[test]
    fn a_value_form_opens_prefilled_and_returns_what_was_typed() {
        let mut form = ValueForm::new("Target", "13,500.00");
        assert_eq!(form.label().plain_text(), "Target");
        assert_eq!(form.value(), "13,500.00");

        for _ in 0..9 {
            form.backspace();
        }
        for c in "9000".chars() {
            form.type_char(c);
        }
        assert_eq!(form.value(), "9000");
    }

    /// The autocomplete list is a window onto rows already written: each
    /// suggestion carries the amount it would fill in, which is a real figure
    /// off a real transaction.
    #[test]
    fn a_demo_blocks_the_amounts_the_autocomplete_list_offers() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install(true);
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("Whole Foods", AccountId(1), 12_345)]);

        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| {
                let area = Rect {
                    x: 0,
                    y: 0,
                    width: 60,
                    height: 3,
                };
                render_popup(frame, area, &popup);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!text.contains("123.45"), "the amount survived: {text}");
        assert!(text.contains("██████"), "nothing was blocked: {text}");
        assert!(text.contains("Whole Foods"), "the description must stay");
    }

    /// A form opens prefilled on an edit, so the field is where the row's own
    /// amount would otherwise be published to whoever is watching. What is in
    /// the buffer is untouched -- the form still commits the real figure --
    /// and the description and date beside it are not money.
    #[test]
    fn a_demo_blocks_the_amount_a_transaction_form_shows_without_touching_it() {
        crate::demo::install(true);
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

        assert_eq!(form.display(TxnField::Amount).plain_text(), "██████");
        assert_eq!(
            form.display(TxnField::Description).plain_text(),
            "Groceries"
        );
        assert_eq!(form.commit().unwrap().cents, Cents(123_456));
    }

    #[test]
    fn a_demo_blocks_the_amount_a_transfer_form_shows() {
        crate::demo::install(true);
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        typed_transfer(&mut form, TransferField::Amount, "3,291.00");
        assert_eq!(form.display(TransferField::Amount).plain_text(), "██████");
    }

    /// The refusal quotes the amount back, and a status line is on screen as
    /// surely as a column is.
    #[test]
    fn a_demo_blocks_the_figure_a_refused_amount_quotes() {
        crate::demo::install(true);
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
        while form.focus != TransferField::To {
            form.next_field();
        }
        form.choice(Step::NEXT);
        typed_transfer(&mut form, TransferField::Amount, "-500");
        let err = form.commit().unwrap_err().to_string();
        assert!(!err.contains("500"), "the amount survived: {err}");
        assert!(err.contains("██████"), "nothing was blocked: {err}");
    }

    /// The sibling refusal, and the sharper one: this error fires only on
    /// text that *already parsed* as money, so what it quotes back is a real
    /// figure every time. `e` on a fund row prefills the stored cents, which
    /// is exactly the input that trips it.
    #[test]
    fn a_demo_blocks_the_figure_a_refused_whole_amount_quotes() {
        crate::demo::install(true);
        let err = parse_whole_amount("60,000.23").unwrap_err().to_string();
        assert!(!err.contains("60,000"), "the amount survived: {err}");
        assert!(err.contains("██████"), "nothing was blocked: {err}");
    }

    /// The same refusal outside a demo still names what was typed rather than
    /// what it parsed to: `1800.5` for `1800.50` is a typo, and the typo is
    /// what makes the message worth reading.
    #[test]
    fn an_ordinary_run_quotes_the_amount_it_refused_as_typed() {
        let err = parse_whole_amount(" 1800.5 ").unwrap_err().to_string();
        assert!(err.contains("1800.5"), "{err}");
    }

    /// A one-field form does not know what it is collecting -- its caller
    /// does -- so money and a plain figure are two constructors. The Planning
    /// screen edits a paycheck period and a split percentage through the same
    /// modal it edits a target through, and neither of those is money.
    #[test]
    fn a_demo_blocks_a_money_value_form_and_leaves_a_plain_figure_alone() {
        crate::demo::install(true);
        assert_eq!(ValueForm::money("Target", "13,500.00").display(), "██████");
        assert_eq!(ValueForm::new("Paycheck Period", "14").display(), "14");
        assert_eq!(ValueForm::money("Target", "13,500.00").value(), "13,500.00");
    }

    /// A `ValueForm` with no account in its label reads exactly as it always
    /// has: the border still says "Edit", the same word every other form's
    /// title opens on -- only the Reconcile modal's title carries a tint, and
    /// that must not cost every other one-field form its wording.
    #[test]
    fn a_value_forms_title_reads_edit_then_its_label() {
        let form = ValueForm::new("Target", "13,500.00");
        assert_eq!(
            form.title().plain_text(),
            "Edit Target — Enter save · Esc cancel"
        );
    }

    /// `prepend` is invented for exactly this: "Edit " has to land ahead of a
    /// label that already carries a colored account segment -- the Reconcile
    /// modal's -- without flattening the segment on the way. The wording is
    /// the same regression `a_value_forms_title_reads_edit_then_its_label`
    /// guards for the plain case; this is its account-bearing sibling.
    #[test]
    fn the_value_forms_title_keeps_its_account_as_a_segment_through_prepend() {
        let all = accounts();
        let label = Label::plain("Target · ").account(Account::named(&all, AccountId(1)));
        let form = ValueForm::new(label, "1,200.00");
        let title = form.title();
        assert_eq!(
            title.plain_text(),
            "Edit Target · Everyday — Enter save · Esc cancel"
        );
        assert_eq!(title.accounts().len(), 1);
        assert_eq!(title.accounts()[0].id(), AccountId(1));
    }

    /// The Reconcile modal names its account twice -- once in the border and
    /// once in the field label two lines down -- and both have to be the same
    /// color, or the modal would draw one account two ways. `render_value`
    /// used to flatten this label back to a `String` with `plain_text()`
    /// before drawing it, which is exactly how that happened.
    #[test]
    fn the_value_forms_field_label_draws_its_account_in_the_accounts_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let all = accounts();
        let label = Label::plain("Target · ").account(Account::named(&all, AccountId(1)));
        let form = ValueForm::new(label, "1,200.00");

        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal.draw(|frame| render_value(frame, &form)).unwrap();
        let buffer = terminal.backend().buffer();

        // The field label, not the border title: the title also says
        // "Everyday" but is the only row carrying "Edit".
        let (y, line) = (0..10u16)
            .map(|y| {
                (
                    y,
                    (0..80u16)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains("Everyday") && !line.contains("Edit"))
            .expect("the field label was not drawn");

        let expected = crate::tui::style::account_color(AccountId(1), None);
        let at = crate::tui::column_of(&line, "Everyday");
        assert_eq!(buffer[(at, y)].fg, expected, "{line:?}");
    }

    /// `field_line_labeled` replaced a `format!("{label:>12}  ")` with a
    /// pad span measured off `Label::plain_text()`. For a label with no
    /// account the two must draw the identical characters, whether the label
    /// is short enough to need padding or long enough to overrun it --
    /// otherwise every other `ValueForm` (Planning's constants, Actual
    /// Value, Birth Date) would have shifted along with the Reconcile fix.
    #[test]
    fn a_labeled_field_with_no_account_reads_identically_to_field_line() {
        fn joined(line: &TextLine) -> String {
            line.spans.iter().map(|s| s.content.as_ref()).collect()
        }
        for label in ["Target", "A Very Long Label Indeed"] {
            for focused in [false, true] {
                let plain = field_line(label, Label::from("26"), focused);
                let labeled = field_line_labeled(&Label::from(label), Label::from("26"), focused);
                assert_eq!(joined(&plain), joined(&labeled), "{label:?}");
            }
        }
    }

    /// One field means `Tab` has nowhere to go and `←`/`→` have nothing to
    /// cycle. Both must be no-ops rather than doing something surprising.
    #[test]
    fn a_value_forms_navigation_keys_do_nothing() {
        let mut form = ValueForm::new("Target", "26");
        form.next_field();
        form.previous_field();
        form.choice(Step::NEXT);
        form.choice(Step::PREVIOUS);
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
        form.choice(Step::NEXT);
        assert_eq!(form.value(), "1990-03-05");
        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.value(), "1990-03-03");
    }

    #[test]
    fn a_value_form_that_is_not_a_date_form_still_ignores_the_arrows() {
        let mut form = ValueForm::new("Target", "2026-08-15");
        form.choice(Step::NEXT);
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
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-16");
        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-14");
    }

    #[test]
    fn stepping_a_date_crosses_a_month_boundary() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 31), None).unwrap();
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-09-01");
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
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08");
    }

    /// The date and the account are both reachable by `←`/`→`, so each must
    /// stay off the other's field.
    #[test]
    fn stepping_the_date_leaves_the_account_selector_alone() {
        let mut form = TxnForm::add(accounts(), day(2026, 8, 15), None).unwrap();
        form.choice(Step::NEXT);
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
        form.choice(Step::NEXT);
        assert_eq!(form.display(TxnField::Date).plain_text(), "2026-08-15");
    }

    #[test]
    fn the_arrows_step_a_transfer_date_by_a_day() {
        let mut form = TransferForm::transfer(all_accounts(), day(2026, 8, 31)).unwrap();
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
        let mut form = TransferForm::payment(all_accounts(), day(2026, 9, 8)).unwrap();
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
                .map(|i| field_line("Label", Label::from(i.to_string()), false))
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
