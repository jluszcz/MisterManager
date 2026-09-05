//! The forms that act on one goal: `a` allocate, `e` edit, `n` create, `c`
//! close out.
//!
//! Same shape as `form.rs`'s two — plain state machines driven through
//! `FormFields`, with the parsing and validation unit-tested directly and the
//! render functions at the bottom drawing only.

use super::form::{
    DateField, Field, Focused, FormFields, Precision, Step, is_share, next_in, parse_share,
    parse_whole_amount, step_index, tax_note,
};
use super::widget::{field_stack, render_fields};
use super::{Account, Label};
use crate::db::goal::{Allocation, AllocationEdit, GoalEdit};
use crate::db::{AccountId, AllocationId, GoalId};
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

/// What committing an allocation form does, read off the same field the
/// border is -- so the write cannot land somewhere the form did not say it
/// would. The same construction [`GoalTarget`] makes for the goal form.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllocTarget {
    /// `a` on Savings: a row that does not exist yet, against the form's own
    /// `goal_id`.
    Insert,
    /// `e` in a goal's history: the row it opened on.
    Update(AllocationId),
}

/// One allocation against one goal. Backs `a` on Savings and `e` in a goal's
/// allocation history.
///
/// The amount is **signed**: a negative is a spend against the goal, which is
/// what the sheet's hand-accreted `Current` formulas already encode. It also
/// takes `/N` for a fraction of `unallocated`, which is why the form carries
/// the container's remainder at all.
///
/// A correction reads its typed amount at [`Precision::Cents`] where a new row
/// reads at [`Precision::WholeDollars`]: the rows most worth correcting are
/// the ones the import and the interest postings wrote, and those carry cents
/// the form has just prefilled. `/N` divides the same pot at either -- the
/// remainder as it stands now, never one the row being edited is folded back
/// into.
#[derive(Debug)]
pub struct AllocationForm {
    /// The goal the row belongs to, whether or not the row exists yet.
    pub goal_id: GoalId,
    target: AllocTarget,
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
            target: AllocTarget::Insert,
            // The amount is what the row is being written to say; the date is
            // prefilled and right nearly every time.
            focus: AllocField::Amount,
            goal_name: goal_name.to_string(),
            container_name: container_name.to_string(),
            unallocated,
            date: DateField::today(today),
            amount: Field::default(),
            note: Field::default(),
        }
    }

    /// The same form opened on a row that already exists, prefilled from it.
    ///
    /// Every field arrives through `Field::given`, so it counts as the
    /// owner's own text: these are figures already on screen that nobody
    /// asked to have rewritten.
    pub fn edit(
        row: &Allocation,
        goal_name: &str,
        container_name: &str,
        unallocated: Cents,
        today: NaiveDate,
    ) -> AllocationForm {
        AllocationForm {
            goal_id: row.goal_id,
            target: AllocTarget::Update(row.id),
            focus: AllocField::Amount,
            goal_name: goal_name.to_string(),
            container_name: container_name.to_string(),
            unallocated,
            date: DateField::given(today, Some(row.date)),
            amount: Field::given(row.cents.to_string()),
            note: Field::given(row.note.clone().unwrap_or_default()),
        }
    }

    pub fn target(&self) -> AllocTarget {
        self.target
    }

    /// How a typed figure is read, which is the one thing the two subjects
    /// disagree about.
    fn precision(&self) -> Precision {
        match self.target {
            AllocTarget::Insert => Precision::WholeDollars,
            AllocTarget::Update(_) => Precision::Cents,
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
            .then(|| parse_share(raw, self.unallocated, self.precision()).ok())
            .flatten()
    }

    /// The line under the fields: the pot `/N` divides, and the key that
    /// divides it.
    ///
    /// The form carries both because nothing else here can. The remainder is
    /// on the Savings screen behind this modal, and `/N` has no room in the
    /// help table `Topic::Form` shares with the forms that do not offer it.
    ///
    /// Full precision, where the Savings footer behind this modal shows whole
    /// dollars. The two answer different questions: the footer reports
    /// whether there is a remainder worth placing by hand, and this line
    /// states what `/N` is about to divide -- which is the untruncated excess
    /// `App::open_allocate` hands across. So a container sitting at `0.23`
    /// reads `0` on the footer and `0.23` here, and the key still divides the
    /// cents it would otherwise leave stranded.
    pub fn unallocated_line(&self) -> String {
        format!(
            "{} unallocated {} · /N takes 1/N",
            crate::demo::text(&self.container_name),
            crate::demo::figure(self.unallocated)
        )
    }

    pub fn title(&self) -> String {
        let verb = match self.target {
            AllocTarget::Insert => "Allocate to",
            AllocTarget::Update(_) => "Edit allocation to",
        };
        format!(
            "{verb} {} — Tab field · Enter save · Esc cancel",
            crate::demo::text(&self.goal_name)
        )
    }

    pub fn display(&self, field: AllocField) -> Label {
        Label::plain(match field {
            AllocField::Date => self.date.display(self.focus == AllocField::Date),
            // The one amount field that can be holding something other than an
            // amount. `/12` is a count, and counts are not scrambled: what it
            // divides is the unallocated remainder on the line below, which is
            // scrambled there, and `resolved_share` puts the answer beside
            // the field scrambled too. Scrambling the divisor as well would
            // leave the one field whose text is not a figure with no visible
            // feedback at all -- `/12` and `/2` would read the same right up
            // to Enter.
            AllocField::Amount => match is_share(self.amount.value()) {
                true => self.amount.value().to_string(),
                false => crate::demo::typed(self.amount.value()),
            },
            // Prefilled from the stored row by `edit`, so it is owner text the
            // same as a transaction's description -- and the history behind
            // this modal draws the same note through `description::render`.
            AllocField::Note => crate::demo::text(self.note.value()).into_owned(),
        })
    }

    /// The three columns, ready for `goal::insert_allocation` or
    /// `goal::update_allocation` -- one type rather than a form's own copy of
    /// the same three fields, so a correction writes back exactly what the
    /// history read out.
    pub fn commit(&self) -> Result<AllocationEdit> {
        let note = self.note.value().trim().to_string();
        Ok(AllocationEdit {
            date: self.date.parse()?,
            cents: parse_share(self.amount.value(), self.unallocated, self.precision())?,
            // An empty note is no note, not a note that says nothing.
            note: (!note.is_empty()).then_some(note),
        })
    }
}

impl FormFields for AllocationForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&AllocField::ORDER, self.focus, step);
    }

    // No selector: `←`/`→` step the date and move the caret in the other two.
    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            AllocField::Date => Focused::Date(&mut self.date),
            AllocField::Amount => Focused::Text(&mut self.amount),
            AllocField::Note => Focused::Text(&mut self.note),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalField {
    Name,
    Target,
    Date,
    /// Beside `Taxed`, because the two say what the Target above them means,
    /// and after the Date, so that the rhythm of typing a goal that has a
    /// target -- name, target, date -- is the one it has always been.
    Floating,
    Taxed,
    Interest,
}

impl GoalField {
    /// Every field, which is what a goal funded towards a figure offers.
    /// [`GoalForm::fields`] is what a form actually walks.
    pub const ORDER: [GoalField; 6] = [
        GoalField::Name,
        GoalField::Target,
        GoalField::Date,
        GoalField::Floating,
        GoalField::Taxed,
        GoalField::Interest,
    ];

    /// The same list without the two fields that describe a fixed target.
    const FLOATING: [GoalField; 4] = [
        GoalField::Name,
        GoalField::Date,
        GoalField::Floating,
        GoalField::Interest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GoalField::Name => "Name",
            GoalField::Floating => "Floating",
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
    /// The base the form opened on, which is what a suspended Target falls
    /// back to. Kept because the field cannot always be read back: an
    /// imported base carries the sheet's cents, and `parse_whole_amount`
    /// refuses those.
    opened_base: Cents,
    taxed: bool,
    floating: bool,
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
            // A goal that does not exist yet was funded towards nothing.
            opened_base: Cents::ZERO,
            taxed: false,
            floating: false,
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
        floating: bool,
        rate: Option<BasisPoints>,
        today: NaiveDate,
    ) -> GoalForm {
        GoalForm {
            subject: Subject::Existing(goal_id),
            focus: GoalField::Name,
            name: Field::given(name),
            // The base, which is what the table holds. What it comes to is
            // beside it, in `tax_note`. Prefilled with its cents rather than
            // rounded: a goal imported off a fractional cell has to show what
            // it really holds, so that the owner rounds it by hand instead of
            // the form moving it the first time it is opened.
            target: Field::given(base.to_string()),
            date: DateField::given(today, date),
            opened_base: base,
            taxed,
            floating,
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
            GoalField::Name => crate::demo::text(self.name.value()).into_owned(),
            GoalField::Target => crate::demo::typed(self.target.value()),
            GoalField::Date => self.date.display(self.focus == GoalField::Date),
            GoalField::Floating => if self.floating { "yes" } else { "no" }.to_string(),
            GoalField::Taxed => if self.taxed { "yes" } else { "no" }.to_string(),
            GoalField::Interest => if self.eligible { "yes" } else { "no" }.to_string(),
        })
    }

    /// The fields this form offers, which is what Tab walks and what
    /// `render_goal` draws.
    ///
    /// A floating goal is funded to whatever it holds, so the Target and the
    /// Taxed flag describe nothing: both come off the form rather than
    /// sitting there inviting a figure that would never be read. What they
    /// already hold is kept -- see [`GoalForm::commit`] -- so turning the
    /// flag back off restores the goal the form opened on.
    pub fn fields(&self) -> &'static [GoalField] {
        if self.floating {
            &GoalField::FLOATING
        } else {
            &GoalField::ORDER
        }
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
        // Both halves of a fixed target are suspended while `Floating` is on
        // rather than erased: the fields are unreachable, so what they hold
        // is what the goal was funded towards, and an edit made for the name
        // must not spend it. The field is not always readable back -- an
        // imported base carries the sheet's cents, which `parse_whole_amount`
        // refuses -- so an unparseable one falls back to the base the form
        // opened on rather than to zero, which would erase it.
        let base_cents = match parse_whole_amount(self.target.value()) {
            Ok(base) => base,
            Err(_) if self.floating => self.opened_base,
            Err(e) => return Err(e),
        };
        // Refused even though nothing here needs the rate any more: letting it
        // through would write precisely the row the read side calls corrupt,
        // and this is the one place that can ask for the rate before there is
        // a goal to be broken by its absence. A floating goal spends no rate,
        // for the reason `crate::goal::target` reads the flag first.
        if self.taxed && !self.floating {
            self.rate.context(NO_TAX_RATE)?;
        }
        Ok(GoalEdit {
            name,
            base_cents,
            // An empty date field is an undated goal -- rows 6-26 of the sheet.
            goal_date: self.date.parse_opt()?,
            interest_eligible: self.eligible,
            taxed: self.taxed,
            floating: self.floating,
        })
    }
}

impl FormFields for GoalForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(self.fields(), self.focus, step);
    }

    // Both selectors here hold two values, so both directions are the same
    // flip. An undated goal has no date to step, which is what keeps an arrow
    // press from dating one.
    fn cycle(&mut self, _step: Step) {
        match self.focus {
            GoalField::Taxed => self.taxed = !self.taxed,
            GoalField::Floating => self.floating = !self.floating,
            GoalField::Interest => self.eligible = !self.eligible,
            GoalField::Name | GoalField::Target | GoalField::Date => {}
        }
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            GoalField::Name => Focused::Text(&mut self.name),
            GoalField::Target => Focused::Text(&mut self.target),
            GoalField::Date => Focused::Date(&mut self.date),
            GoalField::Taxed | GoalField::Floating | GoalField::Interest => Focused::Selector,
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
            // The destination is the decision this form exists to make; the
            // date is almost always today.
            focus: CloseField::Destination,
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
            crate::demo::text(&self.goal_name),
            crate::demo::figure(self.balance)
        )
    }

    pub fn display(&self, field: CloseField) -> Label {
        Label::plain(match field {
            CloseField::Date => self.date.display(self.focus == CloseField::Date),
            // The first destination is always `None` -- "— unallocated —",
            // the app's own words, never masked -- and every other one names
            // a sibling goal.
            CloseField::Destination => self
                .destinations
                .get(self.destination)
                .map(|(id, label)| match id {
                    Some(_) => crate::demo::text(label).into_owned(),
                    None => label.clone(),
                })
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
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&CloseField::ORDER, self.focus, step);
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
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GoalTransferField {
    Date,
    Amount,
    Destination,
}

impl GoalTransferField {
    pub const ORDER: [GoalTransferField; 3] = [
        GoalTransferField::Date,
        GoalTransferField::Amount,
        GoalTransferField::Destination,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GoalTransferField::Date => "Date",
            GoalTransferField::Amount => "Amount",
            GoalTransferField::Destination => "To",
        }
    }
}

/// A committed transfer, ready for `goal::transfer_value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalTransfer {
    pub date: NaiveDate,
    pub cents: Cents,
    pub to: GoalId,
}

/// Moving part of a goal's value to another goal. Backs `t` on Savings.
///
/// A close-out that ends nothing, so it differs from [`CloseForm`] in exactly
/// the two ways the ending does: the amount is typed rather than being the
/// whole balance, and the destination is a goal rather than a goal *or*
/// unallocated. Returning value to unallocated is `a` with a negative amount;
/// a second spelling of it here would be a second spelling of an ending.
///
/// Named apart from `form::TransferForm`, which is the cash transfer `t`
/// opens on the ledgers: one moves money between accounts and the other moves
/// none at all.
#[derive(Debug)]
pub struct GoalTransferForm {
    pub goal_id: GoalId,
    pub focus: GoalTransferField,
    goal_name: String,
    balance: Cents,
    date: DateField,
    amount: Field,
    /// Never empty: `App::open_goal_transfer` refuses to open the form over a
    /// container whose only open goal is the one the cursor is on.
    destinations: Vec<(GoalId, String)>,
    destination: usize,
}

impl GoalTransferForm {
    /// `siblings` are the *open goals of the same container*, minus this one.
    /// Crossing containers would break both reconciliations at once, since no
    /// cash moved between the accounts.
    pub fn new(
        goal_id: GoalId,
        goal_name: &str,
        balance: Cents,
        siblings: Vec<(GoalId, String)>,
        today: NaiveDate,
    ) -> GoalTransferForm {
        GoalTransferForm {
            goal_id,
            // The amount is the decision, the way it is on the allocation
            // form: the date is almost always today, and the destination is
            // one arrow away.
            focus: GoalTransferField::Amount,
            goal_name: goal_name.to_string(),
            balance,
            date: DateField::today(today),
            amount: Field::default(),
            destinations: siblings,
            destination: 0,
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Move value out of {} ({}) — ←/→ destination · Enter save · Esc cancel",
            crate::demo::text(&self.goal_name),
            crate::demo::figure(self.balance)
        )
    }

    pub fn display(&self, field: GoalTransferField) -> Label {
        Label::plain(match field {
            GoalTransferField::Date => self.date.display(self.focus == GoalTransferField::Date),
            GoalTransferField::Amount => crate::demo::typed(self.amount.value()),
            GoalTransferField::Destination => self
                .destinations
                .get(self.destination)
                .map(|(_, name)| crate::demo::text(name).into_owned())
                .unwrap_or_default(),
        })
    }

    pub fn commit(&self) -> Result<GoalTransfer> {
        let (to, _) = self
            .destinations
            .get(self.destination)
            .context("a transfer form opens only over a container with another open goal")?;
        Ok(GoalTransfer {
            date: self.date.parse()?,
            cents: parse_whole_amount(self.amount.value())?,
            to: *to,
        })
    }
}

impl FormFields for GoalTransferForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&GoalTransferField::ORDER, self.focus, step);
    }

    // `←`/`→` step the date on one field and cycle the destination on
    // another, and neither may reach across.
    fn cycle(&mut self, step: Step) {
        self.destination = step_index(self.destination, self.destinations.len(), step.direction());
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            GoalTransferField::Date => Focused::Date(&mut self.date),
            GoalTransferField::Amount => Focused::Text(&mut self.amount),
            GoalTransferField::Destination => Focused::Selector,
        }
    }
}

use ratatui::Frame;
use ratatui::text::Line as TextLine;

/// Draws the allocate-against-a-goal modal: `a`'s form.
///
/// One line taller than the other two forms here, for the pot `/N` divides.
pub fn render_allocation(frame: &mut Frame, form: &mut AllocationForm) {
    let share = form
        .resolved_share()
        .map(|cents| format!("= {}", crate::demo::whole_figure(cents)))
        .unwrap_or_default();
    let caret = form.caret();
    let mut lines = field_stack(
        &AllocField::ORDER,
        form.focus,
        caret,
        AllocField::label,
        |f| form.display(f),
        &[(AllocField::Amount, share.as_str())],
    );
    lines.push(TextLine::from(format!("  {}", form.unallocated_line())));
    render_fields(frame, form.title(), lines);
}

/// Draws the goal modal: `e`'s form, and `n`'s.
pub fn render_goal(frame: &mut Frame, form: &mut GoalForm) {
    let note = form.tax_note();
    let caret = form.caret();
    let lines = field_stack(
        form.fields(),
        form.focus,
        caret,
        GoalField::label,
        |f| form.display(f),
        // The Target field holds the base rather than what the goal is funded
        // to, so the note says what it comes to beside the figure it is
        // derived from rather than beside itself.
        &[(GoalField::Target, note.as_str())],
    );
    render_fields(frame, form.title(), lines);
}

/// Draws the move-value-to-another-goal modal: `t`'s form.
pub fn render_goal_transfer(frame: &mut Frame, form: &mut GoalTransferForm) {
    let caret = form.caret();
    let lines = field_stack(
        &GoalTransferField::ORDER,
        form.focus,
        caret,
        GoalTransferField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(frame, form.title(), lines);
}

/// Draws the close-out-a-goal modal: `c`'s form.
pub fn render_close(frame: &mut Frame, form: &mut CloseForm) {
    let caret = form.caret();
    let lines = field_stack(
        &CloseField::ORDER,
        form.focus,
        caret,
        CloseField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(frame, form.title(), lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self};
    use crate::test_support::{cash, day, walk_until};
    use crate::tui::form::{backspace_key, char_key};

    fn today() -> NaiveDate {
        day(2026, 8, 21)
    }

    fn accounts() -> Vec<account::Account> {
        vec![cash(1, "SAV"), cash(2, "NST")]
    }

    /// The container and its remainder matter only to `/N`; every test that
    /// types a figure gets a name and an empty pot.
    fn alloc(name: &str) -> AllocationForm {
        AllocationForm::new(GoalId(7), name, "Rainy Day", Cents::ZERO, day(2026, 8, 16))
    }

    fn typed(form: &mut AllocationForm, field: AllocField, text: &str) {
        walk_until!(form.focus == field, form.next_field());
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    /// The date is prefilled and the amount is what the row is being written
    /// to say, so a run of `a` costs no `Tab` before the first digit. A
    /// correction opens there too: every field of it is prefilled, and the
    /// amount is what a correction is about.
    #[test]
    fn an_allocation_opens_focused_on_its_amount_whichever_subject_it_has() {
        assert_eq!(alloc("Apple Watch").focus, AllocField::Amount);

        let row = Allocation {
            id: AllocationId(3),
            goal_id: GoalId(7),
            date: day(2026, 8, 16),
            cents: Cents(12_300),
            note: None,
        };
        let form = AllocationForm::edit(&row, "Apple Watch", "Rainy Day", Cents::ZERO, today());
        assert_eq!(form.focus, AllocField::Amount);
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
        walk_until!(form.focus == AllocField::Date, form.next_field());
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
    /// demo scrambles both. The divisor itself is a count and stays, which is
    /// what leaves the field feedback: scrambled, `/12` and `/2` would read
    /// the same right up to Enter, and the answer beside them is scrambled
    /// already.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_share_and_the_remainder_but_not_the_divisor() {
        crate::demo::install_with_salt(7);
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "/12");
        let text = rendered(&mut form);

        assert!(!text.contains("216"), "the resolved share survived: {text}");
        assert!(!text.contains("2,600"), "the remainder survived: {text}");
        assert!(text.contains("/12"), "the divisor is a count: {text}");
        assert!(
            text.contains(&crate::demo::whole_figure(Cents::from_dollars(216))),
            "no scrambled share found: {text}"
        );
        assert!(
            text.contains(&crate::demo::figure(Cents(260_017))),
            "no scrambled remainder found: {text}"
        );
        assert!(!text.contains("Lego"), "the goal name survived: {text}");
        assert!(
            text.contains(&crate::demo::text("Lego").to_string()),
            "no scrambled goal name found: {text}"
        );
        assert!(text.contains("2026-08-16"), "the date must stay: {text}");
    }

    /// The other half of the same field. A typed figure *is* a figure, and
    /// the form opens prefilled on an edit -- a field showing what is already
    /// there publishes it to whoever is watching.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_an_amount_typed_into_the_same_field() {
        crate::demo::install_with_salt(7);
        let mut form = AllocationForm::new(
            GoalId(7),
            "Lego",
            "Rainy Day",
            Cents(260_017),
            day(2026, 8, 16),
        );
        typed(&mut form, AllocField::Amount, "1234");
        let text = rendered(&mut form);

        assert!(!text.contains("1234"), "the typed amount survived: {text}");
        assert!(
            text.contains(&crate::demo::typed("1234")),
            "no scrambled amount found: {text}"
        );
    }

    /// The note is owner text, and an edit prefills it from the stored row --
    /// so the field publishes a note the history behind this modal is already
    /// masking, which is the one place the two could disagree.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_note_prefilled_from_the_row_being_corrected() {
        crate::demo::install_with_salt(7);
        let row = Allocation {
            id: AllocationId(3),
            goal_id: GoalId(7),
            date: day(2026, 8, 16),
            cents: Cents(12_300),
            note: Some("birthday money".to_string()),
        };
        let mut form = AllocationForm::edit(&row, "Lego", "Rainy Day", Cents::ZERO, today());
        let text = rendered(&mut form);

        assert!(
            !text.contains("birthday money"),
            "the note survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("birthday money").to_string()),
            "no scrambled note found: {text}"
        );
    }

    #[cfg(feature = "demo")]
    fn rendered(form: &mut AllocationForm) -> String {
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
            false,
            Some(BasisPoints(625)),
            today(),
        )
    }

    fn typed_goal(form: &mut GoalForm, field: GoalField, text: &str) {
        walk_until!(form.focus == field, form.next_field());
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
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Target).plain_text(), "1,000.50");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("1,000.50"), "{err}");

        walk_until!(form.focus == GoalField::Target, form.next_field());
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
            false,
            None,
            today(),
        );
        walk_until!(form.focus == GoalField::Date, form.next_field());
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
            false,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Interest).plain_text(), "no");

        walk_until!(form.focus == GoalField::Interest, form.next_field());
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
        walk_until!(form.focus == GoalField::Date, form.next_field());
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
            false,
            None,
            today(),
        );
        walk_until!(form.focus == GoalField::Name, form.next_field());
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
        walk_until!(form.focus == GoalField::Taxed, form.next_field());
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
            false,
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

        walk_until!(form.focus == GoalField::Taxed, form.next_field());
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
        walk_until!(form.focus == GoalField::Target, form.next_field());
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
            false,
            None,
            today(),
        );
        walk_until!(form.focus == GoalField::Taxed, form.next_field());
        form.choice(Step::NEXT);

        assert_eq!(form.tax_note(), "");
        let err = form.commit().unwrap_err().to_string();
        assert!(err.contains("tax rate"), "{err}");
    }

    /// The second selector on the form, and it cycles like the first.
    #[test]
    fn the_choice_keys_flip_the_tax_selector_both_ways() {
        let mut form = taxable("Couch", Cents(100_000));
        walk_until!(form.focus == GoalField::Taxed, form.next_field());
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

    /// The fields the form actually offers, in order, read off the walk
    /// rather than off `fields()` -- what a test wants to know is what Tab
    /// reaches.
    fn walk(form: &mut GoalForm) -> Vec<GoalField> {
        let first = form.focus;
        let mut seen = vec![first];
        loop {
            form.next_field();
            if form.focus == first {
                return seen;
            }
            seen.push(form.focus);
            assert!(
                seen.len() <= GoalField::ORDER.len(),
                "{seen:?} does not close"
            );
        }
    }

    /// A floating goal has no target and nothing to tax, so the two fields
    /// that describe one come off the form rather than sitting there
    /// unreachable-looking but typeable.
    #[test]
    fn turning_a_goal_floating_takes_the_target_and_tax_fields_off_the_form() {
        let mut form = taxable("Brokerage", Cents(100_000));
        assert_eq!(
            walk(&mut form),
            vec![
                GoalField::Name,
                GoalField::Target,
                GoalField::Date,
                GoalField::Floating,
                GoalField::Taxed,
                GoalField::Interest
            ]
        );

        walk_until!(form.focus == GoalField::Floating, form.next_field());
        form.choice(Step::NEXT);
        form.next_field();

        assert_eq!(
            walk(&mut form),
            vec![
                GoalField::Interest,
                GoalField::Name,
                GoalField::Date,
                GoalField::Floating
            ],
            "Tab walks past the target and the tax flag"
        );
    }

    /// The base is suspended rather than erased: the Target field is
    /// unreachable while the flag is on, so what it holds is what the goal
    /// was funded towards, and turning the flag back off has to find it
    /// still there.
    #[test]
    fn a_floating_goal_commits_the_flag_and_keeps_the_base_it_opened_with() {
        let mut form = taxable("Brokerage", Cents(100_000));
        walk_until!(form.focus == GoalField::Floating, form.next_field());
        form.choice(Step::NEXT);

        let edit = form.commit().unwrap();
        assert!(edit.floating);
        assert_eq!(edit.base_cents, Cents(100_000));
    }

    /// The Target field is unreachable while the flag is on, so what it holds
    /// is a figure the form cannot always read back: an imported goal's base
    /// carries whatever cents the sheet had, and `parse_whole_amount` refuses
    /// those on purpose. Falling back to zero there would erase the figure the
    /// goal was funded towards on an edit that never touched it.
    #[test]
    fn a_floating_goal_keeps_a_base_the_target_field_cannot_parse() {
        let mut form = taxable("Brokerage", Cents(100_050));
        assert_eq!(form.display(GoalField::Target).plain_text(), "1,000.50");
        assert!(
            form.commit().is_err(),
            "the field is refused while it is still reachable"
        );

        walk_until!(form.focus == GoalField::Floating, form.next_field());
        form.choice(Step::NEXT);

        assert_eq!(form.commit().unwrap().base_cents, Cents(100_050));
    }

    /// The one unparseable Target a floating goal can reach: a goal typed
    /// from scratch, where nothing has been typed into a field the form no
    /// longer offers. Zero is what an empty field means, and refusing the
    /// commit over it would make the flag unusable on `n`.
    #[test]
    fn a_floating_goal_typed_from_scratch_commits_a_zero_base() {
        let mut form = GoalForm::add(
            Account::named(&accounts(), AccountId(1)),
            Some(BasisPoints(625)),
            today(),
        );
        typed_goal(&mut form, GoalField::Name, "Brokerage");
        walk_until!(form.focus == GoalField::Floating, form.next_field());
        form.choice(Step::NEXT);

        let edit = form.commit().unwrap();
        assert!(edit.floating);
        assert_eq!(edit.base_cents, Cents::ZERO);
    }

    /// Nothing about a floating goal spends the sales tax rate, so the
    /// refusal that guards a stored base does not fire: the flag beside it is
    /// suspended, not spent, and `crate::goal::target` reads floating first
    /// for the same reason.
    #[test]
    fn a_floating_goal_is_not_refused_for_want_of_a_tax_rate() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Brokerage",
            Cents(100_000),
            None,
            true,
            true,
            false,
            None,
            today(),
        );
        assert!(form.commit().is_err(), "taxed with no rate is refused");

        walk_until!(form.focus == GoalField::Floating, form.next_field());
        form.choice(Step::NEXT);

        let edit = form.commit().unwrap();
        assert!(edit.floating);
        assert!(edit.taxed, "the flag is suspended, not cleared");
    }

    /// Opening a floating goal with the selector off would fix it to the base
    /// it has been ignoring, on an edit made for the name.
    #[test]
    fn a_form_opened_on_a_floating_goal_opens_with_the_selector_on() {
        let mut form = GoalForm::new(
            GoalId(7),
            "Brokerage",
            Cents(100_000),
            None,
            true,
            false,
            true,
            None,
            today(),
        );
        assert_eq!(form.display(GoalField::Floating).plain_text(), "yes");

        typed_goal(&mut form, GoalField::Name, "!");
        assert!(form.commit().unwrap().floating);
    }

    /// Like every other flag on the form: nothing has told it otherwise yet.
    #[test]
    fn a_goal_typed_from_scratch_does_not_float() {
        let form = GoalForm::add(
            Account::named(&accounts(), AccountId(1)),
            Some(BasisPoints(625)),
            today(),
        );
        assert_eq!(form.display(GoalField::Floating).plain_text(), "no");
    }

    fn siblings() -> Vec<(GoalId, String)> {
        vec![
            (GoalId(8), "Rug".to_string()),
            (GoalId(9), "Lamp".to_string()),
        ]
    }

    /// The destination is what the form is for; the date is almost always
    /// today. Opening on `To` is what keeps a close-out to `c` `→` `Enter`.
    #[test]
    fn a_close_out_opens_focused_on_its_destination() {
        let form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        assert_eq!(form.focus, CloseField::Destination);
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

        walk_until!(form.focus == CloseField::Destination, form.next_field());
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
    /// move -- and the goal's own name sits right beside it, so a demo has
    /// to hide both.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_balance_a_close_out_is_about_to_move() {
        crate::demo::install_with_salt(7);
        let form = CloseForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        );
        assert!(!form.title().contains("600.00"), "{}", form.title());
        assert!(
            form.title().contains(&crate::demo::figure(Cents(60_000))),
            "no scrambled balance found: {}",
            form.title()
        );
        assert!(!form.title().contains("Couch"), "{}", form.title());
        assert!(
            form.title()
                .contains(&crate::demo::text("Couch").to_string()),
            "no scrambled goal name found: {}",
            form.title()
        );
    }

    /// The destination selector names a sibling goal to close into, and a
    /// demo hides that name too -- but "— unallocated —" is the app's own
    /// word for the default, never the owner's, and stays exactly as typed.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_close_outs_sibling_destinations_but_not_unallocated() {
        crate::demo::install_with_salt(7);
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

        walk_until!(form.focus == CloseField::Destination, form.next_field());
        form.choice(Step::NEXT);
        let drawn = form.display(CloseField::Destination).plain_text();
        assert_ne!(drawn, "Rug");
        assert_eq!(drawn, crate::demo::text("Rug"));
        // The buffer is untouched: the id, not the name, is what commits.
        assert_eq!(form.commit().unwrap().to, Some(GoalId(8)));
    }

    fn transfer() -> GoalTransferForm {
        GoalTransferForm::new(
            GoalId(7),
            "Couch",
            Cents(60_000),
            siblings(),
            day(2026, 8, 16),
        )
    }

    fn typed_transfer(form: &mut GoalTransferForm, field: GoalTransferField, text: &str) {
        walk_until!(form.focus == field, form.next_field());
        for c in text.chars() {
            form.edit(char_key(c));
        }
    }

    /// The amount is the decision here, the way it is on the allocation form
    /// -- unlike a close-out, where the amount is the whole balance and the
    /// destination is all there is to choose.
    #[test]
    fn a_goal_transfer_opens_on_the_amount() {
        assert_eq!(transfer().focus, GoalTransferField::Amount);
    }

    /// Whole dollars, the reading `a` takes: this is an amount typed by hand,
    /// and cents typed into it are a typo rather than arithmetic.
    #[test]
    fn a_goal_transfer_reads_its_amount_in_whole_dollars() {
        let mut form = transfer();
        typed_transfer(&mut form, GoalTransferField::Amount, "250");
        assert_eq!(form.commit().unwrap().cents, Cents(25_000));

        let mut form = transfer();
        typed_transfer(&mut form, GoalTransferField::Amount, "250.50");
        assert!(form.commit().is_err(), "cents are a typo in a whole field");
    }

    /// A transfer needs somewhere for the value to land, so the selector
    /// holds only goals -- returning value to unallocated is what `a` with a
    /// negative amount is for, and offering it here would be a second way to
    /// spell an ending.
    #[test]
    fn a_goal_transfer_cycles_through_the_containers_other_goals_and_nothing_else() {
        let mut form = transfer();
        typed_transfer(&mut form, GoalTransferField::Amount, "250");
        assert_eq!(
            form.display(GoalTransferField::Destination).plain_text(),
            "Rug"
        );
        assert_eq!(form.commit().unwrap().to, GoalId(8));

        walk_until!(
            form.focus == GoalTransferField::Destination,
            form.next_field()
        );
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(GoalTransferField::Destination).plain_text(),
            "Lamp"
        );
        assert_eq!(form.commit().unwrap().to, GoalId(9));

        form.choice(Step::NEXT);
        assert_eq!(
            form.display(GoalTransferField::Destination).plain_text(),
            "Rug",
            "the cycle wraps through goals alone"
        );
    }

    /// The balance is not what moves here -- the typed amount is -- so the
    /// title is where it is read, and it bounds nothing: an overspent goal is
    /// a real state, and `transfer_value` lets one through the way
    /// `move_value` already does.
    #[test]
    fn a_goal_transfer_names_the_goal_the_value_is_leaving_and_what_it_holds() {
        let form = transfer();
        assert!(form.title().contains("Couch"), "{}", form.title());
        assert!(form.title().contains("600.00"), "{}", form.title());
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_balance_and_the_names_a_goal_transfer_draws() {
        crate::demo::install_with_salt(7);
        let mut form = transfer();
        assert!(!form.title().contains("600.00"), "{}", form.title());
        assert!(!form.title().contains("Couch"), "{}", form.title());
        assert!(
            form.title().contains(&crate::demo::figure(Cents(60_000))),
            "no scrambled balance found: {}",
            form.title()
        );

        let drawn = form.display(GoalTransferField::Destination).plain_text();
        assert_ne!(drawn, "Rug");
        assert_eq!(drawn, crate::demo::text("Rug"));
        // The buffer is untouched: the id, not the name, is what commits.
        typed_transfer(&mut form, GoalTransferField::Amount, "250");
        assert_eq!(form.commit().unwrap().to, GoalId(8));
    }

    /// Every date field in the app steps a day at a time under `←`/`→`, and
    /// the allocation form's is the one field the arrows had nothing to do on.
    #[test]
    fn the_arrows_step_an_allocation_date_by_a_day() {
        let mut form = alloc("Apple Watch");
        walk_until!(form.focus == AllocField::Date, form.next_field());
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
        walk_until!(form.focus == AllocField::Date, form.next_field());
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

        walk_until!(form.focus == GoalField::Date, form.next_field());
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
            false,
            None,
            today(),
        );
        walk_until!(form.focus == GoalField::Date, form.next_field());
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
        walk_until!(form.focus == CloseField::Date, form.next_field());
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
        walk_until!(form.focus == CloseField::Destination, form.next_field());
        form.choice(Step::NEXT);
        assert_eq!(
            form.display(CloseField::Destination).plain_text(),
            "— unallocated —"
        );
        assert_eq!(form.commit().unwrap().to, None);
    }
}
