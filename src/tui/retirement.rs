//! Screen 8's draw. What it draws is `crate::retirement`'s; the decisions
//! here are only a terminal's -- glyphs, widths, and which figure wears a
//! color.

use super::Label;
use super::form::{DateField, Field, Focused, FormFields, next_in, parse_whole_amount};
use super::style;
use super::widget::{field_stack, render_fields};
use crate::balance_history::{
    self, CASH_TITLE, Charts, History, INVESTMENT_TITLE, NO_HISTORY, Series,
};
use crate::calc::Month;
use crate::calc::retirement::{BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::retirement::Retirement;
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Cell, Chart, Dataset, GraphType, LegendPosition, Paragraph, Row, Table,
};

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

/// The charts' window is named only once it is narrowed: the whole history
/// is what the x axes already show, and a screen quoting a narrowed one
/// must say so.
fn title(r: &Retirement, charts: Option<(Month, Month)>) -> Line<'static> {
    let mut text = String::from("Retirement");
    if let Some(age) = r.age {
        text.push_str(&format!(" · Age {age}"));
    }
    if let Some(salary) = r.salary {
        text.push_str(&format!(" · Salary {}", dollars(salary)));
    }
    if let Some((first, last)) = charts {
        text.push_str(&format!(
            " · Charts {}{BAND_DASH}{}",
            first.label(),
            last.label()
        ));
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

/// The months both charts are drawn across. `None` at an edge is that edge
/// of the history, whatever it has grown to by the next draw -- which is
/// what the screen opens on, and what `Esc` goes back to.
///
/// View state only, like the Overview's scrub: nothing is stored, so every
/// launch opens on the whole history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    start: Option<Month>,
    end: Option<Month>,
}

impl Window {
    /// A window from `start` through `end`, held inside `extent`: an edge
    /// past the history is moved onto it, and an edge landing *on* it is
    /// stored as `None`, so a window stepped back out to the history's edge
    /// is the unnarrowed one rather than a copy of it that stops growing.
    pub fn within(start: Option<Month>, end: Option<Month>, extent: (Month, Month)) -> Window {
        let (first, last) = extent;
        let start = start.unwrap_or(first).clamp(first, last);
        let end = end.unwrap_or(last).clamp(start, last);
        Window {
            start: (start != first).then_some(start),
            end: (end != last).then_some(end),
        }
    }

    pub fn start(self) -> Option<Month> {
        self.start
    }

    pub fn end(self) -> Option<Month> {
        self.end
    }

    /// The months drawn, given what is recorded -- held inside it, since the
    /// history a window was stepped against may not be the one it is drawn
    /// against after a reload.
    pub fn bounds(self, extent: (Month, Month)) -> (Month, Month) {
        let held = Window::within(self.start, self.end, extent);
        (held.start.unwrap_or(extent.0), held.end.unwrap_or(extent.1))
    }

    /// The start moved by `months`, no later than the end.
    pub fn step_start(self, months: i32, extent: (Month, Month)) -> Window {
        let (start, end) = self.bounds(extent);
        Window::within(Some(start.shifted(months).min(end)), self.end, extent)
    }

    /// The end moved by `months`, no earlier than the start.
    pub fn step_end(self, months: i32, extent: (Month, Month)) -> Window {
        let (start, end) = self.bounds(extent);
        Window::within(self.start, Some(end.shifted(months).max(start)), extent)
    }

    pub fn is_narrowed(self) -> bool {
        self != Window::default()
    }
}

/// What a chart with history, none of it inside the window, draws instead.
const NOTHING_IN_WINDOW: &str = "Nothing recorded in this window";

/// A month as a point on the x axis.
fn month_x(month: Month) -> f64 {
    f64::from(month.ordinal())
}

fn plot(points: &[(Month, Cents)]) -> Vec<(f64, f64)> {
    points
        .iter()
        .map(|(m, c)| (month_x(*m), c.0 as f64))
        .collect()
}

/// One chart: a line per account, in its own color, and their sum, where
/// there is one, in [`style::TOTAL`], against the scale both charts share.
fn render_chart(
    frame: &mut Frame,
    area: Rect,
    title: &'static str,
    chart: &balance_history::Chart,
    charts: &Charts,
    empty: &'static str,
) {
    let block = Block::bordered().title(title);
    if chart.series.is_empty() {
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let balance_history::Chart { series, total } = chart;
    let (first, last) = charts.months;
    let (low, high) = charts.cents;
    let data: Vec<Vec<(f64, f64)>> = series.iter().map(|s| plot(&s.points)).collect();
    let total_data = plot(total);
    let ink = Style::default().fg(style::TOTAL);
    let datasets = series
        .iter()
        .zip(&data)
        .map(|(s, points)| super::account_series(&s.account, points))
        .chain((!total.is_empty()).then(|| {
            Dataset::default()
                .name(Span::styled("Total", ink))
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(ink)
                .data(&total_data)
        }))
        .collect();
    // One month alone is centred between a month either side: on an axis of
    // its own width it would sit on the left edge, under the legend.
    let x = match first == last {
        true => Axis::default()
            .bounds([month_x(first) - 1.0, month_x(first) + 1.0])
            .labels([String::new(), first.label(), String::new()]),
        false => Axis::default()
            .bounds([month_x(first), month_x(last)])
            .labels([first.label(), last.label()]),
    };
    let y = Axis::default()
        .bounds([low.0 as f64, high.0 as f64])
        .labels(charts.ticks().map(compact));
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

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    r: &Retirement,
    history: &History,
    window: Window,
) {
    let standing = standing_lines(r);
    let bounds = balance_history::extent(history).map(|extent| window.bounds(extent));
    // Each box is its content plus a border above and below, the table a
    // header row too. The charts take whatever is left.
    let [title_area, box_area, table_area, charts_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(standing.len() as u16 + 2),
        Constraint::Length(r.rows.len() as u16 + 3),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(title(r, bounds.filter(|_| window.is_narrowed()))),
        title_area,
    );
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
    let empty = |series: &[Series]| match series.is_empty() {
        true => NO_HISTORY,
        false => NOTHING_IN_WINDOW,
    };
    let Some(months) = bounds else {
        for (area, title) in [(cash_area, CASH_TITLE), (investment_area, INVESTMENT_TITLE)] {
            let block = Block::bordered().title(title);
            frame.render_widget(Paragraph::new(NO_HISTORY).block(block), area);
        }
        return;
    };
    let charts = Charts::new(history, months);
    render_chart(
        frame,
        cash_area,
        CASH_TITLE,
        &charts.cash,
        &charts,
        empty(&history.cash),
    );
    render_chart(
        frame,
        investment_area,
        INVESTMENT_TITLE,
        &charts.investment,
        &charts,
        empty(&history.investment),
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowField {
    Start,
    End,
}

impl WindowField {
    /// Tab order, and the order the fields render in.
    pub const ORDER: [WindowField; 2] = [WindowField::Start, WindowField::End];

    pub fn label(self) -> &'static str {
        match self {
            WindowField::Start => "Start",
            WindowField::End => "End",
        }
    }
}

/// `w` on Retirement: the charts' window typed rather than stepped. A date
/// field rather than a month one, because a `DateField` is what every date in
/// the app is typed into; any day names its month.
///
/// It opens on the months the charts are drawing, open edges included, so a
/// change is an edit of what is on screen rather than a retyping of it: the
/// first of the start month and the last of the end one, reading as the span
/// drawn. An edge saved where the history stops follows it again, through
/// [`Window::within`], and an edge left blank is that edge of the history too.
#[derive(Debug)]
pub struct WindowForm {
    pub focus: WindowField,
    start: DateField,
    end: DateField,
}

impl WindowForm {
    /// `shown` is what the charts draw, `None` before anything is recorded.
    pub fn new(today: NaiveDate, shown: Option<(Month, Month)>) -> WindowForm {
        WindowForm {
            focus: WindowField::Start,
            start: DateField::given(today, shown.map(|(start, _)| start.first_day())),
            end: DateField::given(today, shown.map(|(_, end)| end.last_day())),
        }
    }

    pub fn display(&self, field: WindowField) -> Label {
        let focused = self.focus == field;
        Label::from(match field {
            WindowField::Start => self.start.display(focused),
            WindowField::End => self.end.display(focused),
        })
    }

    /// The two months, refusing an end before the start: clamping one to the
    /// other would draw a window the owner did not type.
    pub fn commit(&self) -> Result<(Option<Month>, Option<Month>)> {
        let start = self.start.parse_opt()?.map(Month::of);
        let end = self.end.parse_opt()?.map(Month::of);
        if let (Some(start), Some(end)) = (start, end) {
            ensure!(start <= end, "the end is before the start");
        }
        Ok((start, end))
    }
}

impl FormFields for WindowForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&WindowField::ORDER, self.focus, step);
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            WindowField::Start => Focused::Date(&mut self.start),
            WindowField::End => Focused::Date(&mut self.end),
        }
    }
}

pub(super) fn render_window_form(frame: &mut Frame, form: &mut WindowForm) {
    let caret = form.caret();
    let lines = field_stack(
        &WindowField::ORDER,
        form.focus,
        caret,
        WindowField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(
        frame,
        "Chart window — blank is the whole history · Enter save · Esc cancel",
        lines,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_label::Account;
    use crate::test_support::{cash, day};
    use jluszcz_finance_utils::tui::testing::draw_buffer;
    use ratatui::buffer::Buffer;

    const WIDTH: u16 = 60;
    const HEIGHT: u16 = 20;

    fn series(accounts: &[crate::db::account::Account], index: usize, dollars: [i64; 2]) -> Series {
        Series {
            account: Account::named(accounts, accounts[index].id),
            points: vec![
                (Month::of(day(2026, 7, 1)), Cents::from_dollars(dollars[0])),
                (Month::of(day(2026, 8, 1)), Cents::from_dollars(dollars[1])),
            ],
        }
    }

    fn drawn(series: &[Series]) -> Buffer {
        draw_buffer(WIDTH, HEIGHT, |frame| {
            let history = History {
                cash: series.to_vec(),
                ..History::default()
            };
            let charts = Charts::new(&history, (month(7), month(8)));
            render_chart(
                frame,
                frame.area(),
                "Chart",
                &charts.cash,
                &charts,
                NO_HISTORY,
            )
        })
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect()
    }

    /// The accounts top out at $9,000 and their sum at $11,000: the one axis
    /// reaches the sum, so the Total is drawn rather than clipped off the top.
    #[test]
    fn the_total_is_drawn_in_the_terminal_foreground_against_the_accounts_axis() {
        let accounts = [cash(1, "CHK"), cash(2, "SAV")];
        let buffer = drawn(&[
            series(&accounts, 0, [1_000, 2_000]),
            series(&accounts, 1, [9_000, 9_000]),
        ]);
        let top = row(&buffer, 1);
        assert!(top.starts_with("│$11K"), "{top}");
        let legend = (0..HEIGHT).map(|y| row(&buffer, y)).collect::<String>();
        assert!(legend.contains("Total"), "{legend}");
        let black_line = buffer.content().iter().any(|cell| {
            cell.fg == style::TOTAL
                && cell
                    .symbol()
                    .chars()
                    .all(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
        });
        assert!(black_line, "no braille drawn in the Total's color");
    }

    fn month(m: u32) -> Month {
        Month::of(day(2026, m, 1))
    }

    /// March through August.
    const EXTENT: fn() -> (Month, Month) = || (month(3), month(8));

    #[test]
    fn a_window_reaching_past_the_history_is_held_inside_it() {
        let window = Window::within(
            Some(Month::of(day(2025, 1, 1))),
            Some(Month::of(day(2027, 1, 1))),
            EXTENT(),
        );
        assert_eq!(window.bounds(EXTENT()), EXTENT());
        assert!(!window.is_narrowed());
    }

    /// Stored as `None` rather than as August, so a month recorded next
    /// month is drawn without the owner widening the window to reach it.
    #[test]
    fn an_edge_stepped_back_onto_the_history_follows_it_again() {
        let window = Window::default()
            .step_end(-1, EXTENT())
            .step_end(1, EXTENT());
        assert_eq!(window, Window::default());
    }

    #[test]
    fn a_window_drawn_against_a_shorter_history_than_it_was_set_on_stays_inside_it() {
        let window = Window::within(Some(month(7)), None, EXTENT());
        assert_eq!(window.bounds((month(3), month(5))), (month(5), month(5)));
    }

    #[test]
    fn the_start_stops_at_the_end_rather_than_passing_it() {
        let window = Window::default().step_end(-3, EXTENT());
        let window = (0..10).fold(window, |w, _| w.step_start(1, EXTENT()));
        assert_eq!(window.bounds(EXTENT()), (month(5), month(5)));
        let window = (0..10).fold(window, |w, _| w.step_end(-1, EXTENT()));
        assert_eq!(window.bounds(EXTENT()), (month(5), month(5)));
    }

    #[test]
    fn the_window_form_refuses_an_end_before_the_start() {
        let mut form = WindowForm::new(day(2026, 8, 15), Some((month(6), month(7))));
        assert_eq!(form.commit().unwrap(), (Some(month(6)), Some(month(7))));
        form.focus = WindowField::End;
        let Focused::Date(end) = form.focused() else {
            panic!("End is a date field");
        };
        for _ in 0.."2026-07-31".len() {
            end.backspace();
        }
        for c in "2026-04-30".chars() {
            end.push(c);
        }
        assert!(form.commit().is_err());
    }
}
