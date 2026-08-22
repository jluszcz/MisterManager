//! The forms that act on one goal: `a` allocate, `e` edit, `n` create, `c`
//! close out.
//!
//! Same shape as `form.rs`'s two — plain state machines driven through
//! `FormFields`, with the parsing and validation unit-tested directly and the
//! render functions at the bottom drawing only.

use super::form::{
    Caret, DateField, Field, Focused, FormFields, Step, field_line, field_line_noted, is_share,
    next_in, parse_share, parse_whole_amount, render_fields, step_index, tax_note,
};
use super::{Account, Label};
use crate::db::goal::GoalEdit;
use crate::db::{AccountId, GoalId};
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, Months, NaiveDate};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllocField {
    Date,
    Amount,
    Note,
}

impl AllocField {
    pub const ORDER: [AllocField; 3] = [AllocField::Date, AllocField::Amount, AllocField::Note];

    pub fn label(self) -> &'static str {
        match self {
            AllocField::Date => "Date",
            AllocField::Amount => "Amount",
            AllocField::Note => "Note",
        }
    }
}

/// A committed allocation, ready for `goal::insert_allocation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub date: NaiveDate,
    pub cents: Cents,
    pub note: Option<String>,
}

/// One allocation against one goal. Backs `a`.
///
/// The amount is **signed**: a negative is a spend against the goal, which is
/// what the sheet's hand-accreted `Current` formulas already encode. It also
/// takes `/N` for a fraction of `unallocated`, which is why the form carries
/// the container's remainder at all.
#[derive(Debug)]
pub struct AllocationForm {
    pub goal_id: GoalId,
    pub focus: AllocField,
    goal_name: String,
    container_name: String,
    /// The container's unallocated remainder, as it stood when the form
    /// opened. A snapshot is enough: the form writes once and closes, and
    /// nothing else can move the figure while it is up.
    unallocated: Cents,
    date: DateField,
    amount: Field,
    note: Field,
}

impl AllocationForm {
    pub fn new(
        goal_id: GoalId,
        goal_name: &str,
        container_name: &str,
        unallocated: Cents,
        today: NaiveDate,
    ) -> AllocationForm {
        AllocationForm {
            goal_id,
            focus: AllocField::Date,
            goal_name: goal_name.to_string(),
            container_name: container_name.to_string(),
            unallocated,
            date: DateField::today(today),
            amount: Field::default(),
            note: Field::default(),
        }
    }

    /// What a `/N` amount comes to, for the form to show beside the field.
    ///
    /// `None` for a typed figure, which needs no resolving, and for a divisor
    /// that will not parse, which has nothing to show -- that one reports
    /// itself on the status line at Enter, like every other bad field.
    pub fn resolved_share(&self) -> Option<Cents> {
        let raw = self.amount.value().trim();
        is_share(raw)
            .then(|| parse_share(raw, self.unallocated).ok())
            .flatten()
    }

    /// The line under the fields: the pot `/N` divides, and the key that
    /// divides it.
    ///
    /// The form carries both because nothing else here can. The remainder is
    /// on the Savings screen behind this modal, and `/N` has no room in the
    /// help table `Topic::Form` shares with the forms that do not offer it.
    ///
    /// Full precision, as the Savings footer keeps: sub-dollar drift is the
    /// whole reason that figure is worth showing.
    pub fn unallocated_line(&self) -> String {
        format!(
            "{} unallocated {} · /N takes 1/N",
            self.container_name,
            crate::demo::figure(self.unallocated)
        )
    }

    pub fn title(&self) -> String {
        format!(
            "Allocate to {} — Tab field · Enter save · Esc cancel",
            self.goal_name
        )
    }

    pub fn display(&self, field: AllocField) -> Label {
        Label::plain(match field {
            AllocField::Date => self.date.display(self.focus == AllocField::Date),
            // The one amount field that can be holding something other than an
            // amount. `/12` is a count, and counts are not masked: what it
            // divides is the unallocated remainder on the line below, which is
            // blocked out there, and `resolved_share` puts the answer beside
            // the field blocked out too. Masking the divisor as well would
            // leave the one field whose text is not a figure with no visible
            // feedback at all -- `/12` and `/2` would read the same right up
            // to Enter.
            AllocField::Amount => match is_share(self.amount.value()) {
                true => self.amount.value().to_string(),
                false => crate::demo::typed(self.amount.value()),
            },
            AllocField::Note => self.note.value().to_string(),
        })
    }

    pub fn commit(&self) -> Result<Allocation> {
        let note = self.note.value().trim().to_string();
        Ok(Allocation {
            date: self.date.parse()?,
            cents: parse_share(self.amount.value(), self.unallocated)?,
            // An empty note is no note, not a note that says nothing.
            note: (!note.is_empty()).then_some(note),
        })
    }
}

impl FormFields for AllocationForm {
    fn next_field(&mut self) {
        self.focus = next_in(&AllocField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&AllocField::ORDER, self.focus, -1);
    }

    // No selector: `←`/`→` step the date and move the caret in the other two.
    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            AllocField::Date => Focused::Date(&mut self.date),
            AllocField::Amount => Focused::Text(&mut self.amount),
            AllocField::Note => Focused::Text(&mut self.note),
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            AllocField::Date => Caret::in_field(self.date.text()),
            AllocField::Amount => Caret::in_field(&self.amount),
            AllocField::Note => Caret::in_field(&self.note),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalField {
    Name,
    Target,
    Date,
    Taxed,
    Interest,
}

impl GoalField {
    pub const ORDER: [GoalField; 5] = [
        GoalField::Name,
        GoalField::Target,
        GoalField::Date,
        GoalField::Taxed,
        GoalField::Interest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GoalField::Name => "Name",
            GoalField::Target => "Target",
            GoalField::Date => "Goal Date",
            GoalField::Taxed => "Taxed",
            GoalField::Interest => "Interest",
        }
    }
}

use crate::goal::NO_TAX_RATE;

/// What committing a `GoalForm` does. `Subject` without the name the border
/// needs, which is the whole of what a caller has to decide between.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalTarget {
    Update(GoalId),
    Create(AccountId),
}

/// Which goal a `GoalForm` is for: one that does not exist yet, named by the
/// container it will land in, or one that does, named by its id.
///
/// One field rather than an `Option<GoalId>` beside an `Option<String>`,
/// because those two could disagree about which job the form is doing and the
/// border would then say one thing while the commit did another.
#[derive(Debug)]
enum Subject {
    New { container: Account },
    Existing(GoalId),
}

/// A goal's name, target, goal date, and whether an interest posting weights
/// it. Backs `e`, and `n` for a goal that does not exist yet.
///
/// A goal's container and its recurring goal entry are not editable: those are
/// what the goal *is*, and changing a container without moving cash would
/// break the reconciliation. Creation is therefore the only time a container
/// is chosen, which is why `New` names it in the border -- under the Savings
/// screen's All filter nothing else on screen says which one `n` defaulted to,
/// the way `Picker` and `Worksheet` always name theirs.
///
/// The two flags are `bool`s rather than `Field`s: they are selectors, so a
/// keystroke cannot leave one saying something that is neither yes nor no.
///
/// `Taxed` is a field of the goal, stored and derived from rather than spent
/// at commit: the Target field holds the **base**, and what the goal is
/// actually funded to is [`GoalForm::tax_note`], beside it.
#[derive(Debug)]
pub struct GoalForm {
    subject: Subject,
    pub focus: GoalField,
    name: Field,
    target: Field,
    date: DateField,
    taxed: bool,
    eligible: bool,
    /// The sales tax rate `Taxed` applies, as it stood when the form
    /// opened. `None` is a database no `Constants` sheet has been imported
    /// into: the form still opens, since an untaxed goal needs no rate, and
    /// only the commit that actually wants one refuses.
    rate: Option<BasisPoints>,
}

impl GoalForm {
    /// The goal date a new goal opens on: the first of the next month.
    ///
    /// A goal date is a deadline, so today is never one -- a goal funded by
    /// today is a goal already due -- which is why this is the one date field
    /// in the app that does not open on today. The first of a month is where
    /// nearly every real goal date lands, and being a month out leaves the
    /// arrows a short walk to the rest.
    ///
    /// A date the calendar cannot hold leaves the field blank, which is the
    /// undated goal the field already supports rather than a new failure.
    fn opening_date(today: NaiveDate) -> Option<NaiveDate> {
        today.with_day(1)?.checked_add_months(Months::new(1))
    }

    /// A blank form, for a goal that does not exist yet.
    pub fn add(container: Account, rate: Option<BasisPoints>, today: NaiveDate) -> GoalForm {
        GoalForm {
            subject: Subject::New { container },
            focus: GoalField::Name,
            name: Field::prefilled(""),
            target: Field::prefilled(""),
            date: match GoalForm::opening_date(today) {
                Some(date) => DateField::on(today, date),
                None => DateField::blank(today),
            },
            taxed: false,
            // A goal typed from scratch takes interest, like every goal the
            // sheet ever had.
            eligible: true,
            rate,
        }
    }

    // Every parameter is a distinct field of an existing goal, prefilling a
    // form that opens on one -- there is no group of them a caller would
    // otherwise pass as a struct.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        goal_id: GoalId,
        name: &str,
        base: Cents,
        date: Option<NaiveDate>,
        interest_eligible: bool,
        taxed: bool,
        rate: Option<BasisPoints>,
        today: NaiveDate,
    ) -> GoalForm {
        GoalForm {
            subject: Subject::Existing(goal_id),
            focus: GoalField::Name,
            name: Field::given(name),
            // The base, which is what the table holds. What it comes to is
            // beside it, in `tax_note`.
            target: Field::given(base.to_string()),
            date: DateField::given(today, date),
            taxed,
            eligible: interest_eligible,
            rate,
        }
    }

    /// What committing this form does: update that goal, or create one in that
    /// container. Read off the same field the border is, so the write cannot
    /// land somewhere the form did not say it would.
    pub fn target(&self) -> GoalTarget {
        match &self.subject {
            Subject::Existing(id) => GoalTarget::Update(*id),
            Subject::New { container } => GoalTarget::Create(container.id()),
        }
    }

    pub fn title(&self) -> Label {
        match &self.subject {
            Subject::Existing(_) => Label::from("Edit goal — Tab field · Enter save · Esc cancel"),
            Subject::New { container } => Label::plain("New goal in ")
                .account(container.clone())
                .text(" — Tab field · Enter save · Esc cancel"),
        }
    }

    pub fn display(&self, field: GoalField) -> Label {
        Label::plain(match field {
            GoalField::Name => self.name.value().to_string(),
            GoalField::Target => crate::demo::typed(self.target.value()),
            GoalField::Date => self.date.display(self.focus == GoalField::Date),
            GoalField::Taxed => if self.taxed { "yes" } else { "no" }.to_string(),
            GoalField::Interest => if self.eligible { "yes" } else { "no" }.to_string(),
        })
    }

    /// The note beside the Target: what the base in the field comes to once
    /// the tax lambda has had it -- the figure every reader will derive.
    ///
    /// Empty whenever there is nothing to say -- the flag is off, the field is
    /// not a whole figure yet, or no rate is on record -- rather than a guess
    /// at one of the three. The commit does not go through this path: a bad
    /// target and a missing rate are errors it has to report by name, and
    /// this cannot tell the caller which of the two it hit.
    pub fn tax_note(&self) -> String {
        tax_note(self.taxed, self.target.value(), self.rate)
    }

    pub fn commit(&self) -> Result<GoalEdit> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        let base_cents = parse_whole_amount(self.target.value())?;
        // Refused even though nothing here needs the rate any more: letting it
        // through would write precisely the row the read side calls corrupt,
        // and this is the one place that can ask for the rate before there is
        // a goal to be broken by its absence.
        if self.taxed {
            self.rate.context(NO_TAX_RATE)?;
        }
        Ok(GoalEdit {
            name,
            base_cents,
            // An empty date field is an undated goal -- rows 6-26 of the sheet.
            goal_date: self.date.parse_opt()?,
            interest_eligible: self.eligible,
            taxed: self.taxed,
        })
    }
}

impl FormFields for GoalForm {
    fn next_field(&mut self) {
        self.focus = next_in(&GoalField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&GoalField::ORDER, self.focus, -1);
    }

    // Both selectors here hold two values, so both directions are the same
    // flip. An undated goal has no date to step, which is what keeps an arrow
    // press from dating one.
    fn cycle(&mut self, _step: Step) {
        match self.focus {
            GoalField::Taxed => self.taxed = !self.taxed,
            GoalField::Interest => self.eligible = !self.eligible,
            GoalField::Name | GoalField::Target | GoalField::Date => {}
        }
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            GoalField::Name => Focused::Text(&mut self.name),
            GoalField::Target => Focused::Text(&mut self.target),
            GoalField::Date => Focused::Date(&mut self.date),
            GoalField::Taxed | GoalField::Interest => Focused::Selector,
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            GoalField::Name => Caret::in_field(&self.name),
            GoalField::Target => Caret::in_field(&self.target),
            GoalField::Date => Caret::in_field(self.date.text()),
            GoalField::Taxed | GoalField::Interest => Caret::End,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CloseField {
    Date,
    Destination,
}

impl CloseField {
    pub const ORDER: [CloseField; 2] = [CloseField::Date, CloseField::Destination];

    pub fn label(self) -> &'static str {
        match self {
            CloseField::Date => "Date",
            CloseField::Destination => "To",
        }
    }
}

/// A committed ending, ready for `goal::move_value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseOut {
    pub date: NaiveDate,
    /// `None` is the abandon ending: the value returns to unallocated.
    pub to: Option<GoalId>,
}

/// Ending a goal. Backs `c`.
///
/// The only real field is the destination, and it is a selector rather than a
/// text field so a goal that does not exist is unrepresentable. The amount is
/// the goal's whole balance and is deliberately absent: a partial move is two
/// `a` allocations, and calling that a close-out would leave a goal closed
/// with money in it.
#[derive(Debug)]
pub struct CloseForm {
    pub goal_id: GoalId,
    pub focus: CloseField,
    goal_name: String,
    balance: Cents,
    date: DateField,
    /// `None` is unallocated, and is always first: abandoning is the ending
    /// that needs no second goal, so it is the safe default.
    destinations: Vec<(Option<GoalId>, String)>,
    destination: usize,
}

impl CloseForm {
    /// `siblings` are the *open goals of the same container*, minus this one.
    /// Crossing containers would break both reconciliations at once, since no
    /// cash moved between the accounts.
    pub fn new(
        goal_id: GoalId,
        goal_name: &str,
        balance: Cents,
        siblings: Vec<(GoalId, String)>,
        today: NaiveDate,
    ) -> CloseForm {
        let mut destinations = vec![(None, "— unallocated —".to_string())];
        destinations.extend(siblings.into_iter().map(|(id, name)| (Some(id), name)));
        CloseForm {
            goal_id,
            focus: CloseField::Date,
            goal_name: goal_name.to_string(),
            balance,
            date: DateField::today(today),
            destinations,
            destination: 0,
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Close out {} ({}) — ←/→ destination · Enter save · Esc cancel",
            self.goal_name,
            crate::demo::figure(self.balance)
        )
    }

    pub fn display(&self, field: CloseField) -> Label {
        Label::plain(match field {
            CloseField::Date => self.date.display(self.focus == CloseField::Date),
            CloseField::Destination => self
                .destinations
                .get(self.destination)
                .map(|(_, label)| label.clone())
                .unwrap_or_default(),
        })
    }

    pub fn commit(&self) -> Result<CloseOut> {
        Ok(CloseOut {
            date: self.date.parse()?,
            to: self
                .destinations
                .get(self.destination)
                .and_then(|(id, _)| *id),
        })
    }
}

impl FormFields for CloseForm {
    fn next_field(&mut self) {
        self.focus = next_in(&CloseField::ORDER, self.focus, 1);
    }

    fn previous_field(&mut self) {
        self.focus = next_in(&CloseField::ORDER, self.focus, -1);
    }

    // `←`/`→` step the date on one field and cycle the destination on the
    // other, and neither may reach across.
    fn cycle(&mut self, step: Step) {
        self.destination = step_index(self.destination, self.destinations.len(), step.direction());
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            CloseField::Date => Focused::Date(&mut self.date),
            CloseField::Destination => Focused::Selector,
        }
    }

    fn caret(&self) -> Caret {
        match self.focus {
            CloseField::Date => Caret::in_field(self.date.text()),
            CloseField::Destination => Caret::End,
        }
    }
}

use ratatui::Frame;
use ratatui::text::Line as TextLine;

/// Draws the allocate-against-a-goal modal: `a`'s form.
///
/// One line taller than the other two forms here, for the pot `/N` divides.
pub fn render_allocation(frame: &mut Frame, form: &AllocationForm) {
    let share = form
        .resolved_share()
        .map(|cents| format!("= {}", crate::demo::whole_figure(cents)))
        .unwrap_or_default();
    let mut lines: Vec<TextLine> = AllocField::ORDER
        .iter()
        .map(|f| {
            let note = if *f == AllocField::Amount {
                share.as_str()
            } else {
                ""
            };
            field_line_noted(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
                note,
            )
        })
        .collect();
    lines.push(TextLine::from(format!("  {}", form.unallocated_line())));
    render_fields(frame, form.title(), lines);
}

/// Draws the goal modal: `e`'s form, and `n`'s.
pub fn render_goal(frame: &mut Frame, form: &GoalForm) {
    let note = form.tax_note();
    let lines: Vec<TextLine> = GoalField::ORDER
        .iter()
        .map(|f| {
            // The Target field holds the base rather than what the goal is
            // funded to, so the note says what it comes to beside the figure
            // it is derived from rather than beside itself.
            let note = if *f == GoalField::Target {
                note.as_str()
            } else {
                ""
            };
            field_line_noted(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
                note,
            )
        })
        .collect();
    render_fields(frame, form.title(), lines);
}

/// Draws the close-out-a-goal modal: `c`'s form.
pub fn render_close(frame: &mut Frame, form: &CloseForm) {
    let lines: Vec<TextLine> = CloseField::ORDER
        .iter()
        .map(|f| {
            field_line(
                f.label(),
                form.display(*f),
                (form.focus == *f).then(|| form.caret()),
            )
        })
        .collect();
    render_fields(frame, form.title(), lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Group, Kind};
    use crate::tui::form::{backspace_key, char_key};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn today() -> NaiveDate {
        day(2026, 8, 21)
    }

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "SAV".into(),
                name: "Rainy Day".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            account::Account {
                id: AccountId(2),
                code: "NST".into(),
                name: "Nest Egg".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: None,
            },
        ]
    }

    /// The container and its remainder matter only to `/N`; every test that
    /// types a figure gets a name and an empty pot.
    fn alloc(name: &str) -> AllocationForm {
        AllocationForm::new(GoalId(7), name, "Rainy Day", Cents::ZERO, day(2026, 8, 16))
    }

    fn typed(form: &mut AllocationForm, field: AllocField, text: &str) {
        while form.focus != field {
            form.next_field();
        }
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    #[test]
    fn an_allocation_prefills_todays_date_and_commits_what_was_typed() {
        let mut form = alloc("Apple Watch");
        assert_eq!(form.display(AllocField::Date).plain_text(), "2026-08-16");

        typed(&mut form, AllocField::Amount, "$72");
        typed(&mut form, AllocField::Note, "birthday money");

        let allocated = form.commit().unwrap();
        assert_eq!(allocated.date, day(2026, 8, 16));
        assert_eq!(allocated.cents, Cents(7_200));
        assert_eq!(allocated.note.as_deref(), Some("birthday money"));
    }

    /// The amount is signed and a negative is a spend -- that is what a
    /// hand-accreted `=1200+72+...-450+87` cell already encodes, and it is
    /// why goal balances are a ledger rather than a typed number.
    #[test]
    fn a_negative_allocation_is_a_spend_and_is_accepted() {
        let mut form = alloc("Bill Payments");
        typed(&mut form, AllocField::Amount, "-450");
        assert_eq!(form.commit().unwrap().cents, Cents(-45_000));
    }

    /// Every figure a goal carries is a whole dollar, in both directions.
    /// The cents a goal drifts by come from interest and rounding, not from
    /// the keyboard, and they collect in the container's unallocated
    /// remainder rather than inside a goal.
    #[test]
    fn an_allocation_with_cents_in_it_is_refused() {
        let mut form = alloc("Bill Payments");
        typed(&mut form, AllocField::Amount, "-450.85");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("-450.85"), "{err}");
    }

    /// A note is optional; an empty one must not become an empty string in the
    /// database, where it would read as a note that says nothing.
    #[test]
    fn an_empty_note_commits_as_no_note() {
        let mut form = alloc("Dropbox");
        typed(&mut form, AllocField::Amount, "64");
        assert_eq!(form.commit().unwrap().note, None);
    }

    #[test]
    fn an_allocation_with_no_amount_is_refused() {
        let form = alloc("Dropbox");
        assert!(form.commit().is_err());
    }

    #[test]
    fn an_allocations_date_must_be_yyyy_mm_dd() {
        let mut form = alloc("Dropbox");
        typed(&mut form, AllocField::Amount, "64");
        while form.focus != AllocField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        for c in "08/16/2026".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("08/16/2026"), "{err}");
    }

    /// `/N` divides the container's unallocated remainder, so splitting a
    /// remainder across goals needs no calculator. The cents floor away and
    /// stay in the remainder.
    #[test]
    fn an_allocation_of_a_share_takes_a_fraction_of_the_containers_remainder() {
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "/2");
        assert_eq!(form.commit().unwrap().cents, Cents::from_dollars(1300));
    }

    /// Two figures reach this modal beside whatever is typed -- the share a
    /// `/N` resolves to, and the container's remainder underneath -- and a
    /// demo blocks both. The divisor itself is a count and stays, which is
    /// what leaves the field feedback: blocked, `/12` and `/2` would read the
    /// same right up to Enter, and the answer beside them is blocked already.
    #[test]
    fn a_demo_blocks_the_share_and_the_remainder_but_not_the_divisor() {
        crate::demo::install(true);
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "/12");
        let text = rendered(&form);

        assert!(!text.contains("216"), "the resolved share survived: {text}");
        assert!(!text.contains("2,600"), "the remainder survived: {text}");
        assert!(text.contains("/12"), "the divisor is a count: {text}");
        assert!(text.contains("██████"), "nothing was blocked: {text}");
        assert!(text.contains("Lego"), "the goal name must stay: {text}");
        assert!(text.contains("2026-08-16"), "the date must stay: {text}");
    }

    /// The other half of the same field. A typed figure *is* a figure, and
    /// the form opens prefilled on an edit -- a field showing what is already
    /// there publishes it to whoever is watching.
    #[test]
    fn a_demo_blocks_an_amount_typed_into_the_same_field() {
        crate::demo::install(true);
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "1234");
        let text = rendered(&form);

        assert!(!text.contains("1234"), "the typed amount survived: {text}");
        assert!(text.contains("██████"), "nothing was blocked: {text}");
    }

    fn rendered(form: &AllocationForm) -> String {
        use crate::tui::MIN_WIDTH;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 12)).unwrap();
        terminal
            .draw(|frame| {
                render_allocation(frame, form);
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// A divisor is not a figure, so the form resolves it on screen rather
    /// than making the owner commit to find out what they typed.
    #[test]
    fn a_share_shows_the_figure_it_resolves_to() {
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "/12");
        assert_eq!(form.resolved_share(), Some(Cents::from_dollars(216)));
    }

    /// A typed figure needs no resolving, and a divisor that will not parse
    /// has nothing to resolve to -- it reports itself at Enter, like every
    /// other bad field.
    #[test]
    fn only_a_share_resolves_to_anything() {
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "1234");
        assert_eq!(form.resolved_share(), None);

        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "/0");
        assert_eq!(form.resolved_share(), None);
    }

    /// The pot `/N` divides is not on screen anywhere else, and the key that
    /// divides it has no room in the shared form help table -- so the form
    /// carries both itself. Full precision, as the Savings footer does: the
    /// drift is the whole reason that figure is worth showing.
    #[test]
    fn the_form_names_the_container_and_the_remainder_a_share_divides() {
        let form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        let line = form.unallocated_line();
        assert!(line.contains("Rainy Day"), "{line}");
        assert!(line.contains("2,600.17"), "{line}");
        assert!(line.contains("/N"), "{line}");
    }

    /// A goal form over a database that knows the sales tax rate, which is
    /// what every question about the `Taxed` selector needs.
    fn taxable(name: &str, base: Cents) -> GoalForm {
        GoalForm::new(
            GoalId(7),
            name,
            base,
            None,
            true,
            false,
            Some(BasisPoints(625)),
            today(),
        )
    }

    fn typed_goal(form: &mut GoalForm, field: GoalField, text: &str) {
        while form.focus != field {
            form.next_field();
        }
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    #[test]
    fn editing_a_goal_prefills_every_field_and_commits_the_changes() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            Some(day(2026, 12, 1)),
            true,
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Name).plain_text(), "Couch");
        assert_eq!(form.display(GoalField::Target).plain_text(), "1,000.00");
        assert_eq!(form.display(GoalField::Date).plain_text(), "2026-12-01");

        typed_goal(&mut form, GoalField::Name, " Mk II");
        let edit = form.commit().unwrap();
        assert_eq!(edit.name, "Couch Mk II");
        assert_eq!(edit.base_cents, Cents(100_000));
        assert_eq!(edit.goal_date, Some(day(2026, 12, 1)));
    }

    /// A goal's target is a whole dollar. The prefill is the stored figure
    /// with its cents, so a goal imported off a fractional cell shows what it
    /// really holds and has to be rounded by hand rather than silently moved
    /// the first time the form is opened.
    #[test]
    fn a_goal_target_with_cents_in_it_is_refused() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_050),
            None,
            true,
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Target).plain_text(), "1,000.50");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("1,000.50"), "{err}");

        while form.focus != GoalField::Target {
            form.next_field();
        }
        for _ in 0.."1,000.50".len() {
            form.edit(backspace_key());
        }
        for c in "1000".chars() {
            form.edit(char_key(c));
        }
        assert_eq!(form.commit().unwrap().base_cents, Cents(100_000));
    }

    /// Rows 6-26 of the sheet have no goal date, so clearing the field has to
    /// be how a dated goal becomes an undated one.
    #[test]
    fn clearing_the_date_field_makes_a_goal_undated() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            Some(day(2026, 12, 1)),
            true,
            false,
            None,
            today(),
        );
        while form.focus != GoalField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    #[test]
    fn an_undated_goal_opens_with_an_empty_date_field() {
        let form = GoalForm::new(
            GoalId(7),
            "Bill Payments",
            Cents(1_500_000),
            None,
            true,
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Date).plain_text(), "");
        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    /// Eligibility is the one field here that is not typed, so `←`/`→` have
    /// to reach it.
    #[test]
    fn the_choice_keys_flip_interest_eligibility() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Down Payment",
            Cents(100_000),
            None,
            false,
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Interest).plain_text(), "no");

        while form.focus != GoalField::Interest {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.display(GoalField::Interest).plain_text(), "yes");
        assert!(form.commit().unwrap().interest_eligible);
    }

    /// The importer marks the down-payment bucket ineligible, and opening its
    /// form must not quietly hand the flag back.
    #[test]
    fn an_ineligible_goal_commits_unchanged_as_ineligible() {
        let form = GoalForm::new(
            GoalId(7),
            "Down Payment",
            Cents(100_000),
            None,
            false,
            false,
            None,
            today(),
        );
        assert!(!form.commit().unwrap().interest_eligible);
    }

    /// A goal typed from scratch takes interest, like every goal the sheet
    /// ever had.
    #[test]
    fn a_goal_typed_from_scratch_opens_interest_eligible() {
        let mut form = GoalForm::add(Account::named(&accounts(), AccountId(1)), None, today());
        typed_goal(&mut form, GoalField::Name, "Couch");
        typed_goal(&mut form, GoalField::Target, "1000");
        assert_eq!(form.display(GoalField::Interest).plain_text(), "yes");
        assert!(form.commit().unwrap().interest_eligible);
    }

    /// A goal date is a deadline, and today is never one: a goal funded by
    /// today is a goal already due. The first of the next month is the date
    /// nearly every real one lands on, and it is one field the owner then
    /// nudges with the arrows rather than types out.
    #[test]
    fn a_new_goal_opens_on_the_first_of_the_next_month() {
        let form = GoalForm::add(
            Account::named(&accounts(), AccountId(1)),
            None,
            day(2026, 8, 21),
        );
        assert_eq!(form.display(GoalField::Date).plain_text(), "2026-09-01");
    }

    /// December's next month is next January, not month thirteen.
    #[test]
    fn a_new_goal_opened_in_december_lands_in_the_new_year() {
        let form = GoalForm::add(
            Account::named(&accounts(), AccountId(1)),
            None,
            day(2026, 12, 31),
        );
        assert_eq!(form.display(GoalField::Date).plain_text(), "2027-01-01");
    }

    /// The prefill is a default, not a decision: an undated goal is rows 6-26
    /// of the sheet, and clearing the field is still how one is made.
    #[test]
    fn clearing_a_new_goals_date_still_makes_an_undated_goal() {
        let mut form = GoalForm::add(Account::named(&accounts(), AccountId(1)), None, today());
        typed_goal(&mut form, GoalField::Name, "Couch");
        typed_goal(&mut form, GoalField::Target, "1000");
        while form.focus != GoalField::Date {
            form.next_field();
        }
        for _ in 0.."2026-09-01".len() {
            form.edit(backspace_key());
        }

        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    /// A new goal's container is named in the border in the same color the
    /// Account column would give it, since under the Savings screen's All
    /// filter this is the only place on screen that says which container `n`
    /// defaulted to.
    #[test]
    fn the_new_goal_title_names_its_container_as_an_account() {
        let form = GoalForm::add(Account::named(&accounts(), AccountId(2)), None, today());
        let title = form.title();
        assert!(
            title.plain_text().contains("New goal in Nest Egg"),
            "{}",
            title.plain_text()
        );
        assert_eq!(title.accounts().len(), 1);
        assert_eq!(title.accounts()[0].id(), AccountId(2));
    }

    /// The field is a selector, so a keystroke meant for a text field must
    /// not land in it -- nor anywhere else.
    #[test]
    fn typing_on_the_interest_field_changes_nothing() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            None,
            true,
            false,
            None,
            today(),
        );
        typed_goal(&mut form, GoalField::Interest, "no");

        assert_eq!(form.display(GoalField::Interest).plain_text(), "yes");
        assert_eq!(form.display(GoalField::Name).plain_text(), "Couch");
    }

    #[test]
    fn a_goal_with_an_empty_name_is_refused() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            None,
            true,
            false,
            None,
            today(),
        );
        while form.focus != GoalField::Name {
            form.next_field();
        }
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    /// The flag is stored and the figure is not rewritten: what the table
    /// holds is the base, and every reader derives the target from it. A
    /// commit that taxed the figure here would tax it again on the next edit.
    #[test]
    fn a_taxed_goal_commits_its_base_and_the_flag() {
        let mut form = taxable("Couch", Cents(100_000));
        while form.focus != GoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.display(GoalField::Taxed).plain_text(), "yes");
        let edit = form.commit().unwrap();
        assert_eq!(edit.base_cents, Cents(100_000));
        assert!(edit.taxed);
    }

    /// The flag is a field of the goal now, so the form opens on whatever the
    /// goal holds. Opening a taxed goal with the selector at `no` would make
    /// every edit of it silently untax it.
    #[test]
    fn a_form_opened_on_a_taxed_goal_opens_with_the_selector_on() {
        let form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            None,
            true,
            true,
            Some(BasisPoints(625)),
            today(),
        );

        assert_eq!(form.display(GoalField::Taxed).plain_text(), "yes");
        assert_eq!(
            form.display(GoalField::Target).plain_text(),
            "1,000.00",
            "the field holds the base"
        );
        assert_eq!(form.tax_note(), "(1,065 w/ tax)");
        let edit = form.commit().unwrap();
        assert_eq!(edit.base_cents, Cents(100_000));
        assert!(edit.taxed, "the flag round-trips");
    }

    /// A rate on record is not the same as tax being asked for: the flag is
    /// what decides, and it opens off.
    #[test]
    fn a_goal_opens_untaxed_and_commits_the_target_untouched() {
        let form = taxable("Couch", Cents(100_000));
        assert_eq!(form.display(GoalField::Taxed).plain_text(), "no");
        assert_eq!(form.commit().unwrap().base_cents, Cents(100_000));
    }

    /// The Target field goes on holding the base, so what the goal is funded
    /// to has to be said somewhere. The note is that somewhere.
    #[test]
    fn the_note_beside_the_target_says_what_it_comes_to_with_tax() {
        let mut form = taxable("Couch", Cents(100_000));
        assert_eq!(form.tax_note(), "", "nothing to say while the flag is off");

        while form.focus != GoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "(1,065 w/ tax)");
        assert_eq!(
            form.display(GoalField::Target).plain_text(),
            "1,000.00",
            "the field itself still holds the base"
        );
    }

    /// A half-typed target is not a figure yet, and a note that guessed at
    /// one would flicker through amounts the owner never asked about.
    #[test]
    fn the_note_stays_empty_until_the_target_is_a_whole_figure() {
        let mut form = taxable("Couch", Cents(100_000));
        while form.focus != GoalField::Target {
            form.next_field();
        }
        for _ in 0.."1,000.00".len() {
            form.edit(backspace_key());
        }
        typed_goal(&mut form, GoalField::Taxed, "");
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
    }

    /// No rate on record is a database nobody has imported `Constants` into.
    /// The form still opens -- an untaxed goal needs no rate -- and only asks
    /// when the answer is actually wanted.
    #[test]
    fn a_taxed_goal_with_no_rate_on_record_is_refused_rather_than_saved_untaxed() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            None,
            true,
            false,
            None,
            today(),
        );
        while form.focus != GoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("tax rate"), "{err}");
    }

    /// The second selector on the form, and it cycles like the first.
    #[test]
    fn the_choice_keys_flip_the_tax_selector_both_ways() {
        let mut form = taxable("Couch", Cents(100_000));
        while form.focus != GoalField::Taxed {
            form.next_field();
        }
        form.choice(Step::NEXT);
        assert_eq!(form.display(GoalField::Taxed).plain_text(), "yes");
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(GoalField::Taxed).plain_text(), "no");
    }

    /// A selector, so a keystroke meant for a text field must not land in it
    /// -- nor anywhere else.
    #[test]
    fn typing_on_the_tax_field_changes_nothing() {
        let mut form = taxable("Couch", Cents(100_000));
        typed_goal(&mut form, GoalField::Taxed, "yes");

        assert_eq!(form.display(GoalField::Taxed).plain_text(), "no");
        assert_eq!(form.display(GoalField::Name).plain_text(), "Couch");
        assert_eq!(form.commit().unwrap().base_cents, Cents(100_000));
    }

    /// A goal typed from scratch is untaxed too, like every other flag on the
    /// form: nothing has told it otherwise yet.
    #[test]
    fn a_goal_typed_from_scratch_opens_untaxed() {
        let form = GoalForm::add(
            Account::named(&accounts(), AccountId(1)),
            Some(BasisPoints(625)),
            today(),
        );
        assert_eq!(form.display(GoalField::Taxed).plain_text(), "no");
    }

    fn siblings() -> Vec<(GoalId, String)> {
        vec![
            (GoalId(8), "Rug".to_string()),
            (GoalId(9), "Lamp".to_string()),
        ]
    }

    #[test]
    fn a_close_out_opens_on_unallocated_and_cycles_through_the_containers_goals() {
        let mut form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        assert_eq!(
            form.display(CloseField::Destination).plain_text(),
            "— unallocated —"
        );
        assert_eq!(form.commit().unwrap().to, None, "the default is abandon");

        while form.focus != CloseField::Destination {
            form.next_field();
        }
        form.choice(Step::NEXT);
        assert_eq!(form.display(CloseField::Destination).plain_text(), "Rug");
        assert_eq!(form.commit().unwrap().to, Some(GoalId(8)));

        form.choice(Step::NEXT);
        assert_eq!(form.display(CloseField::Destination).plain_text(), "Lamp");
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(CloseField::Destination).plain_text(),
            "— unallocated —",
            "the cycle wraps back to abandon"
        );
    }

    /// The amount is not editable, so the title is where the user reads what
    /// is about to move.
    #[test]
    fn a_close_out_names_the_goal_and_the_whole_balance_that_will_move() {
        let form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        assert!(form.title().contains("Couch"), "{}", form.title());
        assert!(form.title().contains("600.00"), "{}", form.title());
    }

    /// The balance is the whole point of the title -- it is what is about to
    /// move -- so it is exactly the figure a demo has to block.
    #[test]
    fn a_demo_blocks_the_balance_a_close_out_is_about_to_move() {
        crate::demo::install(true);
        let form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        assert!(!form.title().contains("600.00"), "{}", form.title());
        assert!(form.title().contains("██████"), "{}", form.title());
        assert!(form.title().contains("Couch"), "{}", form.title());
    }

    /// Every date field in the app steps a day at a time under `←`/`→`, and
    /// the allocation form's is the one field the arrows had nothing to do on.
    #[test]
    fn the_arrows_step_an_allocation_date_by_a_day() {
        let mut form = alloc("Apple Watch");
        form.choice(Step::NEXT);
        assert_eq!(form.display(AllocField::Date).plain_text(), "2026-08-17");
        form.choice(Step::PREVIOUS);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(AllocField::Date).plain_text(), "2026-08-15");
    }

    /// The arrows are a nudge on a date that is already there: a field the
    /// owner is halfway through typing keeps what they typed.
    #[test]
    fn the_arrows_leave_a_half_typed_allocation_date_alone() {
        let mut form = alloc("Apple Watch");
        while form.focus != AllocField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.edit(backspace_key());
        }
        for c in "2026-".chars() {
            form.edit(char_key(c));
        }
        form.choice(Step::NEXT);
        assert_eq!(form.display(AllocField::Date).plain_text(), "2026-");
    }

    /// The amount is where `/N` lives and the note is free text: neither may
    /// move when the arrows are pressed off the date.
    #[test]
    fn the_arrows_do_nothing_away_from_the_allocation_date() {
        let mut form = alloc("Apple Watch");
        typed(&mut form, AllocField::Amount, "72");
        form.choice(Step::NEXT);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(AllocField::Amount).plain_text(), "72");
        assert_eq!(form.display(AllocField::Date).plain_text(), "2026-08-16");
    }

    #[test]
    fn the_arrows_step_a_goal_date_by_a_day() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            Some(day(2026, 12, 31)),
            true,
            false,
            None,
            today(),
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(GoalField::Date).plain_text(),
            "2026-12-31",
            "the name field is focused, and the arrows must stay off the date"
        );

        while form.focus != GoalField::Date {
            form.next_field();
        }
        form.choice(Step::NEXT);
        assert_eq!(form.display(GoalField::Date).plain_text(), "2027-01-01");
    }

    /// An undated goal stays undated: there is no date to step, and seeding
    /// one would date a goal by pressing an arrow at it.
    #[test]
    fn the_arrows_leave_an_undated_goal_undated() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Bill Payments",
            Cents(1_500_000),
            None,
            true,
            false,
            None,
            today(),
        );
        while form.focus != GoalField::Date {
            form.next_field();
        }
        form.choice(Step::NEXT);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(GoalField::Date).plain_text(), "");
        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    #[test]
    fn the_arrows_step_a_close_out_date_by_a_day() {
        let mut form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        form.choice(Step::PREVIOUS);
        assert_eq!(form.display(CloseField::Date).plain_text(), "2026-08-15");
        assert_eq!(
            form.display(CloseField::Destination).plain_text(),
            "— unallocated —",
            "stepping the date must not move the destination"
        );
    }

    /// A container with one goal in it still has to be abandonable.
    #[test]
    fn a_close_out_with_no_sibling_goals_offers_only_unallocated() {
        let mut form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            Vec::new(),
            day(2026, 8, 16),
        );
        while form.focus != CloseField::Destination {
            form.next_field();
        }
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(CloseField::Destination).plain_text(),
            "— unallocated —"
        );
        assert_eq!(form.commit().unwrap().to, None);
    }
}
