//! Screen 0's draw. What it draws is `crate::retirement`'s; the decisions
//! here are only a terminal's -- glyphs, widths, and which figure wears a
//! color.

use super::Label;
use super::form::{DateField, Field, Focused, FormFields, next_in, parse_whole_amount};
use super::style;
use super::widget::{field_stack, render_fields};
use crate::calc::retirement::{BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::retirement::Retirement;
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

/// What a figure that cannot be stated draws as -- the Funds screen's word
/// for the same thing.
const ABSENT: &str = "--";

fn dollars(cents: Cents) -> String {
    super::dollar(crate::demo::whole_figure(cents))
}

fn compact(cents: Cents) -> String {
    super::dollar(crate::demo::compact_figure(cents))
}

fn dollar_band(band: Band<Cents>) -> String {
    match band.low == band.high {
        true => compact(band.low),
        false => format!("{}{BAND_DASH}{}", compact(band.low), compact(band.high)),
    }
}

fn status_span(status: Status, short_by: Option<Cents>) -> Span<'static> {
    let text = match short_by {
        Some(gap) => format!("{} {}", status.label(), dollars(gap)),
        None => status.label().to_string(),
    };
    Span::styled(text, Style::default().fg(style::standing_color(status)))
}

fn title(r: &Retirement) -> Line<'static> {
    let mut text = String::from("Retirement");
    if let Some(age) = r.age {
        text.push_str(&format!(" · age {age}"));
    }
    if let Some(salary) = r.salary {
        text.push_str(&format!(" · salary {}", dollars(salary)));
    }
    Line::from(text)
}

/// What the "Where you stand" box holds: the two standings, or the one thing
/// standing between the owner and them.
fn standing_lines(r: &Retirement) -> Vec<Line<'static>> {
    if r.held.is_empty() {
        return vec![Line::from(
            "Mark investment accounts as Retirement on Accounts (9)",
        )];
    }
    let Some(now) = r.now() else {
        return vec![Line::from("Press e to set a birth date")];
    };
    let saved = match (r.multiple(), r.saved_status()) {
        (Some(multiple), Some((status, short_by))) => Line::from(vec![
            Span::raw(format!(
                "Saved     {:>12}   {:>7}   target now {:<16}",
                dollars(r.saved),
                multiple.to_string(),
                now.saved.to_string()
            )),
            status_span(status, short_by),
        ]),
        _ => Line::from(format!(
            "Saved     {:>12}   Press e to set a salary",
            dollars(r.saved)
        )),
    };
    let tax_free = match (r.tax_free_share(), r.tax_free_status()) {
        (Some(share), Some((status, short_by))) => Line::from(vec![
            Span::raw(format!(
                "Tax-free  {:>12}   {:>7}   target now {:<16}",
                dollars(r.tax_free),
                format!("{share}%"),
                now.tax_free.to_string()
            )),
            status_span(status, short_by),
        ]),
        _ => Line::from(format!("Tax-free  {:>12}   {ABSENT}", dollars(r.tax_free))),
    };
    vec![saved, tax_free]
}

fn milestone_table(r: &Retirement) -> Table<'static> {
    let header = Row::new(vec!["  Milestone", "Saved × salary", "Saved $", "Tax-free"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = r.rows.iter().map(|row| {
        let label = match row.now {
            true => format!("▸ Now ({})", row.age),
            false => format!("  By {}", row.age),
        };
        let multiple = match r.salary {
            Some(_) => row.saved.to_string(),
            None => ABSENT.to_string(),
        };
        let target = row
            .saved_dollars
            .map(dollar_band)
            .unwrap_or_else(|| ABSENT.to_string());
        Row::new(vec![
            Cell::from(label),
            Cell::from(multiple),
            Cell::from(target),
            Cell::from(row.tax_free.to_string()),
        ])
    });
    Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Length(26),
            Constraint::Min(14),
        ],
    )
    .header(header)
}

fn account_table(r: &Retirement) -> Table<'static> {
    let header = Row::new(vec![
        Cell::from("Account"),
        Cell::from("Tax"),
        super::right_header("Balance"),
        super::right_header("Share"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = r.held.iter().map(|h| {
        let share = h
            .share_of(r.saved)
            .map(|s| format!("{s}%"))
            .unwrap_or_else(|| ABSENT.to_string());
        Row::new(vec![
            super::account_cell(&h.account),
            Cell::from(h.tax_label()),
            super::whole_amount(h.balance),
            Cell::from(Line::from(share).right_aligned()),
        ])
    });
    Table::new(
        rows,
        [
            Constraint::Min(20),
            super::label_width("Tax", super::tax_labels()),
            Constraint::Length(14),
            Constraint::Length(9),
        ],
    )
    .header(header)
}

pub(super) fn render(frame: &mut Frame, area: Rect, r: &Retirement) {
    let standing = standing_lines(r);
    let [title_area, box_area, _, table_area, _, accounts_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(standing.len() as u16 + 2),
        Constraint::Length(1),
        Constraint::Length(r.rows.len() as u16 + 1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(title(r)), title_area);
    frame.render_widget(
        Paragraph::new(standing).block(Block::bordered().title("Where you stand")),
        box_area,
    );
    frame.render_widget(milestone_table(r), table_area);
    if !r.held.is_empty() {
        frame.render_widget(account_table(r), accounts_area);
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RetirementField {
    Salary,
    BirthDate,
}

impl RetirementField {
    /// Tab order, and the order the fields render in.
    pub const ORDER: [RetirementField; 2] = [RetirementField::Salary, RetirementField::BirthDate];

    pub fn label(self) -> &'static str {
        match self {
            RetirementField::Salary => "Salary",
            RetirementField::BirthDate => "Birth date",
        }
    }
}

/// `e` on Retirement: the two facts about the owner the screen needs and no
/// account carries. Either may be left blank, which clears it -- unset is a
/// state each reader already draws.
#[derive(Debug)]
pub struct RetirementForm {
    pub focus: RetirementField,
    salary: Field,
    birth: DateField,
}

impl RetirementForm {
    pub fn new(
        today: NaiveDate,
        salary: Option<Cents>,
        birth: Option<NaiveDate>,
    ) -> RetirementForm {
        RetirementForm {
            focus: RetirementField::Salary,
            // Whole dollars by construction, so the prefill round-trips
            // through `parse_whole_amount`.
            salary: Field::given(salary.map(|s| s.dollars().to_string()).unwrap_or_default()),
            birth: DateField::given(today, birth),
        }
    }

    pub fn display(&self, field: RetirementField) -> Label {
        match field {
            RetirementField::Salary => Label::from(crate::demo::typed(self.salary.value())),
            RetirementField::BirthDate => Label::from(self.birth.display(self.focus == field)),
        }
    }

    /// A salary of nothing is refused rather than stored: every multiple is a
    /// divide by it.
    pub fn commit(&self) -> Result<(Option<Cents>, Option<NaiveDate>)> {
        let salary = match self.salary.value().trim() {
            "" => None,
            raw => {
                let cents = parse_whole_amount(raw)?;
                ensure!(cents.0 > 0, "salary must be more than nothing");
                Some(cents)
            }
        };
        Ok((salary, self.birth.parse_opt()?))
    }
}

impl FormFields for RetirementForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&RetirementField::ORDER, self.focus, step);
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            RetirementField::Salary => Focused::Text(&mut self.salary),
            RetirementField::BirthDate => Focused::Date(&mut self.birth),
        }
    }
}

pub(super) fn render_form(frame: &mut Frame, form: &mut RetirementForm) {
    let caret = form.caret();
    let lines = field_stack(
        &RetirementField::ORDER,
        form.focus,
        caret,
        RetirementField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(
        frame,
        "Edit retirement — Tab field · Enter save · Esc cancel",
        lines,
    );
}
