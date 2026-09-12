//! The terminal UI.
//!
//! `ratatui` and `crossterm` are named only inside this directory, exactly as
//! `rusqlite` is confined to `src/db/` and `calamine` to `src/import/`.
//! Nothing in `calc`, `db`, `plan`, or `projection` learns that a terminal
//! exists.
//!
//! The seam is placed so that everything with a decision in it is a plain
//! type — `Overview`, `Ledger`, `TxnForm`, `TransferForm`, `Autocomplete` —
//! holding no ratatui state and unit-tested directly. The render functions
//! only draw.

mod account_label;
pub mod accounts;
pub mod app;
pub mod autocomplete;
mod cursor;
pub mod destination;
pub mod form;
pub mod fund;
pub mod goal_form;
mod help;
mod history;
pub mod ledger;
pub mod ledger_form;
mod modal;
pub mod month;
pub mod overview;
pub mod picker;
pub mod planning;
pub mod recurring_goal;
pub mod recurring_txn;
pub mod savings;
mod search;
pub mod style;
mod text;
mod widget;
pub mod worksheet;

use crate::account_label::{Account, Label};
use crate::db::Db;
use account_label::{account_cell, label_line};
use anyhow::{Result, ensure};
use app::App;
use chrono::NaiveDate;
use cursor::{Scroll, Viewport};
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{
    Block, Cell, Padding, Row as TableRow, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState,
};
use std::time::Duration;
use style::Color;

/// How long the loop waits for an event before looking at the clock again.
/// Nothing animates and every other change arrives as an event, so this
/// decides one thing: how closely a status message's expiry follows
/// [`app::STATUS_TTL`].
const TICK: Duration = Duration::from_millis(250);

/// The narrowest terminal the screens are laid out for.
///
/// Nothing enforces it at runtime -- a narrower terminal still draws, and
/// ratatui truncates whatever no longer fits. What the number is for is the
/// width tests: every screen renders one row of its widest plausible content
/// at this width and asserts nothing is cut. A constant rather than a literal
/// in each of them so the contract is stated once and the next retarget is a
/// one-line edit.
pub const MIN_WIDTH: u16 = 120;

/// The bigger step `Shift` with `←`/`→` takes on a date, in days.
///
/// A week is what reaches the middle of the fortnightly paycheck cycle in one
/// press and the cycle after it in two, which is what makes it the useful
/// second size on the one key that already means "move this date". Written
/// once, because the Overview scrub and every date field in every form take
/// the same step for the same reason.
pub const WEEK: i64 = 7;

use crate::money::Cents;

/// One share of `pot`, floored to a whole dollar.
///
/// The `/N` operator both screens offer: the worksheet divides its posted
/// amount across the selected lines, and the allocation form divides the
/// container's unallocated remainder. One function so the two cannot drift --
/// the same pot and the same divisor must give the same figure whichever
/// screen it is typed on.
///
/// `n` is typed, so a non-positive divisor is an error rather than a
/// `debug_assert!`; the forms surface it on the status line.
pub fn share_of(pot: Cents, n: i64) -> Result<Cents> {
    ensure!(n > 0, "cannot divide by {n}");
    Ok(Cents::from_dollars(pot.dollars() / n))
}

/// A money cell: right-aligned, and colored by [`style::amount_color`].
///
/// Every screen that renders a `Cents` goes through here, so "negative reads
/// red" is one decision rather than one per screen. Right alignment is not decoration
/// either -- a truncated right-aligned cell loses its *leading* characters, so
/// a column too narrow for its figures is visibly wrong rather than quietly
/// off by a digit.
fn amount(cents: Cents) -> Cell<'static> {
    money_cell(cents, crate::demo::figure(cents))
}

/// The same cell with the cents dropped before the color is chosen — see
/// [`Cents::trunc_to_dollar`].
///
/// The Savings screen's figures are targets and balances in the thousands,
/// where two decimal places are two digits of noise on every row. Its
/// `Unallocated` footer drops them too, through
/// [`crate::savings::unallocated`]: the $0.23 a container can sit at is
/// exactly the noise, and a line that reads `0` is the line saying there is
/// nothing there to place.
///
/// The color is chosen from the truncated figure and the text drawn through
/// [`crate::demo::truncated_figure`], which truncates past the key a demo
/// scrambles on: a cell truncating on its own way in would draw whole dollars
/// no other screen quoting that amount draws.
fn whole_amount(cents: Cents) -> Cell<'static> {
    money_cell(
        cents.trunc_to_dollar(),
        crate::demo::truncated_figure(cents),
    )
}

/// A header cell over a right-aligned column.
///
/// A right-aligned column's label belongs over its own figures rather than
/// over the padding to their left, and that padding is wide enough for a
/// left-aligned header to read as though it belonged to the column beside
/// it. Shared rather than inlined per screen so that
/// "right-aligned cells take a right-aligned header" is one decision instead
/// of one per screen, and so a new screen has something obvious to reach for.
fn right_header(text: &str) -> Cell<'static> {
    Cell::from(TextLine::from(text.to_string()).right_aligned())
}

fn money_cell(cents: Cents, text: String) -> Cell<'static> {
    tinted(
        TextLine::from(text).right_aligned(),
        style::amount_color(cents),
    )
}

/// A cell whose color sits on its *text* rather than on the cell.
///
/// A `Cell`'s own style covers its whole area, padding included, and a
/// table's `row_highlight_style` is patched over the row *after* the cells
/// have drawn -- so `Style::patch` leaves each cell's foreground in place and
/// `REVERSED` turns it into a background. A colored cell on the cursor row
/// therefore used to render as a solid block the full width of its column,
/// and two colored columns side by side read as one column of the wrong
/// width. Styling the span instead leaves the padding to the row: nothing
/// changes on an ordinary row, where padding has no glyphs to color, and on
/// the cursor row the color covers exactly the characters it belongs to.
///
/// The color goes on each *span*, not on the `Line`: a line's own style
/// fills its whole area exactly as a cell's does, so setting it there would
/// move the block rather than remove it.
///
/// The tint also starts at the first glyph rather than at the start of the
/// text. Planning indents its labels to show nesting, and an indent is
/// structure rather than content -- a colored run of leading spaces is
/// invisible until something reverses the row, and then it is a block of
/// background in front of the name. Same rule as the padding, stated once.
///
/// `None` leaves the line alone entirely rather than setting `Color::Reset`,
/// so a cell composes with the style its row already carries -- the ledger
/// dims rows dated after today.
fn tinted(mut line: TextLine<'static>, color: Option<Color>) -> Cell<'static> {
    let Some(color) = color else {
        return Cell::from(line);
    };
    // Split any leading indent off the first span, so the tint below starts
    // at the first glyph rather than at the start of the cell.
    let mut from = 0;
    if let Some(span) = line.spans.first() {
        let text = span.content.to_string();
        let indent = text.len() - text.trim_start().len();
        if indent > 0 {
            let style = span.style;
            line.spans[0] = Span::raw(text[..indent].to_string());
            line.spans
                .insert(1, Span::styled(text[indent..].to_string(), style));
            from = 1;
        }
    }
    for span in line.spans.iter_mut().skip(from) {
        span.style = span.style.patch(Style::default().fg(color));
    }
    Cell::from(line)
}

/// The same figure as a span, for the money that lands somewhere other than a
/// table cell — the ledgers' title carries a balance.
///
/// Reads [`style::amount_color`] rather than restating the rule, so a figure
/// in a border is red on the same terms as a figure in a column. No alignment
/// here: a span is placed by whatever line it joins.
///
/// This is the one figure in the app that prints a `$`, and the difference is
/// the setting rather than the money. A column of figures under a right-aligned
/// `Amount` header is already unmistakably money, and a `$` on all forty rows
/// is forty characters of noise; a lone figure in a title sits in prose, where
/// nothing else says what it is. The sign goes outside the `$` — `-$42.00`,
/// the way a ledger writes one.
fn money_span(cents: Cents) -> Span<'static> {
    let span = Span::raw(money_text(cents));
    match style::amount_color(cents) {
        Some(color) => span.style(Style::default().fg(color)),
        None => span,
    }
}

/// `-$42.00`: the text half of [`money_span`], for the figures that carry a
/// color of their own -- the ledgers' reconciliation delta, which is green
/// above its target where an ordinary amount is uncolored.
///
/// Moves the `$` inside the sign `Display` already wrote, rather than
/// re-deriving the figure from an absolute value: this cannot come to disagree
/// with `Cents`'s own formatting, and there is one place that decides where the
/// digits and their separators go.
fn money_text(cents: Cents) -> String {
    dollar(crate::demo::figure(cents))
}

/// [`money_span`] with the cents dropped, for a title over a table whose own
/// figures drop them -- the Funds screen's, where every balance in the list
/// below is a [`whole_amount`]. A title quoting cents over a column that does
/// not would be the one figure on the screen at another precision, and it
/// would not foot against the rows a reader adds up by eye.
///
/// The color comes off the truncated figure and the text off
/// [`crate::demo::truncated_figure`], for [`whole_amount`]'s reasons.
fn whole_money_span(cents: Cents) -> Span<'static> {
    let span = Span::raw(dollar(crate::demo::truncated_figure(cents)));
    match style::amount_color(cents.trunc_to_dollar()) {
        Some(color) => span.style(Style::default().fg(color)),
        None => span,
    }
}

/// A figure with the `$` inside whatever sign it already carries.
fn dollar(figure: String) -> String {
    match figure.strip_prefix('-') {
        Some(magnitude) => format!("-${magnitude}"),
        None => format!("${figure}"),
    }
}

/// A recurring goal entry's month, abbreviated for a table column.
fn month_name(month: i64) -> String {
    formatted_month(month, "%b")
}

/// The same month spelled out, for the recurring goal form's Month selector --
/// one field on a 64-column modal has the room a three-character column does
/// not, and a name is what the reader is picking between.
fn month_full_name(month: i64) -> String {
    formatted_month(month, "%B")
}

/// Blank rather than a panic for a month outside 1-12: the schema's `CHECK`
/// refuses one, and a corrupt row is not a reason to stop drawing the list.
fn formatted_month(month: i64, format: &str) -> String {
    u32::try_from(month)
        .ok()
        .and_then(|m| NaiveDate::from_ymd_opt(2000, m, 1))
        .map(|d| d.format(format).to_string())
        .unwrap_or_default()
}

/// The scroll state for a list of `rows` drawn in a viewport `height` tall,
/// and the [`Viewport`] the screen hands back to its cursor.
///
/// Every screen with a list resolves its offset here rather than keeping a
/// rule of its own, and the rule needs the offset the last draw settled on --
/// which is what the cursor holds it for. Handing `TableState` a bare default
/// instead would leave the offset at zero, and ratatui would scroll the
/// minimum needed to reveal the selection: pinning the cursor to the last
/// visible line, with the rest of the window showing only rows already behind
/// it. An empty list selects nothing, so the highlight does not sit on a row
/// that is not there.
fn table_state(list: &impl Scroll, rows: usize, height: usize) -> (TableState, Viewport) {
    let selected = list.selected_index();
    let offset = cursor::viewport_offset(
        list.cursor().offset(),
        cursor::Selection {
            context: list.context_row(),
            selected,
            tail: list.tail_row(),
        },
        rows,
        height,
    );
    let mut state = TableState::default();
    if rows > 0 {
        state.select(Some(selected));
        *state.offset_mut() = offset;
    }
    (state, Viewport { height, offset })
}

/// How many of the area's lines a header row takes, its margins included.
///
/// [`Chrome::header`] sets it on the row rather than reading it off one a
/// caller built, because ratatui exposes no reader for a `Row`'s height and
/// charges `top_margin + height + bottom_margin` for it. A header given a
/// margin of its own would leave [`Chrome::lines`] a line short, and the
/// scroll arithmetic would then offer the cursor a row that was never drawn.
const HEADER_LINES: u16 = 1;

/// What frames a list, and what that costs the rows inside it.
///
/// Two questions each screen was answering for itself, in the same two
/// subtractions: the bordered block takes two lines off the data rows and the
/// header row takes a third, and seven screens carried the same sentence
/// saying so beside seven copies of the arithmetic.
struct Chrome {
    /// The title of the block the table is drawn in, or `None` for a chooser
    /// handed an area someone else has already inset for a border.
    title: Option<TextLine<'static>>,
    /// The header row, or `None` on Planning, whose rows label themselves, and
    /// on the two choosers.
    header: Option<TableRow<'static>>,
}

impl Chrome {
    /// A list in a bordered block of its own.
    fn titled(title: impl Into<TextLine<'static>>) -> Chrome {
        Chrome {
            title: Some(title.into()),
            header: None,
        }
    }

    /// A list drawn into an area already inset by whoever owns the border,
    /// which [`scroll_track`] takes to be the column immediately past its
    /// rows -- so an area inset by anything else would put the scroll
    /// indicator over a row instead of over a border.
    fn bare() -> Chrome {
        Chrome {
            title: None,
            header: None,
        }
    }

    /// Label the columns, in [`HEADER_LINES`] of them.
    fn header(mut self, header: TableRow<'static>) -> Chrome {
        self.header = Some(header.height(HEADER_LINES).top_margin(0).bottom_margin(0));
        self
    }

    /// How many of `area`'s lines the chrome takes before any row is drawn.
    fn lines(&self) -> u16 {
        2 * u16::from(self.title.is_some()) + self.header_lines()
    }

    /// How many of them the header alone takes.
    fn header_lines(&self) -> u16 {
        self.header.as_ref().map_or(0, |_| HEADER_LINES)
    }

    /// The rectangle the data rows themselves occupy inside `area`.
    ///
    /// [`render_table`] hands the whole of `area` to ratatui and lets the
    /// table inset itself, so this is the same arithmetic read forwards --
    /// what [`lines`](Chrome::lines) subtracts, said as a `Rect`. It exists
    /// for [`scroll_track`], which wants the column the rows end at rather
    /// than the number of lines they cost.
    fn rows_area(&self, area: Rect) -> Rect {
        let border = u16::from(self.title.is_some());
        Rect {
            x: area.x + border,
            y: area.y + border + self.header_lines(),
            width: area.width.saturating_sub(2 * border),
            height: area.height.saturating_sub(self.lines()),
        }
    }
}

/// One list, drawn the way every list in the app is drawn, and the [`Viewport`]
/// the screen hands back to its cursor.
///
/// What is written here rather than per screen is everything a cursor has to
/// look the same for it to read as one cursor -- the reversed highlight, the
/// `> ` marker, and the [`Chrome`] the rows are fitted inside.
///
/// `drawn` is how many rows that cursor may travel over, which is not always
/// `rows.len()`: Accounts does not count the placeholder it draws in place of
/// an empty list.
///
/// What stays at the call sites is what each screen decides for itself: its
/// `widths`, which this directory's `CLAUDE.md` budgets per screen, and the
/// bolding of its own header.
fn render_table(
    frame: &mut Frame,
    area: Rect,
    list: &impl Scroll,
    chrome: Chrome,
    widths: &[Constraint],
    rows: Vec<TableRow<'static>>,
    drawn: usize,
) -> Viewport {
    let rows_area = chrome.rows_area(area);
    let (mut state, viewport) = table_state(list, drawn, usize::from(rows_area.height));

    let mut table = Table::new(rows, widths.iter().copied())
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    if let Some(header) = chrome.header {
        table = table.header(header);
    }
    // Even a bare list is framed, because the frame is what holds the rows
    // off the border to their right: a `Block` with no borders at all, and
    // `GUTTER` of padding.
    table = table.block(
        match chrome.title {
            Some(title) => Block::bordered().title(title),
            None => Block::new(),
        }
        .padding(Padding::right(GUTTER)),
    );
    frame.render_stateful_widget(table, area, &mut state);
    render_scrollbar(frame, scroll_track(rows_area), drawn, viewport);

    viewport
}

/// How far the rows are held off the border to their right, in columns.
///
/// A right-aligned cell ends at the last column its table is given, so
/// without this the largest `$/Pay` on the Savings screen sits against the
/// border -- and against the scroll thumb [`render_scrollbar`] draws over
/// it, where a figure reads as running into the mark beside it. It is spent
/// by every list whether or not that list is long enough to scroll, since a
/// column handed back the moment a filter shortened the list would reflow
/// the table under the eye that is reading it.
///
/// It comes out of the one column each screen gives a `Constraint::Min`,
/// which is where all of a wider terminal's slack already goes -- the fixed
/// columns are sized for their true content and must not be squeezed, for
/// the reason *How wide a screen is* gives. A row's own marks stop with it:
/// the selection bar and the favorite band run the width of the row, and
/// the row is now a column short of the border.
const GUTTER: u16 = 1;

/// Where a list's scroll indicator is drawn: the column immediately right of
/// its rows, for exactly the lines those rows occupy.
///
/// That column is a border on every list in the app, and the rule is one rule
/// because the two chromes put the border in the same place relative to the
/// rows rather than in the same place relative to `area`. A
/// [`Chrome::titled`] list is inset by its own block, so the column past its
/// rows is that block's right edge; a [`Chrome::bare`] one was handed an area
/// someone else already inset, so the column past its rows is the border that
/// caller drew. Neither costs the rows a column of its own: what the rows
/// give up is [`GUTTER`], the space between them and the mark, rather than
/// the mark itself.
fn scroll_track(rows: Rect) -> Rect {
    Rect {
        x: rows.right(),
        y: rows.y,
        width: 1,
        height: rows.height,
    }
}

/// A thumb on `track`, sized and placed by how much of `rows` the viewport
/// holds and where in them it starts.
///
/// Drawn only when there are rows the viewport does not hold, so an unbroken
/// border means the list is all on screen: the mark says "there is more this
/// way" and appears exactly when there is. Nothing else on a list screen says
/// so -- the scroll keys are undocumented on purpose, and a ledger of four
/// hundred rows looks like a ledger of twenty until the cursor runs off the
/// bottom.
///
/// The track symbol is the border's own `│` rather than the `║` a scrollbar
/// defaults to, and the arrow caps are off: the untravelled part of the track
/// has to read as the border it is drawn over, or a list that fits and a list
/// that does not would differ in the whole column instead of in the thumb.
///
/// `content` is the number of *offsets* the list has rather than its length,
/// which is what puts the thumb against the bottom of the track at the last
/// of them; the thumb's own length is then the share of the list on screen.
fn render_scrollbar(frame: &mut Frame, track: Rect, rows: usize, viewport: Viewport) {
    if viewport.height == 0 || rows <= viewport.height {
        return;
    }
    let track = track.intersection(frame.area());
    if track.is_empty() {
        return;
    }
    let content = rows - viewport.height + 1;
    let mut state = ScrollbarState::new(content).position(viewport.offset);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(ratatui::symbols::scrollbar::VERTICAL)
            .begin_symbol(None)
            .end_symbol(None),
        track,
        &mut state,
    );
}

/// The *column* each of `labels` ends at in `line`, matched left to right.
///
/// Sequential rather than independent, so a label that also occurs inside a
/// later one cannot match the wrong column -- the Savings header carries
/// `Goal`, a second `Goal`, and a `Goal Date`, and searching each
/// independently would find all three at the first.
///
/// The header alignment tests compare these against the same reading of a
/// data row: a right-aligned column and its header end in the same place.
///
/// Columns rather than bytes, for the reason [`column_of`] gives: the border
/// `│` is three bytes and one column, and so is the `—` a widened account
/// label carries. A header holding neither and a row holding one end in the
/// same column and two bytes apart, which is a real alignment reported as a
/// failure.
#[cfg(test)]
fn ends_in_order(line: &str, labels: &[&str]) -> Vec<usize> {
    let mut at = 0;
    labels
        .iter()
        .map(|label| {
            let start = at
                + line[at..]
                    .find(label)
                    .unwrap_or_else(|| panic!("no {label:?} past byte {at} of {line:?}"));
            at = start + label.len();
            line[..at].chars().count()
        })
        .collect()
}

/// The buffer *column* a needle starts at, for the tests that read a cell's
/// color back off a drawn row.
///
/// Not `str::find`, which answers in bytes: every screen draws inside a
/// border, and `│` is three bytes and one column. Reading the fg at a byte
/// offset lands two columns to the right of the word asked for, which for a
/// word longer than two characters is still inside it -- so the mistake
/// passes rather than failing, and the test stops saying what it claims to.
#[cfg(test)]
fn column_of(line: &str, needle: &str) -> u16 {
    let byte = line
        .find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in {line:?}"));
    line[..byte].chars().count() as u16
}

/// Run the application, and hand the database back when it quits.
///
/// Returning it is what lets `main` write the report after the terminal is
/// restored, beside the scheduled backup that already runs there -- so every
/// decision about what happens after a quit stays in one place, and `tui`
/// never learns that `config` exists.
///
/// `demo` scrambles the digits of every absolute figure the screens draw --
/// see [`crate::demo`]. Installed here, before the first frame, because it is a
/// constant of the run rather than state a screen can reach: nothing after
/// this point can turn it on or off.
///
/// `sec_contact` is `cfg.sec`'s contact line, read by `main` and handed down
/// rather than read here: `tui` does not otherwise name `config`, and a
/// second read site is a second place for the two to drift.
pub fn run(db: Db, today: NaiveDate, demo: bool, sec_contact: Option<String>) -> Result<Db> {
    crate::demo::install(demo);
    let mut app = App::new(db, today, sec_contact)?;
    // `try_init` enables raw mode, enters the alternate screen, and installs
    // a panic hook that restores the terminal before unwinding -- so a bug
    // leaves a working shell rather than a dead one.
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut app);
    ratatui::try_restore()?;
    result?;
    Ok(app.into_db())
}

/// Whether an event leaves the screen owing a redraw.
///
/// A key press always does, and deliberately without asking what the app did
/// with it: [`App::on_key`] clears the status message before it dispatches, so
/// even a key nothing is bound to takes the footer back. Answering per handler
/// would be a list to keep in step with every screen, and the cost of the
/// over-approximation is one wasted frame per unbound keystroke.
///
/// A resize does because every screen's layout is computed per frame.
/// Everything else -- a key release, a mouse event, focus crossing the
/// window -- reaches no handler here, so the frame it would earn would be the
/// frame already on screen.
fn redraws(event: &Event) -> bool {
    match event {
        Event::Key(key) => is_press(key),
        Event::Resize(..) => true,
        _ => false,
    }
}

/// Whether this is a key arriving rather than leaving.
///
/// Windows reports press and release both; acting on each would run every key
/// twice. Written once because [`redraws`] and the dispatch in the loop must
/// widen together: a key the app answers and the loop draws no frame for is a
/// stale screen, which is the failure the owed draw is not worth risking.
fn is_press(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
}

/// The loop that draws and reads keys, in that order.
///
/// The draw is owed rather than unconditional: at four frames a second an
/// idle app rebuilds every visible row's strings for a buffer ratatui is
/// about to find unchanged. What owes one is a key press, a resize, and a
/// status message reaching its expiry -- which is why the tick goes on firing
/// whether or not anything is drawn.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    // The first frame is owed to nothing in particular: there is no screen yet.
    let mut dirty = true;
    while !app.should_quit() {
        // Before the draw, so a message that has run out of time is gone from
        // the frame it would otherwise appear in for one more tick. It is the
        // one thing that changes the screen with no event behind it, which is
        // why it reports whether it did.
        dirty |= app.expire_status();
        if dirty {
            terminal.draw(|frame| app.render(frame))?;
            dirty = false;
        }
        if !event::poll(TICK)? {
            continue;
        }
        let event = event::read()?;
        dirty |= redraws(&event);
        if let Event::Key(key) = event
            && is_press(&key)
        {
            app.on_key(key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cents are dropped rather than carried: the allocation is a whole
    /// dollar figure, and what is left behind stays in the container's
    /// remainder where the Savings footer reports it.
    #[test]
    fn a_share_of_a_pot_floors_to_a_whole_dollar() {
        assert_eq!(
            share_of(Cents(260_017), 2).unwrap(),
            Cents::from_dollars(1300)
        );
        assert_eq!(
            share_of(Cents(260_017), 6).unwrap(),
            Cents::from_dollars(433)
        );
        assert_eq!(
            share_of(Cents(260_017), 12).unwrap(),
            Cents::from_dollars(216)
        );
    }

    #[test]
    fn a_whole_share_of_a_pot_is_the_pot_less_its_cents() {
        assert_eq!(
            share_of(Cents(260_017), 1).unwrap(),
            Cents::from_dollars(2600)
        );
    }

    /// The remainder goes negative when a container is over-allocated, and
    /// pulling a fraction of it back out of a goal is a real thing to want.
    #[test]
    fn a_share_of_a_negative_pot_is_negative() {
        assert_eq!(
            share_of(Cents::from_dollars(-100), 2).unwrap(),
            Cents::from_dollars(-50)
        );
    }

    /// The divisor is typed, so nothing but this stops a divide by zero.
    #[test]
    fn a_non_positive_divisor_is_refused() {
        assert!(share_of(Cents::from_dollars(100), 0).is_err());
        assert!(share_of(Cents::from_dollars(100), -3).is_err());
    }

    /// A list with nothing but a cursor and a length in it, so the shared
    /// scroll state can be asserted without a screen behind it.
    struct List(cursor::Cursor, usize);

    impl List {
        fn of(rows: usize) -> List {
            List(cursor::Cursor::new(), rows)
        }
    }

    impl Scroll for List {
        fn cursor(&self) -> &cursor::Cursor {
            &self.0
        }

        fn cursor_mut(&mut self) -> &mut cursor::Cursor {
            &mut self.0
        }

        fn row_count(&self) -> usize {
            self.1
        }
    }

    /// An empty list must select nothing: a highlight on row zero of no rows
    /// is a cursor on a row that is not there.
    #[test]
    fn an_empty_list_selects_nothing() {
        let list = List::of(0);
        assert_eq!(table_state(&list, 0, 10).0.selected(), None);
        assert_eq!(table_state(&list, 1, 10).0.selected(), Some(0));
    }

    /// The draw is owed rather than unconditional, so a missed redraw is a
    /// stale screen -- which is worse than the wasted one this exists to
    /// avoid. A key press earns a frame whatever it turns out to mean,
    /// because `on_key` clears the footer's message before it dispatches.
    #[test]
    fn a_key_press_and_a_resize_owe_a_frame_and_nothing_else_does() {
        use ratatui::crossterm::event::{
            KeyCode, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
        };

        let key = |kind| {
            Event::Key(KeyEvent {
                // A letter no screen binds: an unanswered key still takes the
                // status message away, so it still owes a frame.
                code: KeyCode::Char('~'),
                modifiers: KeyModifiers::NONE,
                kind,
                state: KeyEventState::NONE,
            })
        };
        assert!(redraws(&key(KeyEventKind::Press)));
        assert!(redraws(&Event::Resize(80, 24)));

        assert!(!redraws(&key(KeyEventKind::Release)));
        assert!(!redraws(&key(KeyEventKind::Repeat)));
        assert!(!redraws(&Event::FocusGained));
        assert!(!redraws(&Event::FocusLost));
        assert!(!redraws(&Event::Paste("pasted".to_string())));
        assert!(!redraws(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })));
    }

    /// The list drawn into `area`, as one string per line of the terminal.
    ///
    /// `area` is what the screen was handed, which is the whole terminal for
    /// a titled list and an already-inset rectangle for a bare one -- the
    /// difference the scroll track's rule exists to absorb.
    fn drawn(list: &List, chrome: Chrome, area: Rect, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // Long enough to fill the one flexible column, since a row that
        // stops short of the gutter would not notice one that was never
        // reserved.
        let rows: Vec<TableRow<'static>> = (0..list.row_count())
            .map(|i| TableRow::new(vec![Cell::from(format!("row {i} {}", "x".repeat(80)))]))
            .collect();
        let drawn = list.row_count();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render_table(
                    frame,
                    area,
                    list,
                    chrome,
                    &[Constraint::Min(10)],
                    rows,
                    drawn,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// The column an indicator would be drawn in, top to bottom.
    fn column(lines: &[String], x: usize) -> String {
        lines
            .iter()
            .map(|line| line.chars().nth(x).unwrap_or(' '))
            .collect()
    }

    /// A titled list in a terminal too short for it: the thumb rides the
    /// block's own right border, and the corners above and below it are
    /// still the corners.
    #[test]
    fn a_list_taller_than_its_viewport_draws_a_thumb_on_the_border() {
        let lines = drawn(
            &List::of(50),
            Chrome::titled("Credit").header(TableRow::new(vec![Cell::from("Row")])),
            Rect::new(0, 0, 40, 10),
            40,
            10,
        );
        let border = column(&lines, 39);
        assert!(border.contains('█'), "no thumb on the border: {border:?}");
        assert_eq!(
            border.chars().next(),
            Some('┐'),
            "the thumb reached the top corner: {border:?}"
        );
        assert_eq!(
            border.chars().last(),
            Some('┘'),
            "the thumb reached the bottom corner: {border:?}"
        );
    }

    /// The mark says "there is more this way", so a list with nothing off
    /// screen draws an unbroken border and says nothing.
    #[test]
    fn a_list_that_fits_its_viewport_draws_no_thumb() {
        let lines = drawn(
            &List::of(3),
            Chrome::titled("Credit").header(TableRow::new(vec![Cell::from("Row")])),
            Rect::new(0, 0, 40, 10),
            40,
            10,
        );
        assert_eq!(column(&lines, 39), "┐││││││││┘");
    }

    /// Where the thumb sits is where the list is: against the top of the
    /// track at the first row and against the bottom at the last, so the
    /// border answers "how far down am I" as well as "is there more".
    #[test]
    fn the_thumb_travels_the_track_from_the_first_row_to_the_last() {
        let chrome = || Chrome::titled("Credit").header(TableRow::new(vec![Cell::from("Row")]));
        let area = Rect::new(0, 0, 40, 10);

        // The track is the seven lines the rows take: the border less its
        // two corners and the header line above them, which stay `│`.
        let top = drawn(&List::of(50), chrome(), area, 40, 10);
        assert_eq!(column(&top, 39), "┐│█││││││┘");

        let mut list = List::of(50);
        list.select_last();
        let bottom = drawn(&list, chrome(), area, 40, 10);
        let border = column(&bottom, 39);
        assert!(
            border.ends_with("█┘"),
            "the last row left the thumb short of the bottom: {border:?}"
        );
    }

    /// The rows stop a column short of the border beside them, so the thumb
    /// is read against a space rather than against the last of a figure.
    ///
    /// The rows here fill their column, which is what makes this a test:
    /// a row stopping short of the gutter would not notice one that was
    /// never reserved. Both chromes, since a bare list asks its own `Block`
    /// for the padding a titled one gets from its border's.
    #[test]
    fn the_rows_are_held_a_column_off_the_border_beside_them() {
        for chrome in [Chrome::titled("Credit"), Chrome::bare()] {
            let titled = chrome.title.is_some();
            let area = match titled {
                true => Rect::new(0, 0, 40, 10),
                false => Rect::new(1, 1, 38, 8),
            };
            let lines = drawn(&List::of(50), chrome, area, 40, 10);
            // The rows either way, and the border they are held off is 39.
            let rows = &lines[2..9];
            assert!(
                rows.iter().all(|line| line.chars().nth(38) == Some(' ')),
                "a row reached the gutter: {rows:?}"
            );
            assert!(
                rows.iter().any(|line| line.chars().nth(37) == Some('x')),
                "no row reached the column before it: {rows:?}"
            );
        }
    }

    /// A bare list was handed an area someone else inset for a border, so
    /// its track is the column past the rows in exactly the same sense --
    /// one column outside the area it was given, and still not a column any
    /// row was drawn in.
    #[test]
    fn a_bare_list_draws_its_thumb_on_the_border_its_caller_owns() {
        let lines = drawn(
            &List::of(50),
            Chrome::bare(),
            Rect::new(1, 1, 38, 8),
            40,
            10,
        );
        assert!(
            column(&lines, 39).contains('█'),
            "no thumb past the rows: {:?}",
            column(&lines, 39)
        );
    }

    /// The offset a draw resolves is the one it reports back, so the next
    /// draw carries on from where this one left the list.
    #[test]
    fn the_drawn_offset_is_the_one_reported_back() {
        let mut list = List::of(58);
        list.cursor_mut().select(57);
        let (state, viewport) = table_state(&list, 58, 30);
        assert_eq!(state.offset(), 28);
        assert_eq!(
            viewport,
            Viewport {
                height: 30,
                offset: 28
            }
        );
    }
}
