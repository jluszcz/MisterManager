//! The confirmation modal behind `t`: the transfers a payday would write,
//! and the date they would carry.

use crate::transfer;
use crate::tui::Label;
use crate::tui::form::{Caret, DateField, Step};
use crate::tui::text::Edit;
use crate::tui::widget::{field_line_noted, render_fields};
use anyhow::Result;
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::text::Line as TextLine;

/// The confirm modal behind `t`: what will be written, and when.
///
/// The date is the only editable thing. The rows are not: they are the plan,
/// and a plan edited row by row in a modal is a worksheet, which is what
/// opens next.
pub struct TransferConfirm {
    rows: Vec<transfer::Row>,
    date: DateField,
}

impl TransferConfirm {
    pub fn new(rows: Vec<transfer::Row>, today: NaiveDate, date: NaiveDate) -> TransferConfirm {
        TransferConfirm {
            rows,
            date: DateField::on(today, date),
        }
    }

    pub fn rows(&self) -> &[transfer::Row] {
        &self.rows
    }

    pub fn date_value(&self) -> &str {
        self.date.value()
    }

    /// The date this dialog will write, when what was typed does not already
    /// say it -- a `M/D` shorthand. `None` for a date typed in full.
    pub fn resolved_date(&self) -> Option<String> {
        self.date.resolved()
    }

    pub fn type_char(&mut self, c: char) {
        self.date.push(c);
    }

    pub fn backspace(&mut self) {
        self.date.backspace();
    }

    /// Answer an editing key over the date, the same as any other field.
    pub(in crate::tui) fn edit(&mut self, key: KeyEvent) -> Edit {
        self.date.edit(key)
    }

    pub(in crate::tui) fn caret(&self) -> Caret {
        Caret::in_field(self.date.text())
    }

    /// Step the date by `step`, as `←`/`→` do on every date field in the
    /// app, `Shift` with them a week at a time, and `[`/`]` a month.
    pub fn step_date(&mut self, step: Step) {
        self.date.step(step);
    }

    /// The date as typed. Parsed before anything is written, so a typo leaves
    /// the modal up with everything still in it.
    pub fn commit(&self) -> Result<NaiveDate> {
        self.date.parse()
    }
}

/// The confirm modal behind `t`: what will be written, and when. The strings
/// on screen are the strings that land in the ledger -- a transfer's own
/// name, a withdrawal's line label -- so this is a preview of the ledger
/// rows, not a summary of them. It is the last thing drawn before real money
/// moves, which is why the owner-typed half of it goes through the mask a
/// demo installs.
pub fn render_transfers(frame: &mut Frame, confirm: &TransferConfirm) {
    let rows = confirm.rows();
    let mut lines: Vec<TextLine> = rows
        .iter()
        .map(|row| {
            let (label, cents) = match row {
                // The account's own name, which `transfer::plan` carries
                // unmasked because it is also the description the ledger row
                // is written under. The transfers block on the screen behind
                // this modal reaches the same name through
                // `plan_rows::RowLabel::Account`, which masks; this is the
                // other consumer, and it has no `Account` to inherit that
                // from.
                transfer::Row::Transfer { name, cents, .. } => {
                    (crate::demo::text(name).into_owned(), *cents)
                }
                // The app's own vocabulary, never masked.
                transfer::Row::Withdrawal { line, cents } => (line.label().to_string(), *cents),
            };
            TextLine::from(format!(
                "{label:<40}{:>20}",
                crate::demo::whole_figure(cents)
            ))
        })
        .collect();
    // One field, so focus never leaves it: the resolution `display` would
    // show on any other form goes beside it instead.
    lines.push(field_line_noted(
        "Date",
        Label::from(confirm.date_value().to_string()),
        Some(confirm.caret()),
        &confirm.resolved_date().unwrap_or_default(),
    ));
    lines.push(TextLine::from("Enter write · Esc cancel"));
    render_fields(frame, "Confirm transfers", lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::money::Cents;
    use crate::plan_line::Line;

    use crate::test_support::day;
    use crate::transfer::{self};
    use crate::tui::MIN_WIDTH;

    /// Every rendered line of the confirm modal, inside the border.
    /// `t`'s confirmation is one field, so focus never leaves it and the
    /// unfocused rendering that resolves a shorthand on every other form
    /// never comes round. This is the dialog that moves real money, so it
    /// says what it is about to write beside what was typed.
    #[test]
    fn a_shorthand_on_the_transfer_confirmation_shows_the_date_it_will_write() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 21), day(2026, 8, 24));
        for _ in 0.."2026-08-24".len() {
            confirm.backspace();
        }
        for c in "9/10".chars() {
            confirm.type_char(c);
        }

        let text = drawn_confirm(&confirm);
        assert!(text.contains("9/10"), "{text}");
        assert!(text.contains("2026-09-10"), "{text}");
        assert_eq!(confirm.commit().unwrap(), day(2026, 9, 10));
    }

    /// A date already written out is not repeated: a note echoing the field
    /// beside it is noise on every date the owner types in full.
    #[test]
    fn a_confirmation_date_typed_in_full_is_shown_once() {
        let confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 21), day(2026, 8, 24));
        let text = drawn_confirm(&confirm);
        assert_eq!(text.matches("2026-08-24").count(), 1, "{text}");
    }

    /// Half a date has no date to resolve to, so nothing is offered: a note
    /// that guessed at one would be a date the dialog never writes.
    #[test]
    fn a_half_typed_confirmation_date_is_offered_no_resolution() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 21), day(2026, 8, 24));
        for _ in 0.."24".len() {
            confirm.backspace();
        }

        let text = drawn_confirm(&confirm);
        assert!(text.contains("2026-08-"), "{text}");
        assert!(confirm.commit().is_err());
    }

    /// The modal that confirms a payday is the last thing shown before real
    /// money moves, and it is as much a part of the demo as the screen behind
    /// it.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_confirm_modal_and_keeps_its_date() {
        crate::demo::install_with_salt(7);
        let rows = vec![transfer::Row::Transfer {
            to: crate::db::AccountId(1),
            name: "Brokerage".to_string(),
            color: None,
            cents: Cents(123_456),
            lines: Vec::new(),
        }];
        let text = drawn_confirm(&TransferConfirm::new(
            rows,
            day(2026, 8, 24),
            day(2026, 8, 24),
        ));

        assert!(!text.contains("1,234"), "the transfer survived: {text}");
        assert!(
            text.contains(&crate::demo::whole_figure(Cents(123_456))),
            "no scrambled transfer found: {text}"
        );
        assert!(
            !text.contains("Brokerage"),
            "the destination survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Brokerage").to_string()),
            "no scrambled destination found: {text}"
        );
        assert!(text.contains("2026-08-24"), "the date must stay: {text}");
    }

    fn drawn_confirm(confirm: &TransferConfirm) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 12)).unwrap();
        terminal
            .draw(|frame| {
                render_transfers(frame, confirm);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..8)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The confirm modal is part of the Planning screen, so its amounts drop
    /// the cents like every other figure on it. It is neither the pin-drift
    /// footer nor the `edit` prefill, the two places that keep full
    /// precision on purpose.
    #[test]
    fn the_confirm_modal_renders_whole_dollars() {
        let rows = vec![
            transfer::Row::Transfer {
                to: crate::db::AccountId(1),
                name: "Brokerage".to_string(),
                color: None,
                cents: Cents(123_456),
                lines: Vec::new(),
            },
            transfer::Row::Withdrawal {
                line: Line::Retirement,
                cents: Cents(197_900),
            },
        ];
        let text = drawn_confirm(&TransferConfirm::new(
            rows,
            day(2026, 8, 24),
            day(2026, 8, 24),
        ));

        assert!(text.contains("1,234"), "{text}");
        assert!(text.contains("1,979"), "{text}");
        assert!(!text.contains("1,234.56"), "{text}");
        assert!(!text.contains("1,979.00"), "{text}");
    }

    /// The date opens two business days out and is editable: an unparseable
    /// one is refused with the modal still up, so nothing is written on a
    /// typo.
    #[test]
    fn the_confirm_modal_opens_two_business_days_out_and_refuses_a_bad_date() {
        let date = crate::calc::business_day::add(day(2026, 8, 20), 2).unwrap();
        let mut confirm = TransferConfirm::new(Vec::new(), date, date);
        assert_eq!(confirm.date_value(), "2026-08-24");
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 24));

        for _ in 0..10 {
            confirm.backspace();
        }
        for c in "not-a-date".chars() {
            confirm.type_char(c);
        }
        assert!(confirm.commit().is_err());
    }

    /// The dialog's one field is a date, so `←`/`→` step it a day at a time —
    /// the same meaning they carry on every other date field in the app.
    #[test]
    fn the_arrows_step_the_confirm_date_by_a_day() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 24), day(2026, 8, 24));
        confirm.step_date(Step::NEXT);
        assert_eq!(confirm.date_value(), "2026-08-25");
        confirm.step_date(Step::PREVIOUS);
        confirm.step_date(Step::PREVIOUS);
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 23));
    }

    /// `[`/`]` step that same field a month, the meaning they carry on every
    /// date field in the app.
    #[test]
    fn the_brackets_step_the_confirm_date_by_a_month() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 24), day(2026, 8, 24));
        confirm.step_date(Step::NEXT_MONTH);
        assert_eq!(confirm.date_value(), "2026-09-24");
        confirm.step_date(Step::PREVIOUS_MONTH);
        confirm.step_date(Step::PREVIOUS_MONTH);
        assert_eq!(confirm.commit().unwrap(), day(2026, 7, 24));
    }

    /// A half-typed date keeps what was typed: the arrows nudge a date that
    /// is already there rather than conjuring one.
    #[test]
    fn the_arrows_leave_a_half_typed_confirm_date_alone() {
        let mut confirm = TransferConfirm::new(Vec::new(), day(2026, 8, 24), day(2026, 8, 24));
        for _ in 0..10 {
            confirm.backspace();
        }
        for c in "2026-0".chars() {
            confirm.type_char(c);
        }
        confirm.step_date(Step::NEXT);
        assert_eq!(confirm.date_value(), "2026-0");
    }
}
