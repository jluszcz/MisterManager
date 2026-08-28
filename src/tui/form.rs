//! The two entry forms, as plain state machines.
//!
//! No ratatui in any signature here except the render functions at the
//! bottom: the parsing, the validation, and the suggestion rules are the
//! parts with decisions in them, and they are unit-tested directly.

use super::Account;
use super::text::{self, Edit, TextBuffer};
use crate::db::account::{self, Kind};
use crate::db::txn::{NewTxn, Suggestion, Txn};
use crate::db::{AccountId, TxnId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{Datelike, Months, NaiveDate, TimeDelta};
use ratatui::crossterm::event::KeyEvent;

/// One text input: its buffer, and whether the user has typed into it.
///
/// `touched` is the whole reason this is a type rather than a `String`. An
/// accepted suggestion fills the fields the user has not touched and leaves
/// the rest alone.
#[derive(Clone, Debug, Default)]
pub(super) struct Field {
    text: TextBuffer,
    touched: bool,
}

impl Field {
    /// A prefilled, untouched field — a suggestion may still overwrite it.
    pub(super) fn prefilled(value: impl Into<String>) -> Field {
        Field {
            text: TextBuffer::from(value),
            touched: false,
        }
    }

    /// A prefilled field that counts as the user's own, so a suggestion
    /// leaves it alone. Editing an existing row uses this: its amount is a
    /// real figure the user can see and did not ask to change.
    pub(super) fn given(value: impl Into<String>) -> Field {
        Field {
            text: TextBuffer::from(value),
            touched: true,
        }
    }

    pub(super) fn value(&self) -> &str {
        self.text.value()
    }

    /// Answer an editing key, and report what it did.
    ///
    /// **A change counts as typing and a motion does not**: an amount cleared
    /// with `Ctrl`+`U` is the user's own empty field, while a caret moved
    /// across a prefill leaves it a prefill a suggestion may still overwrite.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Edit {
        let edit = text::edit_key(&mut self.text, key);
        if edit == Edit::Changed {
            self.touched = true;
        }
        edit
    }

    /// Move the caret one character. What `←`/`→` mean in a text field, where
    /// a date field reads them as a day and a selector as a choice.
    pub(super) fn step_caret(&mut self, step: Step) {
        self.text.step(step.direction());
    }

    pub(super) fn push(&mut self, c: char) {
        self.text.insert(c);
        self.touched = true;
    }

    pub(super) fn backspace(&mut self) {
        self.text.backspace();
        self.touched = true;
    }

    /// Replace the contents without marking the field touched — how a
    /// suggestion fills a field.
    ///
    /// `pub(super)` alongside [`Field::prefilled`] and [`Field::given`]: a
    /// form living beside its own screen has the same suggestion rules to
    /// obey, and without these two it can only approximate them.
    pub(super) fn fill(&mut self, value: impl Into<String>) {
        self.text.set(value);
    }

    /// Replace the contents so that they count as the user's own -- the same
    /// as having typed them. The counterpart of [`Field::fill`], and what an
    /// arrow-stepped date is written back with.
    pub(super) fn retype(&mut self, value: impl Into<String>) {
        self.text.set(value);
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

    /// Answer an editing key over the text, exactly as a plain field does.
    /// What the arrows mean is where the two part company: a date steps.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Edit {
        self.field.edit(key)
    }

    /// The buffer under the date, for the draw that has to place its caret.
    pub(super) fn text(&self) -> &Field {
        &self.field
    }

    /// Step the date by `step`, rewriting it as the date it now means. What
    /// `←`/`→` do on every date field in the app, `Shift` with them a week at
    /// a time, and `[`/`]` a month.
    ///
    /// A field holding something that is not a date is left exactly as it
    /// was: the keys are a nudge on a date already there, not a way to
    /// conjure one. That is what keeps them off a half-typed date, and off
    /// the blank fields that mean something in their own right -- an undated
    /// goal, and a recurring transaction that does not end.
    ///
    /// The step counts as the user's own, the same as a keystroke: a date
    /// arrived at by pressing a key is not a prefill for a suggestion to
    /// overwrite.
    pub(super) fn step(&mut self, step: Step) {
        let Ok(date) = self.parse() else {
            return;
        };
        let Some(stepped) = step.apply(date) else {
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

/// How far one keypress moves what it is pressed on: a day, a week with
/// `Shift`, or a month on `[`/`]`.
///
/// One value rather than a direction and a magnitude, so a form's answer to
/// the keys is one match on its focus rather than several near-identical ones
/// that have to be kept in step by hand. A bigger step is then the same nudge
/// carrying a bigger number, by construction, rather than a second code path
/// beside the first.
///
/// A selector has no week and no month to move, so it reads the direction and
/// ignores the size: a modified arrow that did nothing would be a dead key on
/// the very fields the hand reaches for it on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Step {
    amount: i64,
    unit: Unit,
}

/// What a [`Step`]'s amount counts.
///
/// A month is not a number of days -- August steps to September over 31 of
/// them and February over 28 -- so the unit travels with the amount instead
/// of being flattened into days at the constant, where it would have to guess
/// which month it was about to land in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Unit {
    Days,
    Months,
}

impl Step {
    /// `→`, and `←`.
    pub const NEXT: Step = Step::days(1);
    pub const PREVIOUS: Step = Step::days(-1);
    /// `Shift` with them.
    pub const NEXT_WEEK: Step = Step::days(super::WEEK);
    pub const PREVIOUS_WEEK: Step = Step::days(-super::WEEK);
    /// `]`, and `[`.
    pub const NEXT_MONTH: Step = Step::months(1);
    pub const PREVIOUS_MONTH: Step = Step::months(-1);

    const fn days(amount: i64) -> Step {
        Step {
            amount,
            unit: Unit::Days,
        }
    }

    const fn months(amount: i64) -> Step {
        Step {
            amount,
            unit: Unit::Months,
        }
    }

    /// The date `from` steps to, or `None` where the calendar runs out.
    ///
    /// A month step clamps the day into the month it lands in, which is what
    /// `chrono` does and the only answer there is: the 31st of a month
    /// stepping onto a thirty-day one has nowhere else to go, and stepping
    /// back from there does not return to the 31st. Stepping a *date* is the
    /// one reading of a month here -- what a screen's `[`/`]` month filter
    /// steps is a filter, and lives in [`super::month`].
    pub fn apply(self, from: NaiveDate) -> Option<NaiveDate> {
        match self.unit {
            Unit::Days => from.checked_add_signed(TimeDelta::days(self.amount)),
            Unit::Months => {
                let months = Months::new(u32::try_from(self.amount.unsigned_abs()).ok()?);
                match self.amount {
                    ..0 => from.checked_sub_months(months),
                    _ => from.checked_add_months(months),
                }
            }
        }
    }

    /// Which way, which is all a selector takes.
    pub fn direction(self) -> isize {
        self.amount.signum() as isize
    }
}

/// Where a focused field draws its caret.
///
/// [`Caret::In`] is the answer for a field with a buffer behind it and
/// [`Caret::End`] for a selector, which has none: its text is the choice
/// itself, and the caret goes past it exactly as it always has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Caret {
    End,
    /// `at` characters into `text`, which is what the buffer held when the
    /// draw asked.
    In {
        at: usize,
        text: String,
    },
}

impl Caret {
    pub(super) fn in_field(field: &Field) -> Caret {
        Caret::in_buffer(&field.text)
    }

    /// The same, for a box that is a buffer and nothing else -- a
    /// [`SearchBox`], which has no "touched" to carry.
    ///
    /// [`SearchBox`]: super::search::SearchBox
    pub(super) fn in_buffer(text: &TextBuffer) -> Caret {
        Caret::In {
            at: text.caret(),
            text: text.value().to_string(),
        }
    }

    /// Where to draw the caret in the line `drawn`.
    ///
    /// The offset is honoured only when the text on screen **is** the buffer.
    /// A figure `--demo` has scrambled draws at the same width as what was
    /// typed, so a caret sitting inside it would count the digits back out
    /// even though the count would land in bounds; a name it has replaced
    /// with a pseudonym is the same case, since a pseudonym is as long as the
    /// name. A selector is not a buffer at all. All three draw at the end,
    /// which is where every caret in the app was drawn before there was one
    /// to place.
    fn offset(&self, drawn: &str) -> usize {
        match self {
            Caret::In { at, text } if text == drawn => *at,
            _ => drawn.chars().count(),
        }
    }
}

/// The field a form's caret is in, as the key handler has to see it.
///
/// The three arms are the three readings of `←`/`→`, which is why the
/// distinction is drawn here rather than left to each form: a text field
/// moves the caret, a date steps a day, and a selector cycles.
pub(super) enum Focused<'a> {
    Text(&'a mut Field),
    Date(&'a mut DateField),
    /// A choice, which no keystroke types into.
    Selector,
}

/// The key a typed character arrives on, and the key that rubs one out.
///
/// For the tests that drive a form directly rather than through
/// `App::on_key`: a form has no `type_char` of its own any more, since every
/// key a field answers -- the character, the `Ctrl` editing keys, `Backspace`
/// -- goes through one dispatcher, and a test that stepped around it would be
/// exercising a route the keyboard does not have.
#[cfg(test)]
pub(super) fn char_key(c: char) -> KeyEvent {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[cfg(test)]
pub(super) fn backspace_key() -> KeyEvent {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
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
/// What it quotes back is scrambled, because this is the one refusal in the
/// crate whose subject is *guaranteed* to be a real figure: it fires only on
/// text that already parsed as money, and a form's amount field is prefilled
/// from the row it opened on. [`parse_amount`]'s own error cannot leak the
/// same way -- it fires only on text no reading of which is a figure.
pub(super) fn parse_whole_amount(raw: &str) -> Result<Cents> {
    let cents = parse_amount(raw)?;
    ensure!(
        cents.0 % 100 == 0,
        "amount must be a whole number of dollars: {:?}",
        crate::demo::typed(raw.trim())
    );
    Ok(cents)
}

/// What a base comes to once the tax lambda has had it -- the note the goal
/// form's Target and the recurring-goal form's Base both draw past the caret,
/// in the same words, so the two forms answer the same question the same way.
///
/// Empty whenever there is nothing to say, rather than a guess at one of the
/// three: the flag is off, `typed` is not a whole figure yet, or no rate is
/// on record. Drawn through `demo::whole_figure`, so `--demo` scrambles it
/// like every other absolute figure on a form.
pub(super) fn tax_note(taxed: bool, typed: &str, rate: Option<BasisPoints>) -> String {
    if !taxed {
        return String::new();
    }
    let Some(cents) = parse_whole_amount(typed)
        .ok()
        .and_then(|base| crate::calc::tax(base, rate?).ok())
    else {
        return String::new();
    };
    format!("({} w/ tax)", crate::demo::whole_figure(cents))
}

/// Whether an amount field is holding a `/N` fraction rather than a figure.
///
/// Beside [`parse_share`], which is what decides it: the allocation form asks
/// twice over -- once to resolve the fraction for the line under the field,
/// and once to keep the scramble off a divisor -- and a second spelling of
/// the same prefix test is how those two would come to disagree.
pub(super) fn is_share(raw: &str) -> bool {
    raw.trim().starts_with('/')
}

/// Whether a typed figure has to land on a whole dollar.
///
/// One parameter rather than a strict function and a tolerant twin, the way
/// [`crate::reading::Reading`] is one parameter over the goal readers: the two
/// readings differ in nothing but the thing they name.
///
/// [`Precision::WholeDollars`] is what `a` on Savings writes, for the reason
/// [`parse_whole_amount`] gives. [`Precision::Cents`] is what a *correction*
/// reads by: an allocation the import or an interest posting wrote already
/// holds cents, and a history that refused to save the row it just prefilled
/// would refuse exactly the rows most likely to be wrong.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Precision {
    WholeDollars,
    Cents,
}

/// A whole amount, or a fraction of `pot` written `/N`.
///
/// `pot` is the container's unallocated remainder, and `/6` is a sixth of it:
/// the allocation form's answer to splitting a remainder across goals without
/// reaching for a calculator. Text rather than a keystroke so the divisor can
/// run past nine -- `/12` is a month's worth.
///
/// The fraction floors to a whole dollar whatever the `precision`, and under
/// [`Precision::WholeDollars`] a *typed* `12.50` is refused: cents typed into
/// a whole-dollar field are a typo, while cents left over from a division are
/// arithmetic. They stay in the remainder, which is where
/// [`parse_whole_amount`] says the drift collects.
pub(super) fn parse_share(raw: &str, pot: Cents, precision: Precision) -> Result<Cents> {
    let raw = raw.trim();
    let Some(divisor) = raw.strip_prefix('/') else {
        return match precision {
            Precision::WholeDollars => parse_whole_amount(raw),
            Precision::Cents => parse_amount(raw),
        };
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

/// What `App`'s shared modal key handler needs from a form.
///
/// The two autocomplete methods are defaulted off: only the transaction and
/// transfer forms have a description field, and a form without one must never
/// open the suggestion popup.
pub(super) trait FormFields {
    /// Step the focus `step` places along this form's tab order, wrapping —
    /// which for every form but the one-field [`ValueForm`] is [`next_in`]
    /// over its own `ORDER`. One method rather than a `Tab` half and a
    /// `BackTab` half, because a form that wrote the two separately would be
    /// writing one order twice.
    fn move_focus(&mut self, step: isize);

    fn next_field(&mut self) {
        self.move_focus(1);
    }

    fn previous_field(&mut self) {
        self.move_focus(-1);
    }

    /// The field under the caret. One method, because what a keystroke means
    /// depends on which of the three kinds has the focus and a form that
    /// answered that question twice would answer it differently twice.
    fn focused(&mut self) -> Focused<'_>;

    /// The same field, for the draw, which has no `&mut` to ask with.
    fn caret(&self) -> Caret;

    /// Cycle the focused selector by `step`. Only reached when [`Focused::Selector`]
    /// has the caret, so a form with no selector in it leaves this alone --
    /// and a date's step is not here at all, since one rule for every date
    /// field in the app is the point.
    fn cycle(&mut self, _step: Step) {}

    /// Answer an editing key, and say what it did. A selector types nothing.
    fn edit(&mut self, key: KeyEvent) -> Edit {
        match self.focused() {
            Focused::Text(field) => field.edit(key),
            Focused::Date(date) => date.edit(key),
            Focused::Selector => Edit::Ignored,
        }
    }

    /// `←`/`→`, and `Shift` with them: the field under the caret decides.
    fn choice(&mut self, step: Step) {
        match self.focused() {
            Focused::Text(field) => {
                field.step_caret(step);
                return;
            }
            Focused::Date(date) => {
                date.step(step);
                return;
            }
            Focused::Selector => {}
        }
        self.cycle(step);
    }

    /// `[`/`]`: step the focused date a month, and say whether there was one.
    ///
    /// The difference from [`FormFields::choice`], which every kind of field
    /// answers, is that a bracket stays an ordinary character everywhere else
    /// -- a description may hold one. So a form with no date under the caret
    /// reports that rather than swallowing the key, and the handler above
    /// lets it through to the text.
    fn step_month(&mut self, step: Step) -> bool {
        match self.focused() {
            Focused::Date(date) => {
                date.step(step);
                true
            }
            Focused::Text(_) | Focused::Selector => false,
        }
    }

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

    fn caret(&self) -> Caret {
        match self.focus {
            TxnField::Date => Caret::in_field(self.date.text()),
            TxnField::Amount => Caret::in_field(&self.amount),
            TxnField::Description => Caret::in_field(&self.description),
            TxnField::Account => Caret::End,
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
    /// A figure in dollars, whose digits a demo scrambles.
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
    /// is only that a demo scrambles the digits of what it shows.
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
    fn move_focus(&mut self, _step: isize) {}

    // Nothing to cycle either: the one field is a buffer, and a date among
    // them steps on the arrows like every other date in the app.
    fn focused(&mut self) -> Focused<'_> {
        match &mut self.entry {
            Entry::Figure(field) | Entry::Money(field) => Focused::Text(field),
            Entry::Date(date) => Focused::Date(date),
        }
    }

    fn caret(&self) -> Caret {
        match &self.entry {
            Entry::Figure(field) | Entry::Money(field) => Caret::in_field(field),
            Entry::Date(date) => Caret::in_field(date.text()),
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
            Some(a) => Label::default().account(Account::labelled(a)),
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

    fn caret(&self) -> Caret {
        match self.focus {
            TransferField::Date => Caret::in_field(self.date.text()),
            TransferField::Amount => Caret::in_field(&self.amount),
            TransferField::Description => Caret::in_field(&self.description),
            TransferField::From | TransferField::To => Caret::End,
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

use super::autocomplete::Autocomplete;
use super::style::Color;
use super::{Label, label_line};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
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
pub(super) fn field_line(label: &str, value: Label, caret: Option<Caret>) -> TextLine<'static> {
    field_line_noted(label, value, caret, "")
}

/// The same, with a note past the value -- what the field comes to, where its
/// text is an expression rather than the figure itself. An empty note draws
/// nothing, trailing space included.
pub(super) fn field_line_noted(
    label: &str,
    value: Label,
    caret: Option<Caret>,
    note: &str,
) -> TextLine<'static> {
    let mut spans = vec![Span::raw(format!("{label:>12}  "))];
    spans.extend(value_spans(&value, caret));
    spans.push(Span::raw(trailer(note)));
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
    let mut spans = vec![
        Span::raw(format!("{label:>12}  ")),
        Span::styled(value, Style::default().fg(color)),
    ];
    if focused {
        spans.push(Span::styled(PAST_THE_END, caret_style()));
    }
    spans.push(Span::raw(trailer("")));
    TextLine::from(spans)
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
pub(super) fn field_line_labeled(
    label: &Label,
    value: Label,
    caret: Option<Caret>,
) -> TextLine<'static> {
    let width = label.plain_text().chars().count();
    let mut spans = vec![Span::raw(" ".repeat(12usize.saturating_sub(width)))];
    spans.extend(label_line(label).spans);
    spans.push(Span::raw("  "));
    spans.extend(value_spans(&value, caret));
    spans.push(Span::raw(trailer("")));
    TextLine::from(spans)
}

/// How the caret is drawn: reverse video over the character it is on, which
/// is the block a terminal's own cursor paints.
///
/// A block *over* a character rather than a bar *between* two of them. A bar
/// costs a column, so every value shifted right of the caret as the caret
/// moved through it, and a field read as though it had a space typed into it.
pub(super) fn caret_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// The character the caret sits on at the end of a line. There is nothing
/// typed there to block out, and this is the one place the caret costs a
/// column -- as a terminal's own cursor does, sitting past the last
/// character.
const PAST_THE_END: &str = " ";

/// The value, with the caret drawn onto it at its own offset.
///
/// The value arrives as a [`Label`] because an account is colored wherever it
/// is drawn, so the caret has to be laid over one character of one span
/// without flattening the rest -- and reverse video keeps that span's color,
/// swapping it into the background. `None` is a field that does not have the
/// caret and draws none at all.
pub(super) fn value_spans(value: &Label, caret: Option<Caret>) -> Vec<Span<'static>> {
    let spans = label_line(value).spans;
    let Some(caret) = caret else {
        return spans;
    };
    let at = caret.offset(&value.plain_text());

    let mut out = Vec::with_capacity(spans.len() + 3);
    let mut seen = 0;
    let mut placed = false;
    for span in spans {
        let len = span.content.chars().count();
        if !placed && at < seen + len {
            let (before, on, after) = split_around(&span.content, at - seen);
            if !before.is_empty() {
                out.push(Span::styled(before.to_string(), span.style));
            }
            out.push(Span::styled(
                on.to_string(),
                span.style.patch(caret_style()),
            ));
            if !after.is_empty() {
                out.push(Span::styled(after.to_string(), span.style));
            }
            placed = true;
        } else {
            out.push(span);
        }
        seen += len;
    }
    if !placed {
        out.push(Span::styled(PAST_THE_END, caret_style()));
    }
    out
}

/// `text` split into what precedes the character `at`, that character, and
/// what follows it. Counted in characters, so a multi-byte one is not sliced
/// through the middle.
fn split_around(text: &str, at: usize) -> (&str, &str, &str) {
    let byte = |n: usize| {
        text.char_indices()
            .nth(n)
            .map_or(text.len(), |(index, _)| index)
    };
    let (from, to) = (byte(at), byte(at + 1));
    (&text[..from], &text[from..to], &text[to..])
}

/// The note that follows a field's value, where it has one.
fn trailer(note: &str) -> String {
    if note.is_empty() {
        String::new()
    } else {
        format!("  {note}")
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
        Some(form.caret()),
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
        .map(|f| {
            field_line(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
            )
        })
        .collect();
    let area = render_fields(frame, title, lines);
    render_popup(frame, area, popup)
}

/// Returns how many suggestion rows the popup drew, for
/// `Autocomplete::set_visible`.
pub fn render_transfer(frame: &mut Frame, form: &TransferForm, popup: &Autocomplete) -> usize {
    let lines: Vec<TextLine> = TransferField::ORDER
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
                crate::demo::text(&s.description),
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
    use crate::test_support::{cash, day, walk_until};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::style::Modifier;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Where a field's caret sits, asked for the way a draw asks.
    fn caret_at(field: &Field) -> usize {
        match Caret::in_field(field) {
            Caret::In { at, .. } => at,
            Caret::End => panic!("a field's caret is always in its own text"),
        }
    }

    /// Deleting is typing as far as a suggestion is concerned: an amount
    /// cleared with `Ctrl`+`U` is the user's own empty field, not a prefill
    /// still waiting to be overwritten.
    #[test]
    fn an_editing_key_counts_as_touching_a_field() {
        let mut field = Field::prefilled("40.00");
        assert!(!field.is_touched());
        field.edit(ctrl('u'));
        assert_eq!(field.value(), "");
        assert!(field.is_touched());
    }

    /// Moving the caret is not typing, so a prefill a suggestion may still
    /// overwrite survives being looked at.
    #[test]
    fn moving_the_caret_does_not_count_as_touching_a_field() {
        let mut field = Field::prefilled("40.00");
        field.edit(ctrl('a'));
        assert_eq!(caret_at(&field), 0);
        assert!(!field.is_touched());
    }

    /// The caret is where the next keystroke lands, so a field a suggestion
    /// has just refilled must not leave it out in the old value's length.
    #[test]
    fn filling_a_field_puts_the_caret_at_the_end_of_what_filled_it() {
        let mut field = Field::given("weekly grocery run");
        field.edit(ctrl('a'));
        field.fill("rent");
        assert_eq!(caret_at(&field), 4);

        field.edit(ctrl('a'));
        field.retype("water");
        assert_eq!(caret_at(&field), 5);
    }

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
        f.step(Step::NEXT);
        assert_eq!(f.value(), "2026-09-11");
    }

    /// Blank means an undated goal and a rule that does not end. Neither is
    /// a date a step key may conjure, and neither is a parse failure.
    #[test]
    fn a_blank_date_field_is_no_date_and_no_step_key_dates_it() {
        let mut f = DateField::blank(day(2026, 8, 21));
        assert_eq!(f.parse_opt().unwrap(), None);
        f.step(Step::NEXT);
        f.step(Step::PREVIOUS_WEEK);
        f.step(Step::NEXT_MONTH);
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

    /// February is where that clamp bites hardest, and where it depends on
    /// the year: two days lost stepping out of a 30th in 2026, one in the
    /// leap year. The day does not spring back on the way out the far side,
    /// which is what makes a month step something to check rather than a
    /// nudge to hold down.
    #[test]
    fn a_month_step_into_february_lands_on_its_last_day_and_keeps_that_day_after() {
        let mut f = DateField::on(day(2026, 1, 30), day(2026, 1, 30));
        f.step(Step::NEXT_MONTH);
        assert_eq!(f.value(), "2026-02-28");
        f.step(Step::NEXT_MONTH);
        assert_eq!(f.value(), "2026-03-28");
        f.step(Step::PREVIOUS_MONTH);
        f.step(Step::PREVIOUS_MONTH);
        assert_eq!(f.value(), "2026-01-28");

        let mut leap = DateField::on(day(2028, 1, 30), day(2028, 1, 30));
        leap.step(Step::NEXT_MONTH);
        assert_eq!(leap.value(), "2028-02-29");
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
    /// cents the goals have drifted by. Dividing floors them away, whichever
    /// precision a typed figure is read at.
    #[test]
    fn a_share_reads_a_fraction_of_the_pot() {
        let pot = Cents(260_017);
        for precision in [Precision::WholeDollars, Precision::Cents] {
            assert_eq!(
                parse_share("/2", pot, precision).unwrap(),
                Cents::from_dollars(1300)
            );
            assert_eq!(
                parse_share("/6", pot, precision).unwrap(),
                Cents::from_dollars(433)
            );
        }
    }

    /// The divisor is text rather than a keystroke precisely so it can run
    /// past nine.
    #[test]
    fn a_share_takes_a_divisor_of_more_than_one_digit() {
        assert_eq!(
            parse_share("/12", Cents(260_017), Precision::WholeDollars).unwrap(),
            Cents::from_dollars(216)
        );
    }

    #[test]
    fn a_share_ignores_the_space_around_it() {
        assert_eq!(
            parse_share(" /2 ", Cents::from_dollars(100), Precision::WholeDollars).unwrap(),
            Cents::from_dollars(50)
        );
    }

    /// Not a fraction, so the field means what it has always meant.
    #[test]
    fn an_amount_with_no_slash_parses_as_a_whole_amount() {
        let pot = Cents::ZERO;
        assert_eq!(
            parse_share("140", pot, Precision::WholeDollars).unwrap(),
            Cents(14_000)
        );
        assert!(parse_share("12.50", pot, Precision::WholeDollars).is_err());
    }

    /// The whole of what the second precision changes: a correction may save
    /// the cents the row it opened on already held.
    #[test]
    fn a_typed_amount_read_at_cents_precision_keeps_them() {
        let pot = Cents::ZERO;
        assert_eq!(
            parse_share("12.50", pot, Precision::Cents).unwrap(),
            Cents(1_250)
        );
        assert_eq!(
            parse_share("140", pot, Precision::Cents).unwrap(),
            Cents(14_000)
        );
        assert!(
            parse_share("abc", pot, Precision::Cents).is_err(),
            "text that is no reading of a figure is still refused"
        );
    }

    #[test]
    fn a_share_refuses_a_divisor_that_is_not_a_positive_number() {
        let pot = Cents::from_dollars(100);
        for raw in ["/0", "/-3", "/", "/x"] {
            assert!(
                parse_share(raw, pot, Precision::WholeDollars).is_err(),
                "{raw}"
            );
        }
    }

    /// The message quotes what was typed, the way every other parse error on
    /// these forms does.
    #[test]
    fn a_refused_divisor_says_what_was_typed() {
        let err = parse_share("/x", Cents::ZERO, Precision::WholeDollars)
            .unwrap_err()
            .to_string();
        assert!(err.contains("/x"), "{err}");
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

    /// The Planning screen's `e`: the caller already knows the label and how
    /// to parse the text, so the form is one prefilled field and nothing else.
    #[test]
    fn a_value_form_opens_prefilled_and_returns_what_was_typed() {
        let mut form = ValueForm::new("Target", "13,500.00");
        assert_eq!(form.label().plain_text(), "Target");
        assert_eq!(form.value(), "13,500.00");

        for _ in 0..9 {
            form.edit(backspace_key());
        }
        for c in "9000".chars() {
            form.edit(char_key(c));
        }
        assert_eq!(form.value(), "9000");
    }

    /// The autocomplete list is a window onto rows already written: each
    /// suggestion carries the amount it would fill in and the description off
    /// the same real transaction, and a demo hides both -- the buffer a `Tab`
    /// accepts one into is untouched either way.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amounts_and_descriptions_the_autocomplete_list_offers() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
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
        assert!(
            text.contains(&crate::demo::figure(Cents(12_345))),
            "no scrambled amount found: {text}"
        );
        assert!(
            !text.contains("Whole Foods"),
            "the description survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Whole Foods").to_string()),
            "no scrambled description found: {text}"
        );
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

    /// The sibling refusal, and the sharper one: this error fires only on
    /// text that *already parsed* as money, so what it quotes back is a real
    /// figure every time. `e` on a fund row prefills the stored cents, which
    /// is exactly the input that trips it.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_figure_a_refused_whole_amount_quotes() {
        crate::demo::install_with_salt(7);
        let err = parse_whole_amount("60,000.23").unwrap_err().to_string();
        assert!(!err.contains("60,000"), "the amount survived: {err}");
        assert!(
            err.contains(&crate::demo::typed("60,000.23")),
            "no scrambled amount found: {err}"
        );
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
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_money_value_form_and_leaves_a_plain_figure_alone() {
        crate::demo::install_with_salt(7);
        let drawn = ValueForm::money("Target", "13,500.00").display();
        assert_ne!(drawn, "13,500.00");
        assert_eq!(drawn.len(), "13,500.00".len());
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

    fn joined(line: &TextLine) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Which characters a line draws in reverse video -- the caret, and
    /// nothing else in the app draws a field that way.
    fn reversed(line: &TextLine) -> String {
        line.spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// The caret is a block over the character it is on, not a glyph between
    /// two of them: a bar inserted at the caret costs a column, so the value
    /// shifted right of the caret every time the caret moved through it.
    #[test]
    fn a_focused_field_draws_its_caret_on_the_character_it_is_on() {
        let mut field = Field::given("rent");
        field.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        field.edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        let line = field_line(
            "Description",
            Label::from("rent"),
            Some(Caret::in_field(&field)),
        );
        assert!(joined(&line).ends_with("rent"), "{}", joined(&line));
        assert_eq!(reversed(&line), "n");
    }

    /// The caret lands on a character rather than a byte. A span sliced
    /// through the middle of a multi-byte one panics the draw.
    #[test]
    fn a_caret_on_a_multi_byte_character_blocks_the_whole_character() {
        let mut field = Field::given("café");
        field.edit(ctrl('b'));

        let line = field_line(
            "Description",
            Label::from("café"),
            Some(Caret::in_field(&field)),
        );
        assert!(joined(&line).ends_with("café"), "{}", joined(&line));
        assert_eq!(reversed(&line), "é");
    }

    /// At the end of the line there is no character to block out, so the
    /// caret sits on the space past the last one -- the only place it costs a
    /// column, and where a terminal's own cursor sits too.
    #[test]
    fn a_caret_at_the_end_of_a_line_blocks_the_space_past_it() {
        let field = Field::given("rent");
        let line = field_line(
            "Description",
            Label::from("rent"),
            Some(Caret::in_field(&field)),
        );

        assert!(joined(&line).ends_with("rent "), "{}", joined(&line));
        assert_eq!(reversed(&line), " ");
    }

    #[test]
    fn an_unfocused_field_draws_no_caret() {
        let line = field_line("Description", Label::from("rent"), None);
        assert!(joined(&line).ends_with("rent"));
        assert_eq!(reversed(&line), "");
    }

    /// A selector is a choice rather than a buffer, so its caret goes past
    /// the choice, where every caret in the app was drawn before there was
    /// one to place.
    #[test]
    fn a_selector_draws_its_caret_past_the_choice() {
        let line = field_line("Account", Label::from("CHK"), Some(Caret::End));
        assert!(joined(&line).ends_with("CHK "), "{}", joined(&line));
        assert_eq!(reversed(&line), " ");
    }

    /// A caret drawn inside a scrambled figure would count the digits back
    /// out, and it stays out even though a scrambled figure is exactly as
    /// wide as the one it replaces: it is a different string, not merely a
    /// shorter one.
    #[cfg(feature = "demo")]
    #[test]
    fn a_caret_in_a_scrambled_figure_goes_to_the_end_of_it() {
        crate::demo::install_with_salt(7);
        let mut form = ValueForm::money("Amount", "123.45");
        form.edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        let drawn = form.display();
        assert_ne!(drawn, "123.45");
        let line = field_line("Amount", Label::from(drawn.clone()), Some(form.caret()));
        assert!(
            joined(&line).ends_with(&format!("{drawn} ")),
            "{}",
            joined(&line)
        );
        assert_eq!(reversed(&line), " ");
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
                render_txn(frame, &form, &Autocomplete::default());
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
            for caret in [None, Some(Caret::End)] {
                let plain = field_line(label, Label::from("26"), caret.clone());
                let labeled = field_line_labeled(&Label::from(label), Label::from("26"), caret);
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

        let form = TxnForm::add(accounts(), DateField::today(day(2026, 8, 15)), None).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                render_txn(frame, &form, &Autocomplete::default());
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
                .map(|i| field_line("Label", Label::from(i.to_string()), None))
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
