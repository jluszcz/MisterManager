//! Where the cursor sits in a list, how far a page moves it, and which row the
//! list is drawn from.
//!
//! Every screen with a list had its own copy of this: a `selected` and a
//! `page_height` field, and the same eight one-line methods over them. They
//! had already drifted -- some clamped a shrinking list, some did not, and
//! only two of them offered `Home` and `End` at all.

use ratatui::crossterm::event::KeyCode;

/// A list cursor, without the list.
///
/// The rows stay on the screen that owns them: they are `rows` on Ledger,
/// `visible` on Savings, `entries` on the picker, and `lines` on the
/// worksheet, and two of those are filtered views that change length under the
/// cursor. Every method that needs the length is handed it, which is what lets
/// one cursor serve all of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct Cursor {
    index: usize,
    page_height: usize,
    offset: usize,
}

impl Cursor {
    /// Opens on the first row, one line to a page until a draw says otherwise.
    pub(super) fn new() -> Cursor {
        Cursor {
            index: 0,
            page_height: 1,
            offset: 0,
        }
    }

    pub(super) fn index(self) -> usize {
        self.index
    }

    /// Put the cursor somewhere specific — an anchor after a reload, not a
    /// keypress. The caller is responsible for the index being in range.
    pub(super) fn select(&mut self, index: usize) {
        self.index = index;
    }

    /// Pull the cursor back inside a list that has just shrunk.
    ///
    /// A cursor left past the end would make the row operators — `e`, `d`,
    /// `P` — act on nothing, or on whatever later lands at that index.
    pub(super) fn clamp(&mut self, len: usize) {
        self.index = self.index.min(len.saturating_sub(1));
    }

    pub(super) fn next(&mut self, len: usize) {
        if self.index + 1 < len {
            self.index += 1;
        }
    }

    pub(super) fn previous(&mut self) {
        self.index = self.index.saturating_sub(1);
    }

    pub(super) fn first(&mut self) {
        self.index = 0;
    }

    pub(super) fn last(&mut self, len: usize) {
        self.index = len.saturating_sub(1);
    }

    pub(super) fn page_down(&mut self, len: usize) {
        self.index = (self.index + self.page_height).min(len.saturating_sub(1));
    }

    pub(super) fn page_up(&mut self) {
        self.index = self.index.saturating_sub(self.page_height);
    }

    pub(super) fn page_height(self) -> usize {
        self.page_height
    }

    /// The viewport height, recorded by the draw so `PageUp` and `PageDown`
    /// can move by it. The event loop draws before every key it reads, so a
    /// key never acts on a stale height.
    ///
    /// Clamped to one: a screen too short to hold a row would otherwise leave
    /// paging a no-op.
    pub(super) fn set_page_height(&mut self, height: usize) {
        self.page_height = height.max(1);
    }

    /// Which row the list was last drawn from.
    pub(super) fn offset(self) -> usize {
        self.offset
    }

    /// Take back what the draw resolved: the viewport it had, and where it
    /// started it. The next draw starts from that offset rather than from
    /// zero, which is what makes the view hold still while the cursor has
    /// room to move inside it.
    pub(super) fn record_viewport(&mut self, viewport: Viewport) {
        self.set_page_height(viewport.height);
        self.offset = viewport.offset;
    }
}

/// What a draw resolved for one list: how tall its viewport was, and which row
/// it started at. Both come back out of the draw, because only the draw knows
/// how many lines the screen left for rows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Viewport {
    pub(super) height: usize,
    pub(super) offset: usize,
}

impl Viewport {
    /// A viewport with nothing scrolled out of it — what a screen reports when
    /// it drew a message in place of its list.
    pub(super) fn of_height(height: usize) -> Viewport {
        Viewport { height, offset: 0 }
    }
}

/// How many rows of context the cursor keeps between itself and the edge it is
/// travelling towards, so that the row it is about to reach is already on
/// screen.
const MARGIN: usize = 3;

/// What a draw has to keep on screen: the selected row, and the runs either
/// side of it that the cursor cannot rest on.
///
/// `context` is the topmost row that has to come into view with the
/// selection, `tail` the bottommost. Both are the selection itself on every
/// list whose rows the cursor can all rest on. Where it cannot -- Planning,
/// where only the editable rows are selectable -- a row the cursor can never
/// reach is reachable by nothing but the view, so it travels with the nearest
/// row that can be selected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub(super) context: usize,
    pub(super) selected: usize,
    pub(super) tail: usize,
}

/// Where the viewport starts, given where it started last time, where the
/// cursor is now, and what sits either side of the cursor that it cannot
/// reach.
///
/// The view holds still until the cursor comes within [`MARGIN`] rows of an
/// edge, and then scrolls by exactly what that costs -- so the rows above a
/// cursor moving down stay where they are while there is room for it below,
/// and a screen the cursor has only moved a few rows into has not scrolled at
/// all. Keeping the cursor centred instead would move the list under it from
/// the halfway row onwards, which takes the Planning screen's transfers off the
/// top on the way down to a constant a dozen rows below them -- on a screen
/// whose whole point is that the block at the top is what the owner acts on.
///
/// The margin is what stops the cursor riding the very edge it is moving
/// towards, where the whole viewport shows rows already behind it. It gives
/// way at the ends of the list -- there is nothing beyond the last row to keep
/// in view -- and on a viewport too short to hold it either side of the
/// cursor.
///
/// [`Selection`] says which rows have to be on screen: the cursor's own, and
/// the unreachable runs above and below it that only the view can bring into
/// sight.
pub(super) fn viewport_offset(
    current: usize,
    selection: Selection,
    rows: usize,
    height: usize,
) -> usize {
    let Selection {
        context,
        selected,
        tail,
    } = selection;
    if rows <= height || height == 0 {
        return 0;
    }
    let last = rows - height;
    let margin = MARGIN.min((height - 1) / 2);
    // Far enough down that the cursor and its margin are on screen, and that
    // the tail below it is too -- the tail with no margin of its own, since it
    // is the end of a run rather than a cursor about to move past it. No
    // further down than the cursor's own margin or the context above it.
    // Everything is clamped at the end of the list, which is what the `min`s
    // are: `lowest` outruns `last` in the final screenful, where the margin
    // below is rows that do not exist. `lowest` wins the tie, since the row
    // the cursor is on has to be drawn whatever is either side of it.
    let lowest = selected
        .saturating_sub(height - 1 - margin)
        .max(tail.saturating_sub(height - 1))
        .min(last);
    let highest = selected
        .saturating_sub(margin)
        .min(context)
        .min(last)
        .max(lowest);
    current.clamp(lowest, highest)
}

impl Default for Cursor {
    fn default() -> Cursor {
        Cursor::new()
    }
}

/// A screen with a list in it: where its cursor lives, and how many rows the
/// cursor may move over.
///
/// The three required methods are the whole of what a screen has to say. The
/// six movements are the same on every list, so they are defaults here rather
/// than eight one-line methods per screen — which is what they were, and which
/// is why only some of them offered `Home` and `End`.
///
/// Planning is the one screen that overrides them: barely a third of its rows
/// are editable and the cursor may only rest on those, so every movement there
/// is "and then settle on the nearest editable row".
pub(super) trait Scroll {
    fn cursor(&self) -> &Cursor;
    fn cursor_mut(&mut self) -> &mut Cursor;

    /// How many rows the cursor may move over. The *visible* count wherever a
    /// screen filters — the ledgers, Savings, Recurring Goals, the worksheet
    /// and the destination chooser all narrow the list under the cursor, and
    /// counting every row a screen holds would offer the cursor rows that
    /// were never drawn.
    fn row_count(&self) -> usize;

    fn selected_index(&self) -> usize {
        self.cursor().index()
    }

    /// The topmost row that has to be on screen with the selection: the
    /// selection itself, unless the rows above it are ones the cursor can
    /// never rest on. Planning is the one screen that overrides it, and
    /// [`viewport_offset`] says what it is for.
    fn context_row(&self) -> usize {
        self.selected_index()
    }

    /// The bottommost row that has to be on screen with the selection, and
    /// [`context_row`](Scroll::context_row)'s mirror: the selection itself,
    /// unless the rows below it are ones the cursor can never rest on and no
    /// selectable row below them can bring them into view. Planning is again
    /// the one screen that overrides it.
    fn tail_row(&self) -> usize {
        self.selected_index()
    }

    fn select_next(&mut self) {
        let len = self.row_count();
        self.cursor_mut().next(len);
    }

    fn select_previous(&mut self) {
        self.cursor_mut().previous();
    }

    fn select_first(&mut self) {
        self.cursor_mut().first();
    }

    fn select_last(&mut self) {
        let len = self.row_count();
        self.cursor_mut().last(len);
    }

    fn page_down(&mut self) {
        let len = self.row_count();
        self.cursor_mut().page_down(len);
    }

    fn page_up(&mut self) {
        self.cursor_mut().page_up();
    }

    fn record_viewport(&mut self, viewport: Viewport) {
        self.cursor_mut().record_viewport(viewport);
    }
}

/// The whole of a list screen's [`Scroll`] impl: the `cursor` field it holds,
/// and `$rows`, the field whose length the cursor travels over.
///
/// Nine screens write exactly this and differ in nothing but that field name,
/// so they say it in one line each. Planning is the one screen that still
/// writes the impl out, because it overrides [`Scroll::context_row`] and
/// [`Scroll::tail_row`] as well, and a macro emitting the whole `impl` has
/// nowhere to put them.
macro_rules! impl_scroll {
    ($screen:ty, $rows:ident) => {
        impl $crate::tui::cursor::Scroll for $screen {
            fn cursor(&self) -> &$crate::tui::cursor::Cursor {
                &self.cursor
            }

            fn cursor_mut(&mut self) -> &mut $crate::tui::cursor::Cursor {
                &mut self.cursor
            }

            fn row_count(&self) -> usize {
                self.$rows.len()
            }
        }
    };
}

pub(super) use impl_scroll;

/// Dispatch the six list-navigation keys, reporting whether one was consumed.
///
/// A screen's key handler tries this first and falls through to its own
/// operators. Written once so the six keys mean the same thing everywhere:
/// each handler used to spell them out, and the two modals spelled out only
/// four, leaving `Home` and `End` dead on the worksheet and the picker.
pub(super) fn scroll_key(target: &mut impl Scroll, code: KeyCode) -> bool {
    match code {
        KeyCode::Up => target.select_previous(),
        KeyCode::Down => target.select_next(),
        KeyCode::Home => target.select_first(),
        KeyCode::End => target.select_last(),
        KeyCode::PageUp => target.page_up(),
        KeyCode::PageDown => target.page_down(),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_stops_at_both_ends_of_the_list() {
        let mut cursor = Cursor::new();
        cursor.previous();
        assert_eq!(cursor.index(), 0);
        for _ in 0..5 {
            cursor.next(2);
        }
        assert_eq!(cursor.index(), 1);
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_row() {
        let mut cursor = Cursor::new();
        cursor.last(50);
        assert_eq!(cursor.index(), 49);
        cursor.first();
        assert_eq!(cursor.index(), 0);
    }

    #[test]
    fn paging_moves_the_cursor_by_one_viewport() {
        let mut cursor = Cursor::new();
        cursor.set_page_height(10);

        cursor.page_down(50);
        assert_eq!(cursor.index(), 10);
        cursor.page_down(50);
        assert_eq!(cursor.index(), 20);
        cursor.page_up();
        assert_eq!(cursor.index(), 10);
    }

    #[test]
    fn paging_past_either_end_stops_on_the_last_row() {
        let mut cursor = Cursor::new();
        cursor.set_page_height(10);

        cursor.page_down(15);
        cursor.page_down(15);
        assert_eq!(cursor.index(), 14);
        cursor.page_up();
        cursor.page_up();
        assert_eq!(cursor.index(), 0);
    }

    /// Every move against an empty list has to land on zero rather than
    /// underflowing, since `len - 1` is the natural way to write all of them.
    #[test]
    fn every_move_over_an_empty_list_stays_at_zero() {
        let mut cursor = Cursor::new();
        cursor.set_page_height(10);

        cursor.next(0);
        cursor.last(0);
        cursor.page_down(0);
        assert_eq!(cursor.index(), 0);
    }

    /// A search box narrowing the list under the cursor is the common case.
    #[test]
    fn clamping_pulls_the_cursor_into_a_shrunken_list() {
        let mut cursor = Cursor::new();
        cursor.last(5);
        assert_eq!(cursor.index(), 4);

        cursor.clamp(2);
        assert_eq!(cursor.index(), 1);
        cursor.clamp(0);
        assert_eq!(cursor.index(), 0);
    }

    /// A height of zero would leave `PageDown` moving nowhere on a terminal
    /// too short to hold a row.
    #[test]
    fn a_page_is_never_shorter_than_one_row() {
        let mut cursor = Cursor::new();
        cursor.set_page_height(0);
        assert_eq!(cursor.page_height(), 1);

        cursor.page_down(10);
        assert_eq!(cursor.index(), 1);
    }
    /// Every list but Planning's has a cursor that can rest on any row, which
    /// is the context and the tail both being the selection itself.
    fn offset(current: usize, selected: usize, rows: usize, height: usize) -> usize {
        viewport_offset(current, at(selected), rows, height)
    }

    /// A selection with nothing unreachable either side of it.
    fn at(selected: usize) -> Selection {
        Selection {
            context: selected,
            selected,
            tail: selected,
        }
    }

    /// A list that fits has nothing to scroll: every row is on screen whatever
    /// the cursor does.
    #[test]
    fn a_list_shorter_than_the_viewport_never_scrolls() {
        assert_eq!(offset(0, 0, 4, 10), 0);
        assert_eq!(offset(0, 3, 4, 10), 0);
    }

    /// The complaint this rule was written for: the Planning screen's
    /// transfers head the list, and reaching `Excess (Used)` twenty rows down
    /// a thirty-row viewport must not take them off the top.
    #[test]
    fn the_view_holds_still_while_the_cursor_has_room_below_it() {
        for selected in 0..=26 {
            assert_eq!(
                offset(0, selected, 58, 30),
                0,
                "scrolled with the cursor on row {selected}"
            );
        }
    }

    /// Once the cursor is within the margin of the bottom, the list moves by
    /// exactly what each further row costs.
    #[test]
    fn the_list_scrolls_by_one_row_once_the_cursor_reaches_the_margin() {
        assert_eq!(offset(0, 27, 58, 30), 1);
        assert_eq!(offset(1, 28, 58, 30), 2);
    }

    /// Coming back up, the view holds until the cursor reaches the margin at
    /// the top -- so the context follows the direction of travel rather than
    /// the cursor being pinned to one line of the screen.
    #[test]
    fn coming_back_up_the_view_holds_until_the_cursor_nears_the_top() {
        assert_eq!(offset(20, 25, 58, 30), 20);
        assert_eq!(offset(20, 23, 58, 30), 20);
        assert_eq!(offset(20, 22, 58, 30), 19);
    }

    /// A jump lands the cursor a screenful away, and the view has to follow it
    /// in one step rather than a row at a time.
    #[test]
    fn a_jump_past_the_viewport_brings_the_cursor_back_into_it() {
        assert_eq!(offset(0, 57, 58, 30), 28);
        assert_eq!(offset(28, 0, 58, 30), 0);
    }

    /// The margin gives way at the ends: below the last row there is nothing
    /// to keep in view, so the cursor reaches it rather than the list
    /// scrolling past its end.
    #[test]
    fn the_offset_stops_with_the_last_row_on_screen() {
        assert_eq!(offset(1500, 1599, 1600, 10), 1590);
        assert_eq!(offset(1590, 1599, 1600, 10), 1590);
    }

    /// A viewport too short to hold the margin either side of the cursor still
    /// has to show the row the cursor is on.
    #[test]
    fn a_viewport_shorter_than_the_margin_still_shows_the_cursor() {
        assert_eq!(offset(0, 7, 20, 1), 7);
        assert_eq!(offset(9, 3, 20, 2), 3);
    }

    /// A cursor that cannot reach the rows above it -- Planning's transfers,
    /// which carry nothing to edit -- still has to bring them into view, or
    /// the top of that screen is lost for the rest of the session.
    #[test]
    fn the_context_above_an_unreachable_row_comes_into_view_with_it() {
        let selection = Selection {
            context: 0,
            ..at(14)
        };
        assert_eq!(viewport_offset(28, selection, 58, 30), 0);
    }

    /// The mirror, and the case Planning reaches when the last of its
    /// editable rows has computed lines under it: nothing the cursor can
    /// stand on is below them, so a rule that only ever followed the cursor
    /// would leave them off the bottom for good.
    #[test]
    fn the_tail_below_an_unreachable_row_comes_into_view_with_it() {
        let selection = Selection { tail: 57, ..at(53) };
        assert_eq!(viewport_offset(0, selection, 58, 30), 28);
    }

    /// The tail is the end of a run rather than a cursor about to move past
    /// it, so it comes into view on the last line of the screen and asks for
    /// no margin under itself.
    #[test]
    fn the_tail_keeps_no_margin_of_its_own() {
        let selection = Selection { tail: 45, ..at(38) };
        assert_eq!(viewport_offset(0, selection, 58, 30), 16);
    }

    /// It is context, not a second cursor: the row the cursor is on has to be
    /// drawn, so a run too long for the viewport gives way rather than
    /// scrolling the selection off the top.
    #[test]
    fn context_taller_than_the_viewport_gives_way_to_the_cursor() {
        let selection = Selection {
            context: 0,
            ..at(57)
        };
        assert_eq!(viewport_offset(40, selection, 58, 30), 28);
    }

    /// The height a draw reports is the page height *and* where the next draw
    /// starts from, so both come back through one write.
    #[test]
    fn a_recorded_viewport_is_the_page_height_and_the_offset() {
        let mut cursor = Cursor::new();
        cursor.record_viewport(Viewport {
            height: 12,
            offset: 4,
        });
        assert_eq!(cursor.page_height(), 12);
        assert_eq!(cursor.offset(), 4);
    }
}
