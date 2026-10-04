//! Screen 8's draw. What it draws is `crate::retirement`'s; the decisions
//! here are only a terminal's -- glyphs, widths, and which figure wears a
//! color.

use super::Label;
use super::form::{DateField, Field, Focused, FormFields, next_in, parse_whole_amount};
use super::style;
use super::widget::{field_stack, render_fields};
use crate::balance_history::{History, Series};
use crate::calc::Month;
use crate::calc::retirement::{BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::retirement::Retirement;
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Cell, Chart, LegendPosition, Paragraph, Row, Table};

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
        text.push_str(&format!(" · Age {age}"));
    }
    if let Some(salary) = r.salary {
        text.push_str(&format!(" · Salary {}", dollars(salary)));
    }
    Line::from(text)
}

/// What the "Where you stand" box holds: the two standings, or the one thing
/// standing between the owner and them.
fn standing_lines(r: &Retirement) -> Vec<Line<'static>> {
    if r.held.is_empty() {
        return vec![Line::from(
            "Mark investment accounts as Retirement on Accounts (0)",
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
                format!("{}%", share.tenth_percent()),
                now.tax_free.to_string()
            )),
            status_span(status, short_by),
        ]),
        _ => Line::from(format!("Tax-free  {:>12}   {ABSENT}", dollars(r.tax_free))),
    };
    vec![saved, tax_free]
}

/// A milestone's shortfall in today's dollars, in the color a short
/// standing wears; blank once met, and absent where there is no saying.
fn short_cell(short: Option<Cents>) -> Cell<'static> {
    match short {
        None => Cell::from(ABSENT),
        Some(gap) if gap.0 <= 0 => Cell::from(""),
        Some(gap) => Cell::from(Span::styled(
            dollars(gap),
            Style::default().fg(style::standing_color(Status::Short)),
        )),
    }
}

fn milestone_table(r: &Retirement) -> Table<'static> {
    let header = Row::new(vec![
        "  Milestone",
        "Saved × salary",
        "Saved $",
        "Short",
        "Tax-free",
        "Short",
    ])
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
            short_cell(r.saved_short(row)),
            Cell::from(row.tax_free.to_string()),
            short_cell(r.tax_free_short(row)),
        ])
    });
    Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(16),
            // `$6.5MM – $9.5MM` and a gap.
            Constraint::Length(18),
            // `$10,000,000` and a gap.
            Constraint::Length(13),
            // `17.5 – 25.0%` and a gap.
            Constraint::Length(14),
            Constraint::Min(13),
        ],
    )
    .header(header)
}

/// What a chart with no account behind it draws instead of axes.
const NO_HISTORY: &str = "Nothing recorded yet";

/// A month as a point on the x axis: consecutive months one apart, whatever
/// their lengths.
fn month_x(month: Month) -> f64 {
    let first = month.first_day();
    f64::from(first.year() * 12 + first.month0() as i32)
}

/// One chart: a line per account, in its own color, across every month any
/// of them was recorded in. The y axis starts at zero, so a line's height is
/// its balance rather than its distance from the lowest one drawn.
fn render_chart(frame: &mut Frame, area: Rect, title: &'static str, series: &[Series]) {
    let block = Block::bordered().title(title);
    let months = || series.iter().flat_map(|s| s.points.iter().map(|(m, _)| *m));
    let (Some(first), Some(last)) = (months().min(), months().max()) else {
        frame.render_widget(Paragraph::new(NO_HISTORY).block(block), area);
        return;
    };
    let cents = || series.iter().flat_map(|s| s.points.iter().map(|(_, c)| *c));
    let low = cents().min().unwrap_or(Cents::ZERO).min(Cents::ZERO);
    // A dollar of range at least, so a chart of nothing but zeroes still has
    // an axis to draw them against.
    let high = cents().max().unwrap_or(Cents::ZERO).max(low + Cents(100));
    let data: Vec<Vec<(f64, f64)>> = series
        .iter()
        .map(|s| {
            s.points
                .iter()
                .map(|(m, c)| (month_x(*m), c.0 as f64))
                .collect()
        })
        .collect();
    let datasets = series
        .iter()
        .zip(&data)
        .map(|(s, points)| super::account_series(&s.account, points))
        .collect();
    // One month alone is centred between a month either side: on an axis of
    // its own width it would sit on the left edge, under the legend.
    let label = |m: Month| m.first_day().format("%b %Y").to_string();
    let x = match first == last {
        true => Axis::default()
            .bounds([month_x(first) - 1.0, month_x(first) + 1.0])
            .labels([String::new(), label(first), String::new()]),
        false => Axis::default()
            .bounds([month_x(first), month_x(last)])
            .labels([label(first), label(last)]),
    };
    let y = Axis::default()
        .bounds([low.0 as f64, high.0 as f64])
        .labels([
            compact(low),
            compact(Cents((low.0 + high.0) / 2)),
            compact(high),
        ]);
    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(x)
        .y_axis(y)
        .legend_position(Some(LegendPosition::TopLeft))
        // Shown whenever it fits at all: an account's line is told apart from
        // its neighbours only by its color, and the legend is what names it.
        .hidden_legend_constraints((Constraint::Percentage(100), Constraint::Percentage(100)));
    frame.render_widget(chart, area);
}

pub(super) fn render(frame: &mut Frame, area: Rect, r: &Retirement, history: &History) {
    let standing = standing_lines(r);
    // Each box is its content plus a border above and below, the table a
    // header row too. The charts take whatever is left.
    let [title_area, box_area, table_area, charts_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(standing.len() as u16 + 2),
        Constraint::Length(r.rows.len() as u16 + 3),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(title(r)), title_area);
    frame.render_widget(
        Paragraph::new(standing).block(Block::bordered().title("Where you stand")),
        box_area,
    );
    frame.render_widget(
        milestone_table(r).block(Block::bordered().title("Milestones")),
        table_area,
    );
    // Side by side rather than stacked: a chart is read across, and height
    // is the dimension the boxes above have already spent.
    let [cash_area, investment_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(charts_area);
    render_chart(frame, cash_area, "Cash balances over time", &history.cash);
    render_chart(
        frame,
        investment_area,
        "Investment balances over time",
        &history.investment,
    );
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
