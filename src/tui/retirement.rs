//! Screen 0's draw. What it draws is `crate::retirement`'s; the decisions
//! here are only a terminal's -- glyphs, widths, and which figure wears a
//! color.

use super::style;
use crate::calc::retirement::{Band, Status};
use crate::money::Cents;
use crate::retirement::Retirement;
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

fn dollar_band(band: Band<Cents>) -> String {
    match band.low == band.high {
        true => dollars(band.low),
        false => format!("{}–{}", dollars(band.low), dollars(band.high)),
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
        (Some(share), Some(status)) => Line::from(vec![
            Span::raw(format!(
                "Tax-free  {:>12}   {:>7}   target now {:<16}",
                dollars(r.tax_free),
                format!("{share}%"),
                now.tax_free.to_string()
            )),
            status_span(status, None),
        ]),
        _ => Line::from(format!("Tax-free  {:>12}   {ABSENT}", dollars(r.tax_free))),
    };
    vec![saved, tax_free]
}

fn milestone_table(r: &Retirement) -> Table<'static> {
    let header = Row::new(vec!["  Milestone", "Saved × salary", "Saved $", "Tax-free"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = r.rows.iter().map(|row| {
        let label = match (row.now, row.extrapolated) {
            (true, _) => format!("▸ Now ({})", row.age),
            (false, true) => format!("  By {} ~", row.age),
            (false, false) => format!("  By {}", row.age),
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
            super::tax_treatment_cell(Some(h.treatment)),
            super::whole_amount(h.balance),
            Cell::from(Line::from(share).right_aligned()),
        ])
    });
    Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(9),
        ],
    )
    .header(header)
}

pub(super) fn render(frame: &mut Frame, area: Rect, r: &Retirement) {
    let standing = standing_lines(r);
    let footnote = r.rows.iter().any(|row| row.extrapolated && !row.now);
    let [
        title_area,
        box_area,
        _,
        table_area,
        note_area,
        _,
        accounts_area,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(standing.len() as u16 + 2),
        Constraint::Length(1),
        Constraint::Length(r.rows.len() as u16 + 1),
        Constraint::Length(u16::from(footnote)),
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
    if footnote {
        frame.render_widget(Paragraph::new("  ~ extrapolated past 45"), note_area);
    }
    if !r.held.is_empty() {
        frame.render_widget(account_table(r), accounts_area);
    }
}
