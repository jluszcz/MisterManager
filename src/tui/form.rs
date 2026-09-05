//! The field framework every form in the app is built out of, and the one
//! form that belongs to no single screen.
//!
//! No ratatui in any signature here except [`render_value`] at the bottom:
//! the parsing, the validation, and the suggestion rules are the parts with
//! decisions in them, and they are unit-tested directly.
//!
//! What *draws* a field is `super::widget`, and the two forms the ledgers
//! open are [`super::ledger_form`]. This module is what both of them, and
//! every screen's own form, are written against.

use super::Label;
use super::text::{self, Edit, TextBuffer};
use super::widget::{field_line_labeled, render_fields};
use crate::db::txn::Suggestion;
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{Datelike, Months, NaiveDate, TimeDelta};
use ratatui::Frame;
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
    ///
    /// The birth-date prompt is what wants it. Every `M/D` reading is present
    /// or future and a birth date is decades past, so a shorthand there could
    /// only ever resolve to a wrong year that nothing refuses.
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
    pub(super) fn offset(&self, drawn: &str) -> usize {
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

pub fn render_value(frame: &mut Frame, form: &ValueForm) {
    let lines = vec![field_line_labeled(
        form.label(),
        Label::from(form.display()),
        Some(form.caret()),
    )];
    render_fields(frame, form.title(), lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::db::account;
    use crate::test_support::{cash, day};
    use crate::tui::Account;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    /// The two accounts a label's account segment is resolved against.
    fn accounts() -> Vec<account::Account> {
        vec![cash(1, "CHK"), cash(2, "SAV")]
    }

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
}
