//! The bill modal: adding or editing one row of `Planning!C6:E12`.

use crate::db::BillId;
use crate::db::bill::{self, Bill};
use crate::tui::Label;
use crate::tui::form::{Field, Focused, FormFields, Step, next_in, parse_amount, step_index};
use crate::tui::widget::{field_stack, render_fields};
use anyhow::{Result, ensure};
use ratatui::Frame;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BillField {
    Label,
    Amount,
    Category,
}

impl BillField {
    pub const ORDER: [BillField; 3] = [BillField::Label, BillField::Amount, BillField::Category];

    pub fn label(self) -> &'static str {
        match self {
            BillField::Label => "Label",
            BillField::Amount => "Monthly",
            BillField::Category => "Category",
        }
    }
}

/// Adding or editing one bill. Backs `a` and `E`.
///
/// The category is a selector over `Category::ALL` rather than a text field,
/// so a category the schema's `CHECK` would refuse is unrepresentable.
#[derive(Debug)]
pub struct BillForm {
    /// `Some` when editing an existing bill, `None` when adding one.
    pub editing: Option<BillId>,
    pub focus: BillField,
    label: Field,
    amount: Field,
    category: usize,
}

impl BillForm {
    pub fn add() -> BillForm {
        BillForm {
            editing: None,
            focus: BillField::Label,
            label: Field::default(),
            amount: Field::default(),
            category: 0,
        }
    }

    pub fn edit(bill: &Bill) -> BillForm {
        BillForm {
            editing: Some(bill.id),
            focus: BillField::Label,
            label: Field::given(bill.label.clone()),
            amount: Field::given(bill.cents.to_string()),
            category: bill::Category::ALL
                .iter()
                .position(|c| *c == bill.category)
                .unwrap_or(0),
        }
    }

    pub fn category(&self) -> bill::Category {
        bill::Category::ALL[self.category]
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => "Edit bill — Tab field · ←/→ category · Enter save · Esc cancel",
            None => "Add bill — Tab field · ←/→ category · Enter save · Esc cancel",
        }
    }

    pub fn display(&self, field: BillField) -> Label {
        Label::plain(match field {
            BillField::Label => crate::demo::text(self.label.value()).into_owned(),
            BillField::Amount => crate::demo::typed(self.amount.value()),
            BillField::Category => self.category().as_str().to_string(),
        })
    }

    pub fn commit(&self) -> Result<bill::BillEdit> {
        let label = self.label.value().trim().to_string();
        ensure!(!label.is_empty(), "a bill's label must not be empty");
        Ok(bill::BillEdit {
            label,
            cents: parse_amount(self.amount.value())?,
            category: self.category(),
        })
    }
}

impl FormFields for BillForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&BillField::ORDER, self.focus, step);
    }

    fn cycle(&mut self, step: Step) {
        self.category = step_index(self.category, bill::Category::ALL.len(), step.direction());
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            BillField::Label => Focused::Text(&mut self.label),
            BillField::Amount => Focused::Text(&mut self.amount),
            BillField::Category => Focused::Selector,
        }
    }
}

pub fn render_bill(frame: &mut Frame, form: &mut BillForm) {
    let caret = form.caret();
    let lines = field_stack(
        &BillField::ORDER,
        form.focus,
        caret,
        BillField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(frame, form.title(), lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::bill::Category;

    use crate::db::BillId;
    use crate::money::Cents;

    use crate::tui::form::char_key;

    use crate::tui::planning::test_support::*;

    #[test]
    fn a_bill_form_commits_what_was_typed() {
        let mut form = BillForm::add();
        assert_eq!(form.editing, None);
        assert_eq!(form.category(), Category::Housing);

        for c in "Plumber".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "$82.00".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        form.choice(Step::NEXT);

        let edit = form.commit().unwrap();
        assert_eq!(edit.label, "Plumber");
        assert_eq!(edit.cents, Cents::from_dollars(82));
        assert_eq!(edit.category, Category::Other);
    }

    #[test]
    fn a_bill_form_opened_on_a_bill_prefills_every_field() {
        let form = BillForm::edit(&bill(4, "Phone", 60, Category::Other, 1));
        assert_eq!(form.editing, Some(BillId(4)));
        assert_eq!(form.display(BillField::Label).plain_text(), "Phone");
        assert_eq!(form.display(BillField::Amount).plain_text(), "60.00");
        assert_eq!(form.category(), Category::Other);
    }

    /// A blank-labelled bill is the state §3 makes an import error; the form
    /// must not be the way back into it.
    #[test]
    fn a_bill_with_no_label_is_refused() {
        let mut form = BillForm::add();
        form.next_field();
        for c in "82".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("label"), "{err}");
    }

    #[test]
    fn a_bill_with_an_unparseable_amount_is_refused_with_the_text_that_failed() {
        let mut form = BillForm::add();
        for c in "Plumber".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "eighty".chars() {
            form.edit(char_key(c));
        }
        let err = form.commit().unwrap_err();
        assert!(err.to_string().contains("eighty"), "{err}");
    }

    /// The selector cycles both categories and comes back round -- there is
    /// no third one to reach.
    #[test]
    fn the_category_selector_cycles_both_ways_through_both_categories() {
        let mut form = BillForm::add();
        form.next_field();
        form.next_field();
        assert_eq!(form.category(), Category::Housing);
        form.choice(Step::NEXT);
        assert_eq!(form.category(), Category::Other);
        form.choice(Step::NEXT);
        assert_eq!(form.category(), Category::Housing);
        form.choice(Step::PREVIOUS);
        assert_eq!(form.category(), Category::Other);
    }

    /// `←`/`→` on a text field must not silently change the category.
    #[test]
    fn cycling_does_nothing_unless_the_category_is_focused() {
        let mut form = BillForm::add();
        form.choice(Step::NEXT);
        form.choice(Step::NEXT);
        assert_eq!(form.category(), Category::Housing);
    }
}
