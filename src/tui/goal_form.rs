//! The forms that act on one goal: `a` allocate, `e` edit, `n` create, `c`
//! close out.
//!
//! Same shape as `form.rs`'s two — plain state machines driven through
//! `FormFields`, with the parsing and validation unit-tested directly and the
//! render functions at the bottom drawing only.

use super::form::{
    Field, FormFields, field_line, field_line_noted, next_in, parse_date, parse_share,
    parse_whole_amount, render_fields, step_index,
};
use crate::db::goal::GoalEdit;
use crate::db::{AccountId, GoalId};
use crate::money::Cents;
use anyhow::{Result, ensure};
use chrono::NaiveDate;

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
    date: Field,
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
            date: Field::date(today),
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
        raw.starts_with('/')
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
            self.container_name, self.unallocated
        )
    }

    pub fn title(&self) -> String {
        format!(
            "Allocate to {} — Tab field · Enter save · Esc cancel",
            self.goal_name
        )
    }

    pub fn display(&self, field: AllocField) -> String {
        match field {
            AllocField::Date => self.date.value().to_string(),
            AllocField::Amount => self.amount.value().to_string(),
            AllocField::Note => self.note.value().to_string(),
        }
    }

    pub fn commit(&self) -> Result<Allocation> {
        let note = self.note.value().trim().to_string();
        Ok(Allocation {
            date: parse_date(self.date.value())?,
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

    // The date is the only field `←`/`→` reach: the amount takes `/N` and the
    // note is free text, and neither may move when an arrow is pressed.
    fn next_choice(&mut self) {
        if self.focus == AllocField::Date {
            self.date.step_date(1);
        }
    }

    fn previous_choice(&mut self) {
        if self.focus == AllocField::Date {
            self.date.step_date(-1);
        }
    }

    fn type_char(&mut self, c: char) {
        match self.focus {
            AllocField::Date => self.date.push(c),
            AllocField::Amount => self.amount.push(c),
            AllocField::Note => self.note.push(c),
        }
    }

    fn backspace(&mut self) {
        match self.focus {
            AllocField::Date => self.date.backspace(),
            AllocField::Amount => self.amount.backspace(),
            AllocField::Note => self.note.backspace(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalField {
    Name,
    Target,
    Date,
    Interest,
}

impl GoalField {
    pub const ORDER: [GoalField; 4] = [
        GoalField::Name,
        GoalField::Target,
        GoalField::Date,
        GoalField::Interest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GoalField::Name => "Name",
            GoalField::Target => "Target",
            GoalField::Date => "Goal Date",
            GoalField::Interest => "Interest",
        }
    }
}

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
    New {
        container: AccountId,
        container_name: String,
    },
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
/// Eligibility is a `bool` rather than a `Field`: it is a selector, so a
/// keystroke cannot leave it saying something that is neither yes nor no.
#[derive(Debug)]
pub struct GoalForm {
    subject: Subject,
    pub focus: GoalField,
    name: Field,
    target: Field,
    date: Field,
    eligible: bool,
}

impl GoalForm {
    /// A blank form, for a goal that does not exist yet.
    pub fn add(container: AccountId, container_name: &str) -> GoalForm {
        GoalForm {
            subject: Subject::New {
                container,
                container_name: container_name.to_string(),
            },
            focus: GoalField::Name,
            name: Field::prefilled(""),
            target: Field::prefilled(""),
            date: Field::prefilled(""),
            // A goal typed from scratch takes interest, like every goal the
            // sheet ever had.
            eligible: true,
        }
    }

    pub fn new(
        goal_id: GoalId,
        name: &str,
        target: Cents,
        date: Option<NaiveDate>,
        interest_eligible: bool,
    ) -> GoalForm {
        GoalForm {
            subject: Subject::Existing(goal_id),
            focus: GoalField::Name,
            name: Field::given(name),
            target: Field::given(target.to_string()),
            date: Field::given(
                date.map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            ),
            eligible: interest_eligible,
        }
    }

    /// What committing this form does: update that goal, or create one in that
    /// container. Read off the same field the border is, so the write cannot
    /// land somewhere the form did not say it would.
    pub fn target(&self) -> GoalTarget {
        match self.subject {
            Subject::Existing(id) => GoalTarget::Update(id),
            Subject::New { container, .. } => GoalTarget::Create(container),
        }
    }

    pub fn title(&self) -> String {
        match &self.subject {
            Subject::Existing(_) => "Edit goal — Tab field · Enter save · Esc cancel".to_string(),
            Subject::New { container_name, .. } => {
                format!("New goal in {container_name} — Tab field · Enter save · Esc cancel")
            }
        }
    }

    pub fn display(&self, field: GoalField) -> String {
        match field {
            GoalField::Name => self.name.value().to_string(),
            GoalField::Target => self.target.value().to_string(),
            GoalField::Date => self.date.value().to_string(),
            GoalField::Interest => if self.eligible { "yes" } else { "no" }.to_string(),
        }
    }

    pub fn commit(&self) -> Result<GoalEdit> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        let raw_date = self.date.value().trim();
        Ok(GoalEdit {
            name,
            goal_cents: parse_whole_amount(self.target.value())?,
            // An empty date field is an undated goal -- rows 6-26 of the sheet.
            goal_date: if raw_date.is_empty() {
                None
            } else {
                Some(parse_date(raw_date)?)
            },
            interest_eligible: self.eligible,
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

    // Eligibility is the only selector here, and it has two values, so both
    // directions are the same flip. The goal date is the one field where the
    // two directions differ -- and an undated goal has no date to step, which
    // is what keeps an arrow press from dating one.
    fn next_choice(&mut self) {
        match self.focus {
            GoalField::Date => self.date.step_date(1),
            GoalField::Interest => self.eligible = !self.eligible,
            GoalField::Name | GoalField::Target => {}
        }
    }

    fn previous_choice(&mut self) {
        match self.focus {
            GoalField::Date => self.date.step_date(-1),
            GoalField::Interest => self.eligible = !self.eligible,
            GoalField::Name | GoalField::Target => {}
        }
    }

    fn type_char(&mut self, c: char) {
        match self.focus {
            GoalField::Name => self.name.push(c),
            GoalField::Target => self.target.push(c),
            GoalField::Date => self.date.push(c),
            GoalField::Interest => {}
        }
    }

    fn backspace(&mut self) {
        match self.focus {
            GoalField::Name => self.name.backspace(),
            GoalField::Target => self.target.backspace(),
            GoalField::Date => self.date.backspace(),
            GoalField::Interest => {}
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
    date: Field,
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
            date: Field::date(today),
            destinations,
            destination: 0,
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Close out {} ({}) — ←/→ destination · Enter save · Esc cancel",
            self.goal_name, self.balance
        )
    }

    pub fn display(&self, field: CloseField) -> String {
        match field {
            CloseField::Date => self.date.value().to_string(),
            CloseField::Destination => self
                .destinations
                .get(self.destination)
                .map(|(_, label)| label.clone())
                .unwrap_or_default(),
        }
    }

    pub fn commit(&self) -> Result<CloseOut> {
        Ok(CloseOut {
            date: parse_date(self.date.value())?,
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

    // Guarded on focus, the same way `TxnForm` guards its account selector:
    // `←`/`→` step the date on one field and cycle the destination on the
    // other, and neither may reach across.
    fn next_choice(&mut self) {
        match self.focus {
            CloseField::Date => self.date.step_date(1),
            CloseField::Destination => {
                self.destination = step_index(self.destination, self.destinations.len(), 1)
            }
        }
    }

    fn previous_choice(&mut self) {
        match self.focus {
            CloseField::Date => self.date.step_date(-1),
            CloseField::Destination => {
                self.destination = step_index(self.destination, self.destinations.len(), -1)
            }
        }
    }

    fn type_char(&mut self, c: char) {
        if self.focus == CloseField::Date {
            self.date.push(c);
        }
    }

    fn backspace(&mut self) {
        if self.focus == CloseField::Date {
            self.date.backspace();
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
        .map(|cents| format!("= {}", cents.to_whole_dollars()))
        .unwrap_or_default();
    let mut lines: Vec<TextLine> = AllocField::ORDER
        .iter()
        .map(|f| {
            let note = if *f == AllocField::Amount {
                share.as_str()
            } else {
                ""
            };
            field_line_noted(f.label(), form.display(*f), form.focus == *f, note)
        })
        .collect();
    lines.push(TextLine::from(format!("  {}", form.unallocated_line())));
    render_fields(frame, form.title(), lines);
}

/// Draws the goal modal: `e`'s form, and `n`'s.
pub fn render_goal(frame: &mut Frame, form: &GoalForm) {
    let lines: Vec<TextLine> = GoalField::ORDER
        .iter()
        .map(|f| field_line(f.label(), form.display(*f), form.focus == *f))
        .collect();
    render_fields(frame, form.title(), lines);
}

/// Draws the close-out-a-goal modal: `c`'s form.
pub fn render_close(frame: &mut Frame, form: &CloseForm) {
    let lines: Vec<TextLine> = CloseField::ORDER
        .iter()
        .map(|f| field_line(f.label(), form.display(*f), form.focus == *f))
        .collect();
    render_fields(frame, form.title(), lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
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
            form.type_char(c);
        }
    }

    #[test]
    fn an_allocation_prefills_todays_date_and_commits_what_was_typed() {
        let mut form = alloc("Apple Watch");
        assert_eq!(form.display(AllocField::Date), "2026-08-16");

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
            form.backspace();
        }
        for c in "08/16/2026".chars() {
            form.type_char(c);
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

    fn typed_goal(form: &mut GoalForm, field: GoalField, text: &str) {
        while form.focus != field {
            form.next_field();
        }
        for c in text.chars() {
            form.type_char(c);
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
        );
        assert_eq!(form.display(GoalField::Name), "Couch");
        assert_eq!(form.display(GoalField::Target), "1,000.00");
        assert_eq!(form.display(GoalField::Date), "2026-12-01");

        typed_goal(&mut form, GoalField::Name, " Mk II");
        let edit = form.commit().unwrap();
        assert_eq!(edit.name, "Couch Mk II");
        assert_eq!(edit.goal_cents, Cents(100_000));
        assert_eq!(edit.goal_date, Some(day(2026, 12, 1)));
    }

    /// A goal's target is a whole dollar. The prefill is the stored figure
    /// with its cents, so a goal imported off a fractional cell shows what it
    /// really holds and has to be rounded by hand rather than silently moved
    /// the first time the form is opened.
    #[test]
    fn a_goal_target_with_cents_in_it_is_refused() {
        let mut form = GoalForm::new(GoalId(7), "Couch", Cents(100_050), None, true);
        assert_eq!(form.display(GoalField::Target), "1,000.50");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("1,000.50"), "{err}");

        while form.focus != GoalField::Target {
            form.next_field();
        }
        for _ in 0.."1,000.50".len() {
            form.backspace();
        }
        for c in "1000".chars() {
            form.type_char(c);
        }
        assert_eq!(form.commit().unwrap().goal_cents, Cents(100_000));
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
        );
        while form.focus != GoalField::Date {
            form.next_field();
        }
        for _ in 0..10 {
            form.backspace();
        }
        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    #[test]
    fn an_undated_goal_opens_with_an_empty_date_field() {
        let form = GoalForm::new(GoalId(7), "Bill Payments", Cents(1_500_000), None, true);
        assert_eq!(form.display(GoalField::Date), "");
        assert_eq!(form.commit().unwrap().goal_date, None);
    }

    /// Eligibility is the one field here that is not typed, so `←`/`→` have
    /// to reach it.
    #[test]
    fn the_choice_keys_flip_interest_eligibility() {
        let mut form = GoalForm::new(GoalId(7), "Down Payment", Cents(100_000), None, false);
        assert_eq!(form.display(GoalField::Interest), "no");

        while form.focus != GoalField::Interest {
            form.next_field();
        }
        form.next_choice();

        assert_eq!(form.display(GoalField::Interest), "yes");
        assert!(form.commit().unwrap().interest_eligible);
    }

    /// The importer marks the down-payment bucket ineligible, and opening its
    /// form must not quietly hand the flag back.
    #[test]
    fn an_ineligible_goal_commits_unchanged_as_ineligible() {
        let form = GoalForm::new(GoalId(7), "Down Payment", Cents(100_000), None, false);
        assert!(!form.commit().unwrap().interest_eligible);
    }

    /// A goal typed from scratch takes interest, like every goal the sheet
    /// ever had.
    #[test]
    fn a_goal_typed_from_scratch_opens_interest_eligible() {
        let mut form = GoalForm::add(AccountId(1), "Rainy Day");
        typed_goal(&mut form, GoalField::Name, "Couch");
        typed_goal(&mut form, GoalField::Target, "1000");
        assert_eq!(form.display(GoalField::Interest), "yes");
        assert!(form.commit().unwrap().interest_eligible);
    }

    /// The field is a selector, so a keystroke meant for a text field must
    /// not land in it -- nor anywhere else.
    #[test]
    fn typing_on_the_interest_field_changes_nothing() {
        let mut form = GoalForm::new(GoalId(7), "Couch", Cents(100_000), None, true);
        typed_goal(&mut form, GoalField::Interest, "no");

        assert_eq!(form.display(GoalField::Interest), "yes");
        assert_eq!(form.display(GoalField::Name), "Couch");
    }

    #[test]
    fn a_goal_with_an_empty_name_is_refused() {
        let mut form = GoalForm::new(GoalId(7), "Couch", Cents(100_000), None, true);
        while form.focus != GoalField::Name {
            form.next_field();
        }
        for _ in 0..10 {
            form.backspace();
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
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
        assert_eq!(form.display(CloseField::Destination), "— unallocated —");
        assert_eq!(form.commit().unwrap().to, None, "the default is abandon");

        while form.focus != CloseField::Destination {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(CloseField::Destination), "Rug");
        assert_eq!(form.commit().unwrap().to, Some(GoalId(8)));

        form.next_choice();
        assert_eq!(form.display(CloseField::Destination), "Lamp");
        form.next_choice();
        assert_eq!(
            form.display(CloseField::Destination),
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

    /// Every date field in the app steps a day at a time under `←`/`→`, and
    /// the allocation form's is the one field the arrows had nothing to do on.
    #[test]
    fn the_arrows_step_an_allocation_date_by_a_day() {
        let mut form = alloc("Apple Watch");
        form.next_choice();
        assert_eq!(form.display(AllocField::Date), "2026-08-17");
        form.previous_choice();
        form.previous_choice();
        assert_eq!(form.display(AllocField::Date), "2026-08-15");
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
            form.backspace();
        }
        for c in "2026-".chars() {
            form.type_char(c);
        }
        form.next_choice();
        assert_eq!(form.display(AllocField::Date), "2026-");
    }

    /// The amount is where `/N` lives and the note is free text: neither may
    /// move when the arrows are pressed off the date.
    #[test]
    fn the_arrows_do_nothing_away_from_the_allocation_date() {
        let mut form = alloc("Apple Watch");
        typed(&mut form, AllocField::Amount, "72");
        form.next_choice();
        form.previous_choice();
        assert_eq!(form.display(AllocField::Amount), "72");
        assert_eq!(form.display(AllocField::Date), "2026-08-16");
    }

    #[test]
    fn the_arrows_step_a_goal_date_by_a_day() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Couch",
            Cents(100_000),
            Some(day(2026, 12, 31)),
            true,
        );
        form.next_choice();
        assert_eq!(
            form.display(GoalField::Date),
            "2026-12-31",
            "the name field is focused, and the arrows must stay off the date"
        );

        while form.focus != GoalField::Date {
            form.next_field();
        }
        form.next_choice();
        assert_eq!(form.display(GoalField::Date), "2027-01-01");
    }

    /// An undated goal stays undated: there is no date to step, and seeding
    /// one would date a goal by pressing an arrow at it.
    #[test]
    fn the_arrows_leave_an_undated_goal_undated() {
        let mut form = GoalForm::new(GoalId(7), "Bill Payments", Cents(1_500_000), None, true);
        while form.focus != GoalField::Date {
            form.next_field();
        }
        form.next_choice();
        form.previous_choice();
        assert_eq!(form.display(GoalField::Date), "");
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
        form.previous_choice();
        assert_eq!(form.display(CloseField::Date), "2026-08-15");
        assert_eq!(
            form.display(CloseField::Destination),
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
        form.next_choice();
        assert_eq!(form.display(CloseField::Destination), "— unallocated —");
        assert_eq!(form.commit().unwrap().to, None);
    }
}
