//! The Funds screen: the target/actual split of `Planning!I1:M5`, and the
//! form that edits it.
//!
//! View state only -- no ratatui above the render functions at the bottom,
//! and no `Db` on the type. `App` runs the queries and hands the results in.

use super::Label;
use super::cursor::{Cursor, Viewport, impl_scroll};
use super::form::{self, Field, Focused, FormFields, Step, next_in, step_index};
use crate::db::FundId;
use crate::db::fund::{Fund, FundEdit, Target};
use crate::fund::Allocation;
use crate::money::Cents;
use crate::rate::BasisPoints;
use anyhow::{Result, ensure};

/// One fund as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub fund_id: FundId,
    pub name: String,
    /// `None` for an age row with no birth date on record, which draws as
    /// `—`: a blank cell would read as zero.
    pub target: Option<BasisPoints>,
    pub actual_share: BasisPoints,
    pub delta: Option<BasisPoints>,
    pub actual: Cents,
}

pub struct Funds {
    rows: Vec<Row>,
    total: Cents,
    target_total: BasisPoints,
    furthest_down: Option<usize>,
    cursor: Cursor,
}

impl Funds {
    pub fn new() -> Funds {
        Funds {
            rows: Vec::new(),
            total: Cents::ZERO,
            target_total: BasisPoints::ZERO,
            furthest_down: None,
            cursor: Cursor::new(),
        }
    }

    pub fn set_allocation(&mut self, allocation: Allocation) {
        self.rows = allocation
            .rows
            .into_iter()
            .map(|row| Row {
                fund_id: row.id,
                name: row.name,
                target: row.target,
                actual_share: row.actual_share,
                delta: row.delta,
                actual: row.actual,
            })
            .collect();
        self.total = allocation.total;
        self.target_total = allocation.target_total;
        self.furthest_down = allocation.furthest_down;
        self.cursor.clamp(self.rows.len());
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.cursor.index())
    }

    pub fn total(&self) -> Cents {
        self.total
    }

    pub fn target_total(&self) -> BasisPoints {
        self.target_total
    }

    /// The row the next contribution should go to, if any row is short.
    pub fn furthest_down(&self) -> Option<usize> {
        self.furthest_down
    }

    /// Whether the screen should ask for a birth date.
    ///
    /// Derived rather than stored: a row with no target is exactly an age row
    /// whose age is unknown, so the question stands while -- and only while
    /// -- there is a row that cannot answer it. A table of share rows needs
    /// no birth date and is never asked for one.
    pub fn needs_birth_date(&self) -> bool {
        self.rows.iter().any(|row| row.target.is_none())
    }

    pub fn title(&self) -> String {
        format!("Funds · {}", self.rows.len())
    }
}

impl Default for Funds {
    fn default() -> Funds {
        Funds::new()
    }
}

impl_scroll!(Funds, rows);

/// A share typed as a percentage, into basis points: `40` and `40.00` are
/// both `BasisPoints(4_000)`.
///
/// Parsed by `form::parse_amount` because a percentage with two decimals and
/// an amount with two decimals are the same grammar, thousands separators
/// included; the `Cents` it returns is the scaled integer here and never
/// money.
///
/// Bounded to `0..=100`, which `Percent` deliberately is not: this one is a
/// share of a remainder being divided up, so outside the range it would hand
/// a fund more than there is to give.
pub fn parse_share(raw: &str) -> Result<BasisPoints> {
    let scaled = form::parse_amount(raw)?;
    ensure!(
        (0..=BasisPoints::ONE.0).contains(&scaled.0),
        "share must be between 0 and 100: {:?}",
        raw.trim()
    );
    Ok(BasisPoints(scaled.0))
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FundField {
    Name,
    Kind,
    Share,
    Actual,
}

impl FundField {
    pub fn label(self) -> &'static str {
        match self {
            FundField::Name => "Name",
            FundField::Kind => "Target",
            FundField::Share => "Share",
            FundField::Actual => "Value",
        }
    }
}

/// Adding or editing one fund. Backs `a` and `E`.
///
/// The kind is a selector rather than a text field, so a kind the schema's
/// `CHECK` would refuse is not representable. `Share` is only a field for a
/// share row: an age row's target is a rule, not a number to type, so the
/// form does not offer a box for it.
#[derive(Debug)]
pub struct FundForm {
    pub editing: Option<FundId>,
    pub focus: FundField,
    name: Field,
    share: Field,
    actual: Field,
    kind: usize,
}

impl FundForm {
    pub fn add() -> FundForm {
        FundForm {
            editing: None,
            focus: FundField::Name,
            name: Field::default(),
            share: Field::default(),
            actual: Field::default(),
            kind: 0,
        }
    }

    pub fn edit(fund: &Fund) -> FundForm {
        FundForm {
            editing: Some(fund.id),
            focus: FundField::Name,
            name: Field::given(fund.name.clone()),
            share: Field::given(match fund.target {
                Target::AgeOver30 => String::new(),
                Target::RemainderShare(share) => share.to_string(),
            }),
            actual: Field::given(fund.actual.to_string()),
            kind: Target::KINDS
                .iter()
                .position(|k| k.kind_str() == fund.target.kind_str())
                .unwrap_or(0),
        }
    }

    /// Which variant the selector is on. The share it carries is the
    /// placeholder from `Target::KINDS`; the real one comes from the field.
    pub fn target_kind(&self) -> Target {
        Target::KINDS[self.kind]
    }

    /// The fields this form shows, which is also its tab order.
    pub fn fields(&self) -> Vec<FundField> {
        match self.target_kind() {
            Target::AgeOver30 => vec![FundField::Name, FundField::Kind, FundField::Actual],
            Target::RemainderShare(_) => vec![
                FundField::Name,
                FundField::Kind,
                FundField::Share,
                FundField::Actual,
            ],
        }
    }

    pub fn title(&self) -> &'static str {
        match self.editing {
            Some(_) => "Edit fund — Tab field · ←/→ target · Enter save · Esc cancel",
            None => "Add fund — Tab field · ←/→ target · Enter save · Esc cancel",
        }
    }

    pub fn display(&self, field: FundField) -> Label {
        Label::plain(match field {
            FundField::Name => crate::demo::text(self.name.value()).into_owned(),
            FundField::Share => self.share.value().to_string(),
            FundField::Actual => crate::demo::typed(self.actual.value()),
            FundField::Kind => match self.target_kind() {
                Target::AgeOver30 => "tracks age".to_string(),
                Target::RemainderShare(_) => "share of the rest".to_string(),
            },
        })
    }

    /// Cycle one particular field's selector, whatever the focus is. The
    /// tests use it to reach the share row without pressing Tab first.
    pub fn next_choice_on(&mut self, field: FundField) {
        if field == FundField::Kind {
            self.kind = step_index(self.kind, Target::KINDS.len(), 1);
        }
    }

    pub fn commit(&self) -> Result<FundEdit> {
        let name = self.name.value().trim().to_string();
        ensure!(!name.is_empty(), "name must not be empty");
        let target = match self.target_kind() {
            Target::AgeOver30 => Target::AgeOver30,
            Target::RemainderShare(_) => Target::RemainderShare(parse_share(self.share.value())?),
        };
        Ok(FundEdit {
            name,
            target,
            // Whole dollars, refusing cents -- matching Savings and Planning.
            actual: form::parse_whole_amount(self.actual.value())?,
        })
    }
}

impl FormFields for FundForm {
    fn move_focus(&mut self, step: isize) {
        self.focus = next_in(&self.fields(), self.focus, step);
    }

    fn cycle(&mut self, step: Step) {
        self.kind = step_index(self.kind, Target::KINDS.len(), step.direction());
    }

    fn focused(&mut self) -> Focused<'_> {
        match self.focus {
            FundField::Name => Focused::Text(&mut self.name),
            FundField::Share => Focused::Text(&mut self.share),
            FundField::Actual => Focused::Text(&mut self.actual),
            FundField::Kind => Focused::Selector,
        }
    }
}

use super::widget::{field_stack, render_fields};
use super::{Chrome, render_table, right_header, style, whole_amount};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Cell, Row as TableRow};

/// A right-aligned percentage cell, or `—` where there is no figure.
fn percent(bp: Option<BasisPoints>) -> Cell<'static> {
    tinted_percent(bp, None)
}

/// The same cell in a color, for the one row that takes one.
///
/// Through `tui::tinted` rather than `Cell::style`, which would cover the
/// cell's padding as well as its figure -- and `row_highlight_style` is
/// patched over the row after its cells draw, so on the cursor row that
/// padding becomes a block of background the full width of the column. See
/// the tint invariant in `src/tui/CLAUDE.md`.
fn tinted_percent(bp: Option<BasisPoints>, color: Option<style::Color>) -> Cell<'static> {
    let text = bp.map_or_else(|| "—".to_string(), |bp| bp.to_string());
    super::tinted(TextLine::from(text).right_aligned(), color)
}

pub fn render_form(frame: &mut Frame, form: &mut FundForm) {
    let caret = form.caret();
    let lines = field_stack(
        &form.fields(),
        form.focus,
        caret,
        FundField::label,
        |f| form.display(f),
        &[],
    );
    render_fields(frame, form.title(), lines);
}

/// One row per fund, a bold `Total` under them. Returns the [`Viewport`] it
/// drew: the height `PageUp`/`PageDown` move by, and the row the next draw
/// starts from.
pub(super) fn render(frame: &mut Frame, area: Rect, funds: &Funds) -> Viewport {
    let mut rows: Vec<TableRow> = funds
        .rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let lowest = funds.furthest_down() == Some(i);
            let delta = match lowest {
                true => tinted_percent(r.delta, Some(style::NEGATIVE)),
                false => percent(r.delta),
            };
            let row = TableRow::new(vec![
                Cell::from(crate::demo::text(&r.name).into_owned()),
                percent(r.target),
                percent(Some(r.actual_share)),
                delta,
                whole_amount(r.actual),
            ]);
            match lowest {
                true => row.style(Style::default().add_modifier(Modifier::BOLD)),
                false => row,
            }
        })
        .collect();

    if rows.is_empty() {
        rows.push(TableRow::new(vec![Cell::from("press a to add a fund")]));
    } else {
        // Target and value only: the actual share of the whole is always
        // 100%, and a total delta is not a number that means anything.
        rows.push(
            TableRow::new(vec![
                Cell::from("Total"),
                percent(Some(funds.target_total())),
                Cell::from(""),
                Cell::from(""),
                whole_amount(funds.total()),
            ])
            .style(Style::default().add_modifier(Modifier::BOLD)),
        );
    }

    let header = TableRow::new(vec![
        Cell::from("Fund"),
        right_header("Target %"),
        right_header("Actual %"),
        right_header("Delta"),
        right_header("Actual Value"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Min(16),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(14),
    ];

    // The drawn count includes the `Total` row, so a long list scrolls to the
    // end of what is actually on screen. An empty table draws only its
    // placeholder, and selects nothing.
    let drawn = match funds.rows().is_empty() {
        true => 0,
        false => funds.rows().len() + 1,
    };
    render_table(
        frame,
        area,
        funds,
        Chrome::titled(funds.title()).header(header),
        &widths,
        rows,
        drawn,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fund::{Allocation, FundRow};
    use crate::test_support::walk_until;
    use crate::tui::MIN_WIDTH;
    use crate::tui::cursor::Scroll;
    use crate::tui::form::char_key;

    fn row(id: i64, name: &str, target: Option<i64>, actual_bp: i64, dollars: i64) -> FundRow {
        FundRow {
            id: FundId(id),
            name: name.to_string(),
            actual: Cents::from_dollars(dollars),
            target: target.map(BasisPoints),
            actual_share: BasisPoints(actual_bp),
            delta: target.map(|t| BasisPoints((t - actual_bp).max(0))),
        }
    }

    /// A three-row block, derived.
    fn allocation() -> Allocation {
        Allocation {
            rows: vec![
                row(1, "Bonds", Some(1_000), 1_666, 30_000),
                row(2, "International", Some(3_600), 3_333, 60_000),
                row(3, "Domestic", Some(5_400), 5_000, 90_000),
            ],
            total: Cents::from_dollars(180_000),
            target_total: BasisPoints::ONE,
            furthest_down: Some(2),
            age: Some(40),
        }
    }

    fn screen() -> Funds {
        let mut funds = Funds::new();
        funds.set_allocation(allocation());
        funds
    }

    #[test]
    fn each_row_carries_its_derived_columns() {
        let funds = screen();
        assert_eq!(funds.rows()[2].delta, Some(BasisPoints(400)));
        assert_eq!(funds.rows()[0].target, Some(BasisPoints(1_000)));
        assert_eq!(funds.furthest_down(), Some(2));
        assert_eq!(funds.total(), Cents::from_dollars(180_000));
    }

    #[test]
    fn basis_points_print_as_a_percentage_with_two_decimals() {
        assert_eq!(BasisPoints(3_456).to_string(), "34.56");
        assert_eq!(BasisPoints(725).to_string(), "7.25");
        assert_eq!(BasisPoints::ZERO.to_string(), "0.00");
        assert_eq!(BasisPoints::ONE.to_string(), "100.00");
    }

    /// The prompt is a question about a missing setting, so it stands exactly
    /// while a row has no target to show.
    #[test]
    fn the_screen_asks_for_a_birth_date_only_while_an_age_row_has_no_target() {
        assert!(!screen().needs_birth_date());

        let mut unset = Funds::new();
        unset.set_allocation(Allocation {
            rows: vec![row(1, "Bonds", None, 10_000, 30_000)],
            total: Cents::from_dollars(30_000),
            target_total: BasisPoints::ZERO,
            furthest_down: None,
            age: None,
        });
        assert!(unset.needs_birth_date());

        // No age row, no question: a table of share rows needs no birth date.
        let mut shares = Funds::new();
        shares.set_allocation(Allocation {
            rows: vec![row(1, "Domestic", Some(10_000), 10_000, 1)],
            total: Cents::from_dollars(1),
            target_total: BasisPoints::ONE,
            furthest_down: None,
            age: None,
        });
        assert!(!shares.needs_birth_date());
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut funds = screen();
        assert_eq!(funds.selected().unwrap().fund_id, FundId(1));
        funds.select_previous();
        assert_eq!(funds.selected().unwrap().fund_id, FundId(1));

        funds.select_last();
        assert_eq!(funds.selected().unwrap().fund_id, FundId(3));
        funds.select_next();
        assert_eq!(funds.selected().unwrap().fund_id, FundId(3));
    }

    #[test]
    fn a_shrinking_list_moves_the_selection_into_bounds() {
        let mut funds = screen();
        funds.select_last();

        funds.set_allocation(Allocation {
            rows: vec![row(1, "Bonds", Some(1_000), 10_000, 30_000)],
            total: Cents::from_dollars(30_000),
            target_total: BasisPoints(1_000),
            furthest_down: None,
            age: Some(40),
        });

        assert_eq!(funds.selected_index(), 0);
        assert_eq!(funds.selected().unwrap().fund_id, FundId(1));
    }

    #[test]
    fn an_empty_table_has_nothing_selected() {
        let mut funds = Funds::new();
        funds.set_allocation(Allocation {
            rows: Vec::new(),
            total: Cents::ZERO,
            target_total: BasisPoints::ZERO,
            furthest_down: None,
            age: None,
        });
        assert!(funds.selected().is_none());
    }

    /// An age row's target is not a number to type, so the form has no field
    /// for it -- and a share row's is.
    #[test]
    fn the_form_shows_a_share_field_only_for_a_share_row() {
        let mut form = FundForm::add();
        assert_eq!(
            form.fields(),
            vec![FundField::Name, FundField::Kind, FundField::Actual]
        );

        walk_until!(
            matches!(form.target_kind(), Target::RemainderShare(_)),
            form.next_choice_on(FundField::Kind)
        );
        assert_eq!(
            form.fields(),
            vec![
                FundField::Name,
                FundField::Kind,
                FundField::Share,
                FundField::Actual
            ]
        );
    }

    #[test]
    fn a_share_parses_as_a_percentage_into_basis_points() {
        assert_eq!(parse_share("40").unwrap(), BasisPoints(4_000));
        assert_eq!(parse_share("40.00").unwrap(), BasisPoints(4_000));
        assert_eq!(parse_share(" 0.5 ").unwrap(), BasisPoints(50));
        assert_eq!(parse_share("100").unwrap(), BasisPoints::ONE);
    }

    /// A share is a share of a remainder being divided up, so outside
    /// `0..=100` it would hand a fund more than there is to give.
    #[test]
    fn a_share_outside_zero_to_a_hundred_is_refused() {
        assert!(parse_share("-1").is_err());
        assert!(parse_share("101").is_err());
        assert!(parse_share("").is_err());
        assert!(parse_share("half").is_err());
    }

    #[test]
    fn the_form_commits_what_was_typed() {
        let mut form = FundForm::add();
        for c in "International".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        walk_until!(
            matches!(form.target_kind(), Target::RemainderShare(_)),
            form.choice(Step::NEXT)
        );
        form.next_field();
        for c in "40".chars() {
            form.edit(char_key(c));
        }
        form.next_field();
        for c in "60,000".chars() {
            form.edit(char_key(c));
        }

        let edit = form.commit().unwrap();
        assert_eq!(edit.name, "International");
        assert_eq!(edit.target, Target::RemainderShare(BasisPoints(4_000)));
        assert_eq!(edit.actual, Cents::from_dollars(60_000));
    }

    /// Matching Savings and Planning: a value is typed in whole dollars, and
    /// `1800.5` typed for `1800.50` is a typo rather than a rounding.
    #[test]
    fn the_value_field_refuses_cents() {
        let mut form = FundForm::add();
        for c in "Bonds".chars() {
            form.edit(char_key(c));
        }
        form.focus = FundField::Actual;
        for c in "30000.50".chars() {
            form.edit(char_key(c));
        }
        assert!(form.commit().is_err());
    }

    #[test]
    fn editing_a_fund_prefills_every_field() {
        let form = FundForm::edit(&crate::db::fund::Fund {
            id: FundId(2),
            name: "International".to_string(),
            ord: 1,
            target: Target::RemainderShare(BasisPoints(4_000)),
            actual: Cents::from_dollars(60_000),
        });
        assert_eq!(form.editing, Some(FundId(2)));
        assert_eq!(form.display(FundField::Name).plain_text(), "International");
        assert_eq!(form.display(FundField::Share).plain_text(), "40.00");
        assert_eq!(form.display(FundField::Actual).plain_text(), "60,000.00");
    }

    /// The value and the name are what a demo has to hide; the share is not,
    /// since a fund's allocation is the shape of the portfolio rather than a
    /// sum, and scrambling it would hide the one thing this screen is worth
    /// demonstrating.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_a_funds_value_and_keeps_its_share() {
        crate::demo::install_with_salt(7);
        let form = FundForm::edit(&crate::db::fund::Fund {
            id: FundId(2),
            name: "International".to_string(),
            ord: 1,
            target: Target::RemainderShare(BasisPoints(4_000)),
            actual: Cents::from_dollars(60_000),
        });
        let drawn = form.display(FundField::Actual).plain_text();
        assert_ne!(drawn, "60,000.00");
        assert_eq!(drawn.len(), "60,000.00".len());
        assert_eq!(form.display(FundField::Share).plain_text(), "40.00");
        let drawn_name = form.display(FundField::Name).plain_text();
        assert_ne!(drawn_name, "International");
        assert_eq!(drawn_name, crate::demo::text("International"));
    }

    /// Five columns at `MIN_WIDTH`, all read for one test.
    fn drawn(funds: &Funds) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 9)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), funds);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..9)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// The one colored cell on this screen, held to the same rule as every
    /// other: a `Cell`'s own style covers its padding, and
    /// `row_highlight_style` is patched over the row *after* its cells draw,
    /// so on the cursor row that padding becomes a block of background the
    /// full width of the column. The furthest-down row is both the red one
    /// and, here, the one under the cursor -- which is exactly when it shows.
    #[test]
    fn the_furthest_down_delta_tints_its_figure_and_not_its_padding() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut funds = screen();
        // `furthest_down` is the third row, so park the cursor on it.
        funds.select_last();
        assert_eq!(funds.selected_index(), funds.furthest_down().unwrap());

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 9)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &funds);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let (y, line) = (0..9u16)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains("Domestic"))
            .expect("no Domestic row");

        // The delta figure carries the red. Located past the Actual column,
        // because `4.00` also occurs inside the `54.00` two columns to its
        // left and a plain search would find that one.
        let past_actual = line.find("50.00").expect("no Actual column") + "50.00".len();
        let rel = line[past_actual..].find("4.00").expect("no Delta column");
        let at = line[..past_actual + rel].chars().count() as u16;
        assert_eq!(buffer[(at, y)].fg, style::NEGATIVE, "{line:?}");
        // ...and nothing that is not a character carries anything.
        for x in 0..MIN_WIDTH {
            if buffer[(x, y)].symbol() == " " {
                assert_eq!(
                    buffer[(x, y)].fg,
                    style::Color::Reset,
                    "padding at column {x} is tinted: {line:?}"
                );
            }
        }
    }

    /// A right-aligned cell that gets truncated loses its *leading*
    /// characters, so a column one short turns a figure into a smaller
    /// figure rather than an ellipsis.
    #[test]
    fn every_column_fits_the_minimum_width() {
        let lines = drawn(&screen());
        let table = lines.join("\n");
        for expected in [
            "Bonds",
            "10.00",
            "16.66",
            "0.00",
            "30,000",
            "International",
            "36.00",
            "33.33",
            "Domestic",
            "4.00",
            "90,000",
            "Total",
            "100.00",
            "180,000",
        ] {
            assert!(table.contains(expected), "{expected:?} is cut off: {table}");
        }
    }

    /// Four of the five columns are right-aligned, so their headers must end
    /// where their figures do.
    #[test]
    fn the_right_aligned_headers_end_where_their_own_columns_do() {
        let lines = drawn(&screen());
        let header = super::super::ends_in_order(
            &lines[1],
            &["Fund", "Target %", "Actual %", "Delta", "Actual Value"],
        );
        let row =
            super::super::ends_in_order(&lines[2], &["Bonds", "10.00", "16.66", "0.00", "30,000"]);
        for column in 1..=4 {
            assert_eq!(
                header[column], row[column],
                "column {column} of {:?}",
                lines[2]
            );
        }
    }

    /// An age row with no birth date has no target to print, and a blank cell
    /// would read as zero.
    #[test]
    fn an_age_row_with_no_birth_date_draws_a_dash_for_its_target() {
        let mut funds = Funds::new();
        funds.set_allocation(Allocation {
            rows: vec![row(1, "Bonds", None, 10_000, 30_000)],
            total: Cents::from_dollars(30_000),
            target_total: BasisPoints::ZERO,
            furthest_down: None,
            age: None,
        });
        assert!(drawn(&funds).join("\n").contains("—"));
    }

    #[test]
    fn an_empty_table_still_draws_its_headers_and_says_how_to_add_a_fund() {
        let mut funds = Funds::new();
        funds.set_allocation(Allocation {
            rows: Vec::new(),
            total: Cents::ZERO,
            target_total: BasisPoints::ZERO,
            furthest_down: None,
            age: None,
        });
        let table = drawn(&funds).join("\n");
        assert!(table.contains("Target %"), "{table}");
        assert!(table.contains("press a to add a fund"), "{table}");
    }
}
