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
pub mod ledger;
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
pub mod worksheet;

use crate::account_label::{Account, Label};
use crate::db::Db;
use account_label::{account_cell, label_line};
use anyhow::{Result, ensure};
use app::App;
use chrono::NaiveDate;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::style::Style;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Cell, TableState};
use std::time::Duration;
use style::Color;

/// How long a draw waits for input before looping. Nothing animates, so this
/// decides how quickly a resize is picked up, and how closely a status
/// message's expiry follows [`app::STATUS_TTL`].
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

/// The same cell with the cents dropped — see [`Cents::to_whole_dollars`].
///
/// The Savings screen's figures are targets and balances in the thousands,
/// where two decimal places are two digits of noise on every row. Its footer
/// is the exception and keeps its cents: the whole point of that line is the
/// $0.23 a container can sit at, which truncates away to nothing.
fn whole_amount(cents: Cents) -> Cell<'static> {
    money_cell(cents, crate::demo::whole_figure(cents))
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
    let figure = crate::demo::figure(cents);
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

/// Where the viewport starts, given where the cursor is.
///
/// Keeps the cursor near the middle so there is as much list visible below it
/// as above, and clamps at both ends so the first and last rows stay reachable
/// rather than the view scrolling past them.
///
/// Every screen with a list derives its offset from its selection each frame
/// rather than storing one, so they all land here.
fn scroll_offset(selected: usize, rows: usize, height: usize) -> usize {
    if rows <= height {
        return 0;
    }
    selected.saturating_sub(height / 2).min(rows - height)
}

/// The scroll state for a list of `rows` with the cursor on `selected`.
///
/// Handing `TableState` a bare default instead would leave the offset at zero,
/// and ratatui would scroll the minimum needed to reveal the selection --
/// pinning the cursor to the last visible line, with the rest of the window
/// showing only rows already behind it. An empty list selects nothing, so the
/// highlight does not sit on a row that is not there.
fn table_state(selected: usize, rows: usize, height: usize) -> TableState {
    let mut state = TableState::default();
    if rows > 0 {
        state.select(Some(selected));
        *state.offset_mut() = scroll_offset(selected, rows, height);
    }
    state
}

/// Where each of `labels` ends in `line`, matched left to right.
///
/// Sequential rather than independent, so a label that also occurs inside a
/// later one cannot match the wrong column -- the Savings header carries
/// `Goal`, a second `Goal`, and a `Goal Date`, and searching each
/// independently would find all three at the first.
///
/// The header alignment tests compare these against the same reading of a
/// data row: a right-aligned column and its header end in the same place.
#[cfg(test)]
fn ends_in_order(line: &str, labels: &[&str]) -> Vec<usize> {
    let mut at = 0;
    labels
        .iter()
        .map(|label| {
            let start = at
                + line[at..]
                    .find(label)
                    .unwrap_or_else(|| panic!("no {label:?} after column {at} of {line:?}"));
            at = start + label.len();
            at
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
/// `demo` blocks every absolute figure the screens draw -- see
/// [`crate::demo`]. Installed here, before the first frame, because it is a
/// constant of the run rather than state a screen can reach: nothing after
/// this point can turn it on or off.
pub fn run(db: Db, today: NaiveDate, demo: bool) -> Result<Db> {
    crate::demo::install(demo);
    let mut app = App::new(db, today)?;
    // `try_init` enables raw mode, enters the alternate screen, and installs
    // a panic hook that restores the terminal before unwinding -- so a bug
    // leaves a working shell rather than a dead one.
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut app);
    ratatui::try_restore()?;
    result?;
    Ok(app.into_db())
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit() {
        // Before the draw, so a message that has run out of time is gone from
        // the frame it would otherwise appear in for one more tick.
        app.expire_status();
        terminal.draw(|frame| app.render(frame))?;
        if !event::poll(TICK)? {
            continue;
        }
        if let Event::Key(key) = event::read()?
            // Windows reports press and release both; acting on each would
            // run every key twice.
            && key.kind == KeyEventKind::Press
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

    #[test]
    fn a_list_shorter_than_the_viewport_never_scrolls() {
        assert_eq!(scroll_offset(0, 4, 10), 0);
        assert_eq!(scroll_offset(3, 4, 10), 0);
    }

    #[test]
    fn the_cursor_rides_the_middle_of_a_long_list() {
        assert_eq!(scroll_offset(800, 1600, 10), 795);
    }

    /// Near the top there is nothing to scroll to, so the cursor moves down
    /// the screen instead of the list moving under it.
    #[test]
    fn the_offset_stays_at_zero_until_the_cursor_reaches_the_middle() {
        assert_eq!(scroll_offset(2, 1600, 10), 0);
        assert_eq!(scroll_offset(5, 1600, 10), 0);
        assert_eq!(scroll_offset(6, 1600, 10), 1);
    }

    /// The last row must be reachable: pinning the cursor to the middle to the
    /// very end would scroll past the list.
    #[test]
    fn the_offset_stops_with_the_last_row_on_screen() {
        assert_eq!(scroll_offset(1599, 1600, 10), 1590);
    }

    /// An empty list must select nothing: a highlight on row zero of no rows
    /// is a cursor on a row that is not there.
    #[test]
    fn an_empty_list_selects_nothing() {
        assert_eq!(table_state(0, 0, 10).selected(), None);
        assert_eq!(table_state(0, 1, 10).selected(), Some(0));
    }
}
