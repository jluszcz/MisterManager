//! Where the cursor sits in a list, and how far a page moves it.
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
}

impl Cursor {
    /// Opens on the first row, one line to a page until a draw says otherwise.
    pub(super) fn new() -> Cursor {
        Cursor {
            index: 0,
            page_height: 1,
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

    /// How many rows the cursor may move over. The *visible* count where a
    /// screen filters: Savings and the worksheet both narrow under the cursor.
    fn row_count(&self) -> usize;

    fn selected_index(&self) -> usize {
        self.cursor().index()
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

    fn set_page_height(&mut self, height: usize) {
        self.cursor_mut().set_page_height(height);
    }
}

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
}
