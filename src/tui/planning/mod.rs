//! The Planning screen: `Planning!C1:G41` as a flat list of rows.
//!
//! View state only -- no ratatui above the render functions at the bottom, and
//! no `Db` on the type. `App` runs the queries and hands a [`View`] in.
//!
//! Five modules over one screen, split the way `plan_rows` already splits the
//! subject: [`target`] is what a constant may be edited to, [`view`] is the
//! waterfall as rows, this file is the screen those rows sit in and the
//! cursor that walks them, and [`bill`] and [`confirm`] are the two modals it
//! opens.

mod bill;
mod confirm;
mod target;
#[cfg(test)]
mod test_support;
mod view;

pub use bill::{BillField, BillForm, render_bill};
pub use confirm::{TransferConfirm, render_transfers};
pub use target::Target;
use view::build;
pub use view::{Column, Editable, Row, Tint, View};

use crate::money::Cents;
use crate::tui::cursor::{Cursor, Scroll, Viewport};
use crate::tui::widget::centered;
use crate::tui::{Chrome, render_table};
use anyhow::Result;
use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Clear, Paragraph, Row as TableRow, Wrap};

/// The Planning screen's view state.
pub struct Planning {
    rows: Vec<Row>,
    excess_actual: Cents,
    transfer_detail: Vec<String>,
    pinned: Option<Cents>,
    pinned_at: Option<NaiveDate>,
    /// Why the screen cannot be drawn, when it cannot. A database with no Everyday
    /// checking account has no waterfall, and every other screen still works
    /// there, so the failure lands here rather than on the reload.
    message: Option<String>,
    cursor: Cursor,
}

impl Planning {
    pub fn new() -> Planning {
        Planning {
            rows: Vec::new(),
            excess_actual: Cents::ZERO,
            transfer_detail: Vec::new(),
            pinned: None,
            pinned_at: None,
            message: None,
            cursor: Cursor::new(),
        }
    }

    /// Rebuild every row, keeping the cursor on the constant it was on.
    ///
    /// By target rather than by index: `e` writes and `App` reloads, and a
    /// cursor that reset to the top on every keystroke would make editing
    /// three constants in a row unusable. A target that no longer exists --
    /// a deleted bill -- falls back to the first editable row.
    pub fn set_view(&mut self, view: View) -> Result<()> {
        let held = self.selected_editable();
        self.rows = build(&view)?;
        self.excess_actual = view.plan.excess_actual;
        self.transfer_detail = view.transfer_detail;
        self.pinned = view.pinned;
        self.pinned_at = view.pinned_at;
        self.message = None;
        self.cursor.select(
            held.and_then(|e| self.rows.iter().position(|r| r.editable == Some(e)))
                .unwrap_or(0),
        );
        if self
            .rows
            .get(self.cursor.index())
            .is_none_or(|r| r.editable.is_none())
        {
            self.select_first();
        }
        Ok(())
    }

    /// Why the plan is unresolved, in full, or empty when it resolves.
    pub fn transfer_detail(&self) -> &[String] {
        &self.transfer_detail
    }

    pub fn set_unavailable(&mut self, message: String) {
        self.rows.clear();
        self.transfer_detail.clear();
        self.cursor.first();
        self.message = Some(message);
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn title(&self) -> &'static str {
        "Planning"
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows
            .get(self.cursor.index())
            .filter(|r| r.editable.is_some())
    }

    pub fn selected_editable(&self) -> Option<Editable> {
        self.selected().and_then(|r| r.editable)
    }

    /// The constant the cursor is on, for the callers that only deal in
    /// constants. `None` on a destination row, which `e` opens a list for
    /// rather than a field.
    pub fn selected_target(&self) -> Option<Target> {
        match self.selected_editable() {
            Some(Editable::Constant(target)) => Some(target),
            _ => None,
        }
    }

    pub fn excess_actual(&self) -> Cents {
        self.excess_actual
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }

    /// How the pin reads on screen, or `None` when the plan is not pinned.
    ///
    /// The date is omitted when it is absent: import transcribes
    /// `Planning!D3` into the pin and has no date to transcribe with it, so an
    /// imported pin is dateless and must render rather than fail.
    pub fn pin_line(&self) -> Option<String> {
        let pinned = self.pinned?;
        let mut line = format!("pinned {}", crate::demo::figure(pinned));
        if let Some(at) = self.pinned_at {
            line.push_str(&format!(" on {at}"));
        }
        let drift = self.excess_actual - pinned;
        if drift != Cents::ZERO {
            line.push_str(&format!(
                " · excess has since moved {}",
                crate::demo::figure(drift)
            ));
        }
        Some(line)
    }

    fn editable_at_or_after(&self, from: usize) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, r)| r.editable.is_some())
            .map(|(index, _)| index)
    }

    fn editable_at_or_before(&self, from: usize) -> Option<usize> {
        self.rows
            .get(..=from)?
            .iter()
            .rposition(|r| r.editable.is_some())
    }
}

/// Planning is the one screen whose rows are not all selectable: barely a
/// third carry a `Target`, and the rest are computed lines the cursor must
/// skip. Every movement is therefore overridden — the shared cursor still
/// holds the index and the page height, but it is never told a row count it
/// could move over freely.
impl Scroll for Planning {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// Unused: every movement below is overridden. Reported honestly anyway,
    /// so a default that starts being used cannot silently see zero.
    fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The run of rows above the cursor that it cannot rest on -- a block's
    /// heading and the computed lines under it, and above `Target` the whole
    /// transfers block, which is the top of the screen and the part of it the
    /// owner acts on. The view is the only thing that can reach any of them,
    /// so they come with the row below them: `Up` from `Target` moves the
    /// cursor nowhere, and a list left scrolled past the transfers would stay
    /// there for the rest of the session.
    fn context_row(&self) -> usize {
        let mut row = self.cursor.index().min(self.rows.len());
        while row > 0 && self.rows[row - 1].editable.is_none() {
            row -= 1;
        }
        row
    }

    /// The run of rows below the *last* editable one -- the Destinations
    /// block's own computed lines, and however many of them the screen ends
    /// with. Every other unreachable run comes into view with the editable
    /// row beneath it, so only this one has nothing to travel with: the
    /// cursor stops above it, and the view would stop with the cursor. How
    /// long the run is depends on the plan -- a line whose destination is
    /// unset draws one row fewer to rest on -- so it is not a length the
    /// screen may assume fits inside the scroll margin.
    fn tail_row(&self) -> usize {
        let index = self.cursor.index();
        let editable_below = self
            .rows
            .iter()
            .skip(index + 1)
            .any(|r| r.editable.is_some());
        if editable_below {
            index
        } else {
            self.rows.len().saturating_sub(1).max(index)
        }
    }

    fn select_next(&mut self) {
        if let Some((index, _)) = self
            .rows
            .iter()
            .enumerate()
            .skip(self.cursor.index() + 1)
            .find(|(_, r)| r.editable.is_some())
        {
            self.cursor.select(index);
        }
    }

    fn select_previous(&mut self) {
        if let Some((index, _)) = self.rows[..self.cursor.index()]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.editable.is_some())
        {
            self.cursor.select(index);
        }
    }

    fn select_first(&mut self) {
        self.cursor
            .select(self.editable_at_or_after(0).unwrap_or(0));
    }

    fn select_last(&mut self) {
        let index = self
            .rows
            .iter()
            .rposition(|r| r.editable.is_some())
            .unwrap_or(0);
        self.cursor.select(index);
    }

    /// A page is a screenful of *lines*, as `page_height` is measured: the
    /// cursor moves that far down the list and then settles on the nearest
    /// editable row below where it landed.
    ///
    /// Stepping `page_height` *editable* rows instead would be `End` in
    /// disguise — barely a third of these rows are editable, so on any
    /// terminal taller than about twenty lines a page exceeds the whole
    /// editable count.
    fn page_down(&mut self) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let landing = (self.cursor.index() + self.cursor.page_height()).min(last);
        match self.editable_at_or_after(landing) {
            Some(index) => self.cursor.select(index),
            None => self.select_last(),
        }
    }

    fn page_up(&mut self) {
        let landing = self
            .cursor
            .index()
            .saturating_sub(self.cursor.page_height());
        match self.editable_at_or_before(landing) {
            Some(index) => self.cursor.select(index),
            None => self.select_first(),
        }
    }
}

impl Default for Planning {
    fn default() -> Planning {
        Planning::new()
    }
}

/// The long form of a failure the screen only had a table cell for.
///
/// Wrapped rather than truncated, and sized to what it holds: an empty line
/// in `lines` is a paragraph break, and every other line wraps inside the
/// panel's width.
pub fn render_details(frame: &mut Frame, title: &str, lines: &[String]) {
    const WIDTH: u16 = 68;
    let inner = WIDTH.saturating_sub(2).max(1);
    let wrapped: u16 = lines
        .iter()
        .map(|l| (l.chars().count() as u16).div_ceil(inner).max(1))
        .sum();
    let height = (wrapped + 3).min(frame.area().height);
    let area = centered(frame.area(), WIDTH, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(
            lines
                .iter()
                .map(|l| TextLine::from(l.clone()))
                .collect::<Vec<_>>(),
        )
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(format!("{title} — Esc close"))),
        area,
    );
}

/// The waterfall, top to bottom, with the pin line beneath it. Returns the
/// [`Viewport`] it drew: the height `PageUp`/`PageDown` move by, and the row
/// the next draw starts from.
pub(super) fn render(frame: &mut Frame, area: Rect, planning: &Planning) -> Viewport {
    if let Some(message) = planning.message() {
        frame.render_widget(
            Paragraph::new(TextLine::from(message.to_string()))
                .block(Block::bordered().title(planning.title())),
            area,
        );
        return Viewport::of_height(1);
    }

    let pin = planning.pin_line();
    let footer_height = if pin.is_some() { 3 } else { 0 };
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(footer_height)]).areas(area);

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let rows: Vec<TableRow> = planning
        .rows()
        .iter()
        .map(|r| {
            let tint = |column| Tint::color_in(r.account, column);
            let row = TableRow::new(vec![
                super::tinted(TextLine::from(r.label.clone()), tint(Column::Label)),
                super::tinted(
                    TextLine::from(r.value.clone()).right_aligned(),
                    // Tone first: red says this plan will not run and amber
                    // says there is a gap worth filling, and neither may be
                    // displaced by a tint that only says which account.
                    super::style::tone_color(r.tone).or_else(|| tint(Column::Value)),
                ),
                super::tinted(
                    TextLine::from(r.extra.clone()).right_aligned(),
                    // Tone first, the same precedence the value column
                    // takes: a gap the money will not cover outranks which
                    // account it lands in.
                    super::style::tone_color(r.extra_tone).or_else(|| tint(Column::Extra)),
                ),
            ]);
            if r.bold { row.style(bold) } else { row }
        })
        .collect();
    // Both fixed columns are sized for names rather than for figures: the
    // value column carries a goal's ("Home Down Payment"), and the extra
    // column carries either its container or a suggested goal's name with a
    // question mark after it -- two characters longer again. Truncation here
    // is not a visible ellipsis but a silently missing prefix, so the room
    // comes out of the label column, which has the `Min` and every spare
    // column the terminal is wider than.
    let widths = [
        Constraint::Min(22),
        Constraint::Length(24),
        Constraint::Length(24),
    ];

    let viewport = render_table(
        frame,
        table_area,
        planning,
        Chrome::titled(planning.title()),
        &widths,
        rows,
        planning.rows().len(),
    );

    if let Some(pin) = pin {
        frame.render_widget(
            Paragraph::new(TextLine::from(pin)).block(Block::bordered().title("Pinned")),
            footer_area,
        );
    }

    viewport
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::{AccountId, BillId};
    use crate::money::Cents;
    use crate::plan_line::Line;

    use crate::test_support::{day, walk_until};

    use crate::tui::MIN_WIDTH;

    use crate::tui::style::Tone;

    use super::test_support::*;

    /// A marked date is 11 characters in a column sized for goal names, so
    /// it fits with room to spare -- but a truncated date is a *different*
    /// date rather than a visible ellipsis, and the column's width is set
    /// for other content entirely, so nothing but this says the date still
    /// fits. Drawn at `MIN_WIDTH`, where the column is narrowest.
    #[test]
    fn a_scrubbed_date_is_whole_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut v = view(None, None);
        v.scrubbed_adhoc = Some(day(2026, 8, 29));
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn: String = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(drawn.contains("2026-08-29*"), "{drawn}");
    }

    /// The tint reaches the screen, and reaches the container's name rather
    /// than the goal's. `render` puts the tone ahead of it on the value
    /// column -- red and amber carry instructions where a tint only says
    /// which account -- which no landing currently exercises, because every
    /// landing carrying an account is one that resolved.
    #[test]
    fn a_container_is_drawn_in_its_accounts_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // A dangling account key: red, and naming nothing that exists.
        let mut v = view(None, None);
        v.wiring = wiring();
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // The destination row, located by its label: the screen leads with
        // the transfers, which name accounts of their own, so a search over
        // the whole buffer would find one of those instead.
        // Both needles, because the transfer block above carries a row
        // under the same label -- the destination row is the one that also
        // names the container.
        let (y, line) = (0..height)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains(Line::MomAndDad.label()) && line.contains("Brokerage"))
            .expect("no Mom & Dad destination row");
        let fg = |needle: &str| buffer[(super::super::column_of(&line, needle), y)].fg;

        // `Mom & Dad` lands in Brokerage, whose container name is tinted.
        assert_eq!(
            fg("Brokerage"),
            super::super::style::account_color(AccountId(2), None),
            "{line:?}"
        );
        // And the goal's own name is not an account, so it stays plain.
        assert_eq!(fg("Mom & Dad"), ratatui::style::Color::Reset, "{line:?}");
    }

    /// The gap sits in a fixed-width, right-aligned cell, so a truncated one
    /// loses its *leading* characters -- a wrong width reads as a wrong
    /// number rather than as a visible ellipsis. Every other fixture covers
    /// its bills, so nothing else draws this cell at all. Drawn at
    /// `MIN_WIDTH`, where it is narrowest.
    #[test]
    fn the_gap_on_a_cut_line_is_whole_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(1_000)), None))
            .unwrap();
        let gap = transfer_line(&planning, Line::Bills).extra.clone();
        assert!(!gap.is_empty(), "the fixture covers its bills after all");

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .any(|l| l.contains(&gap));

        assert!(drawn, "{gap:?} truncated on the cut line");
    }

    /// A gap is what the block reports rather than states, so it is drawn in
    /// the negative color wherever it lands -- while the figure the plug
    /// actually *moves*, two rows above it, stays plain: that figure is
    /// right, and the gap below it is what is not.
    #[test]
    fn the_gap_below_the_goals_line_is_drawn_in_the_negative_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut v = view(None, None);
        v.spread_ask_total = v.plan.lines.goals + Cents::from_dollars(220);
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let drawn = |needle: &str| {
            (0..height)
                .map(|y| {
                    (
                        y,
                        (0..MIN_WIDTH)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>(),
                    )
                })
                .find(|(_, line)| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} was never rendered"))
        };

        let (y, line) = drawn("\u{394} -220");
        assert_eq!(
            buffer[(super::super::column_of(&line, "\u{394} -220"), y)].fg,
            super::super::style::tone_color(Tone::Negative).expect("negative has a color"),
            "{line:?}"
        );

        // The Goals line itself is the first "Goals" the screen draws: the
        // transfers block heads it, and the Split and Destinations blocks
        // that repeat the label are both below.
        let (y, line) = drawn("Goals");
        assert_eq!(
            buffer[(super::super::column_of(&line, "Goals"), y)].fg,
            ratatui::style::Color::Reset,
            "{line:?}"
        );
    }

    /// The tint reaches the transfer's account name and stops at the indent
    /// in front of it. Planning indents its labels to show nesting, and an
    /// indent is structure rather than content -- a colored run of leading
    /// spaces is invisible until something reverses the row, and then it is
    /// a block of background sitting in front of the name.
    #[test]
    fn a_transfer_label_is_tinted_from_its_first_glyph() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let planning = screen();
        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let (y, line) = (0..height)
            .map(|y| {
                (
                    y,
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>(),
                )
            })
            .find(|(_, line)| line.contains("Rainy Day") && !line.contains("spread"))
            .expect("no Rainy Day transfer row");

        let at = super::super::column_of(&line, "Rainy Day");
        let expected = super::super::style::account_color(AccountId(1), None);
        assert_eq!(buffer[(at, y)].fg, expected, "{line:?}");
        // The indent in front of it is untinted, and so is everything before.
        for x in 0..at {
            assert_eq!(
                buffer[(x, y)].fg,
                super::super::style::Color::Reset,
                "column {x} of {line:?} is tinted"
            );
        }
    }

    /// Half the screen is labels and computed figures. A cursor that could
    /// land on one would make `e` do nothing, with no way to tell that from a
    /// key that failed.
    #[test]
    fn the_cursor_only_ever_lands_on_an_editable_row() {
        let mut planning = screen();
        assert!(planning.selected().unwrap().editable.is_some());

        let mut seen = 0;
        for _ in 0..planning.rows().len() * 2 {
            planning.select_next();
            assert!(
                planning.selected().unwrap().editable.is_some(),
                "the cursor stopped on {:?}",
                planning.selected().unwrap().label
            );
            seen += 1;
        }
        assert!(seen > 0);
    }

    #[test]
    fn the_first_editable_row_is_selected_when_the_screen_loads() {
        let planning = screen();
        assert_eq!(planning.selected_target(), Some(Target::Target));
    }

    /// The destination block sits below the split, so the end of the screen
    /// is the last line whose destination `e` can point somewhere -- the
    /// Emergency Fund row below it shares its key with a gate, and the two
    /// account lines below that are display only.
    #[test]
    fn the_last_editable_row_is_the_last_editable_destination() {
        let mut planning = screen();
        planning.select_last();
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// `select_previous` is the one cursor mover that slices `rows[..selected]`
    /// -- the one place an off-by-one would panic rather than merely
    /// misbehave. Walking all the way back must stop on the first editable
    /// row rather than wrapping or panicking at the start of the list.
    #[test]
    fn repeated_select_previous_walks_back_to_the_first_editable_row_and_stops() {
        let mut planning = screen();
        planning.select_last();
        for _ in 0..planning.rows().len() {
            planning.select_previous();
        }
        assert_eq!(planning.selected_target(), Some(Target::Target));
    }

    /// The far end of the same invariant: `select_next` past the last
    /// editable row is a no-op, not a wrap back to the top.
    #[test]
    fn repeated_select_next_past_the_last_editable_row_stays_put() {
        let mut planning = screen();
        planning.select_last();
        for _ in 0..3 {
            planning.select_next();
        }
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// `page_height` is a count of *lines*, so a page has to move that far
    /// down the list and settle on the nearest editable row. Twenty lines from
    /// the top of this screen is the bill-payment block; twenty *editable*
    /// rows would be off the end of a list that has twenty-two of them,
    /// making `PageDown` an `End` in disguise.
    #[test]
    fn paging_down_moves_a_screenful_of_lines_not_the_whole_list() {
        let mut planning = screen();
        planning.record_viewport(Viewport::of_height(20));

        planning.page_down();
        assert_eq!(planning.selected_target(), Some(Target::BillPaymentPct));

        planning.page_down();
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// The same, upwards: one page back from the last destination is the
    /// bill block, not the top of the screen.
    #[test]
    fn paging_up_moves_a_screenful_of_lines_back() {
        let mut planning = screen();
        planning.record_viewport(Viewport::of_height(20));
        planning.select_last();

        planning.page_up();

        assert_eq!(planning.selected_target(), Some(Target::Bill(BillId(6))));
    }

    /// Paging past either end stops there rather than panicking or wrapping,
    /// the same guarantee the single-step movers give.
    #[test]
    fn paging_past_either_end_stops_at_that_end() {
        let mut planning = screen();
        planning.record_viewport(Viewport::of_height(20));

        for _ in 0..10 {
            planning.page_up();
        }
        assert_eq!(planning.selected_target(), Some(Target::Target));

        for _ in 0..10 {
            planning.page_down();
        }
        assert_eq!(
            planning.selected_editable(),
            Some(Editable::Destination(Line::MomAndDad))
        );
    }

    /// The Transfers block at the top and everything below the split are the
    /// widest the screen gets: the labels are the longest ("Current
    /// Housing", "Emergency Fund") and the deepest indented, and the
    /// Destinations block puts a goal's name and its container into two
    /// fixed-width columns that were sized for figures. A right-aligned cell
    /// that gets truncated loses its *leading* characters, which turns a
    /// wrong width into a wrong number rather than a visible ellipsis, so
    /// every cell has to survive whole, not just the labels. Every row is
    /// checked, since the cheap parts of the screen cost nothing to cover.
    ///
    /// "Emergency Fund" also labels the Gates row and "  Future Housing" the
    /// Split row, both rendered on every load, so a bare substring search
    /// over the whole screen would pass on the strength of either of those
    /// even if the copy below the split were the one cut short. Each row is
    /// checked against the specific buffer line it rendered on, located by
    /// mapping `planning.rows()`'s index to the buffer through the model's
    /// own "Destinations" heading -- the one label the screen carries exactly
    /// once, which is what an anchor has to be.
    #[test]
    fn every_row_renders_whole_at_the_minimum_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let v = view(Some(Cents::from_dollars(17_500)), None);
        let mut planning = Planning::new();
        planning.set_view(v).unwrap();

        // Tall enough for every row plus both borders and the pinned footer,
        // so nothing scrolls out of view before the assertions below run.
        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();

        let model_rows = planning.rows();
        let heading_idx = model_rows
            .iter()
            .position(|r| r.label == "Destinations")
            .expect("no Destinations heading");
        let heading_y = lines
            .iter()
            .position(|l| l.contains("Destinations"))
            .expect("no Destinations row rendered");
        let offset = heading_y - heading_idx;

        // The Destinations block runs from its own heading to the foot of
        // the screen -- the same boundaries `build` draws it with.
        let block_start = heading_idx + 1;
        let block_end = model_rows.len();
        assert!(block_start < block_end, "nothing rendered below the split");

        let targets = [
            Line::CurrentHousing.label(),
            Line::EmergencyFund.label(),
            Line::FutureHousing.label(),
        ];
        let mut found = [false; 3];
        for (i, row) in model_rows.iter().enumerate() {
            let below_the_split = (block_start..block_end).contains(&i);
            let label = row.label.trim();
            let line = &lines[offset + i];
            assert!(
                line.contains(label),
                "{label:?} truncated on its own rendered row:\n{line}"
            );
            for cell in [&row.value, &row.extra] {
                if !cell.is_empty() {
                    assert!(
                        line.contains(cell.as_str()),
                        "{cell:?} truncated on its own rendered row:\n{line}"
                    );
                }
            }
            for (target, seen) in targets.iter().zip(found.iter_mut()) {
                *seen |= below_the_split && label == *target;
            }
        }
        for (target, seen) in targets.iter().zip(found.iter()) {
            assert!(seen, "{target:?} rendered nowhere below the split");
        }
    }

    /// `e` writes and `App` reloads. A cursor that reset to the top on every
    /// keystroke would make editing three constants in a row unusable.
    #[test]
    fn the_cursor_stays_on_its_target_across_a_reload() {
        let mut planning = screen();
        planning.select_next();
        planning.select_next();
        let held = planning.selected_target();
        assert_eq!(held, Some(Target::PeriodsPerYear));

        planning
            .set_view(view(Some(Cents::from_dollars(17_500)), None))
            .unwrap();

        assert_eq!(planning.selected_target(), held);
    }

    /// A bill deleted out from under the cursor has no row to return to.
    #[test]
    fn a_cursor_on_a_deleted_bill_falls_back_to_the_first_editable_row() {
        let mut planning = screen();
        walk_until!(
            planning.selected_target() == Some(Target::Bill(BillId(6))),
            planning.select_next()
        );

        let mut without_wework = view(Some(Cents::from_dollars(17_500)), None);
        without_wework.other_bills.pop();
        planning.set_view(without_wework).unwrap();

        assert_eq!(planning.selected_target(), Some(Target::Target));
    }

    /// `p` pins the live figure, so the screen has to carry it.
    #[test]
    fn the_screen_carries_the_live_excess_for_the_pin_key() {
        let planning = screen();
        assert_eq!(planning.excess_actual(), Cents(1_750_075));
    }

    /// A database with no Everyday checking account has no waterfall. Every other
    /// screen still works there, so the failure belongs on this one.
    #[test]
    fn an_unavailable_screen_holds_a_message_and_no_rows() {
        let mut planning = screen();
        planning.set_unavailable("no Everyday cash account".to_string());
        assert!(planning.rows().is_empty());
        assert!(planning.selected().is_none());
        assert_eq!(planning.message(), Some("no Everyday cash account"));
    }

    /// The waterfall is a column of absolute figures, and every one of them
    /// is scrambled. The percentages that produced them are not: a split is a
    /// rule rather than a sum, and reading `22%` beside a scrambled figure is
    /// exactly what makes the screen worth demonstrating.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_waterfall_and_keeps_its_percentages() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn: String = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!drawn.contains("32,500"), "the checking balance survived");
        assert!(!drawn.contains("1,200"), "a bill survived");
        assert!(
            drawn.contains(&crate::demo::whole_figure(Cents::from_dollars(1_200))),
            "no scrambled bill found: {drawn}"
        );
        assert!(drawn.contains("%"), "the percentages must stay: {drawn}");
        assert!(drawn.contains("Bills"), "the labels must stay: {drawn}");
    }

    /// The Destinations block draws two more owner-text cells this way: a
    /// goal-backed line's value is the goal's own name (`wiring` hands it
    /// over real, in `Landing::Goal`), and the suggestion beside an unset
    /// line is too, `?` and all. Rendered end to end, the same way
    /// `a_demo_scrambles_the_waterfall_and_keeps_its_percentages` checks the
    /// waterfall above it, so this cannot pass over a cell nothing draws.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_goal_names_the_destinations_block_draws() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        crate::demo::install_with_salt(7);
        let mut planning = Planning::new();
        planning
            .set_view(view(Some(Cents::from_dollars(17_500)), None))
            .unwrap();

        let height = planning.rows().len() as u16 + 5;
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &planning);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn: String = (0..height)
            .map(|y| {
                (0..MIN_WIDTH)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            );

        assert!(
            !drawn.contains("Emergency Savings"),
            "a goal name survived: {drawn}"
        );
        assert!(
            drawn.contains(&crate::demo::text("Emergency Savings").to_string()),
            "no scrambled goal name found: {drawn}"
        );
        assert!(
            !drawn.contains("Home Down Payment?"),
            "a suggestion survived: {drawn}"
        );
        assert!(
            drawn.contains(&format!("{}?", crate::demo::text("Home Down Payment"))),
            "no scrambled suggestion found: {drawn}"
        );
    }
    /// The complaint this scroll rule was written for: on a terminal a dozen
    /// rows shorter than the screen, walking down to `Excess (Used)` -- the
    /// nearest editable constant below the transfers -- must not take the
    /// transfers off the top. There is room for both, and the block at the top
    /// is what the owner acts on.
    ///
    /// Drawn between every keystroke, the way the event loop does, since the
    /// viewport each draw reports is what the next one scrolls from.
    #[test]
    fn walking_down_to_the_pin_leaves_the_transfers_on_screen() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 32)).unwrap();
        let mut draw = |planning: &mut Planning| {
            let mut viewport = Viewport::default();
            terminal
                .draw(|frame| viewport = render(frame, frame.area(), planning))
                .unwrap();
            planning.record_viewport(viewport);
            let buffer = terminal.backend().buffer();
            (0..32)
                .map(|y| {
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        planning.select_first();
        let mut drawn = draw(&mut planning);
        walk_until!(
            planning.rows()[planning.selected_index()].label.trim() == "Excess (Used)",
            {
                planning.select_next();
                drawn = draw(&mut planning);
            }
        );

        assert!(drawn.contains("Transfers"), "{drawn}");
        assert!(drawn.contains("Excess (Used)"), "{drawn}");
    }
    /// The cursor cannot rest above `Target`: the transfers block, and the
    /// blank under it, carry nothing to edit. So walking back up from the
    /// bottom has to bring them into view anyway -- a screen whose top rows
    /// are the ones the owner acts on must not stay scrolled past them
    /// forever, and nothing but the view can reach them.
    #[test]
    fn walking_back_up_brings_the_transfers_into_view() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 32)).unwrap();
        let mut draw = |planning: &mut Planning| {
            let mut viewport = Viewport::default();
            terminal
                .draw(|frame| viewport = render(frame, frame.area(), planning))
                .unwrap();
            planning.record_viewport(viewport);
            let buffer = terminal.backend().buffer();
            (0..32)
                .map(|y| {
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        planning.select_last();
        let mut drawn = draw(&mut planning);
        assert!(!drawn.contains("Transfers"), "{drawn}");

        // Up until the cursor has nowhere left to go, which is the first
        // editable row rather than the first row.
        let mut previous = usize::MAX;
        walk_until!(planning.selected_index() == previous, {
            previous = planning.selected_index();
            planning.select_previous();
            drawn = draw(&mut planning);
        });

        assert_eq!(
            planning.rows()[planning.selected_index()].label.trim(),
            "Target"
        );
        assert!(drawn.contains("Transfers"), "{drawn}");
    }

    /// The mirror of the two above, and the end of the list rather than its
    /// start: the Destinations block ends in rows the cursor cannot rest on,
    /// so `End` leaves the selection above them and only the view can bring
    /// them down. On a terminal short enough to squeeze the scroll margin,
    /// following the cursor alone leaves the last of them off the bottom for
    /// good.
    #[test]
    fn walking_down_brings_the_last_destinations_into_view() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut planning = Planning::new();
        planning.set_view(view(None, None)).unwrap();

        let last = planning.rows().last().expect("no rows").label.clone();
        assert_eq!(last.trim(), Line::Investment.label());

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 8)).unwrap();
        let mut draw = |planning: &mut Planning| {
            let mut viewport = Viewport::default();
            terminal
                .draw(|frame| viewport = render(frame, frame.area(), planning))
                .unwrap();
            planning.record_viewport(viewport);
            let buffer = terminal.backend().buffer();
            (0..8)
                .map(|y| {
                    (0..MIN_WIDTH)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        planning.select_first();
        let mut drawn = draw(&mut planning);
        let mut previous = usize::MAX;
        walk_until!(planning.selected_index() == previous, {
            previous = planning.selected_index();
            planning.select_next();
            drawn = draw(&mut planning);
        });

        assert_eq!(
            planning.rows()[planning.selected_index()].label.trim(),
            Line::MomAndDad.label()
        );
        assert!(drawn.contains(last.trim()), "{drawn}");
    }
}
