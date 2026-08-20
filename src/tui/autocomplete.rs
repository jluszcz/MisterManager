use crate::db::txn::Suggestion;

/// The suggestion popup's state: what `txn::autocomplete` returned for the
/// description typed so far, which row `↑`/`↓` are on, and how many of those
/// rows the last draw actually fitted on screen.
///
/// Holds no `Db`: `App` runs the query and hands the results in, so the popup
/// itself is a plain list with a cursor.
///
/// **What is drawn is what is selectable.** The popup hangs below a form that
/// is centered in the terminal, so on a short window the list is clipped — and
/// a cursor free to roam the whole list would let `Enter` apply a suggestion
/// the user never saw. In a money app with no undo that is a silent wrong
/// write. `set_visible` is how the render pass reports what it managed to
/// draw, and `next`, `previous` and `selected` are confined to that.
#[derive(Clone, Debug, Default)]
pub struct Autocomplete {
    suggestions: Vec<Suggestion>,
    selected: usize,
    /// How many suggestion rows the last draw fitted on screen — never more
    /// than `suggestions.len()`.
    visible: usize,
}

impl Autocomplete {
    /// How many suggestions to ask for and show.
    pub const LIMIT: i64 = 5;

    /// Replace the list, resetting the cursor to the top.
    ///
    /// Reset rather than preserved: each keystroke re-queries, so a carried
    /// selection would point at whichever unrelated description happened to
    /// land at that index.
    ///
    /// The new list counts as fully visible until the next draw says
    /// otherwise. The event loop draws before it reads a key, so `set_visible`
    /// always corrects this before anything can act on it.
    pub fn set(&mut self, suggestions: Vec<Suggestion>) {
        self.visible = suggestions.len();
        self.suggestions = suggestions;
        self.selected = 0;
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.selected = 0;
        self.visible = 0;
    }

    /// Record how many suggestion rows the draw just fitted, pulling the
    /// cursor back onto them if the window shrank underneath it.
    pub fn set_visible(&mut self, visible: usize) {
        self.visible = visible.min(self.suggestions.len());
        self.selected = self.selected.min(self.visible.saturating_sub(1));
    }

    /// Whether there is anything to draw. Deliberately *not* what decides
    /// whether keys are captured — see `visible`. Were this to fold in the
    /// visible count, a frame that fitted nothing would latch the popup shut
    /// and it could never draw again.
    pub fn is_open(&self) -> bool {
        !self.suggestions.is_empty()
    }

    /// How many rows the last draw fitted, and therefore how many can be
    /// selected. Zero means the popup captures no keys at all, so `Enter`
    /// saves the form as its title bar promises.
    pub fn visible(&self) -> usize {
        self.visible
    }

    pub fn suggestions(&self) -> &[Suggestion] {
        &self.suggestions
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Suggestion> {
        if self.visible == 0 {
            return None;
        }
        self.suggestions.get(self.selected)
    }

    pub fn next(&mut self) {
        if self.visible == 0 {
            return;
        }
        self.selected = (self.selected + 1) % self.visible;
    }

    pub fn previous(&mut self) {
        if self.visible == 0 {
            return;
        }
        self.selected = (self.selected + self.visible - 1) % self.visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::money::Cents;

    fn suggestion(description: &str) -> Suggestion {
        Suggestion {
            description: description.to_string(),
            account_id: AccountId(1),
            cents: Cents(1_000),
            uses: 3,
        }
    }

    #[test]
    fn a_closed_popup_has_nothing_selected() {
        let mut popup = Autocomplete::default();
        assert!(!popup.is_open());
        assert!(popup.selected().is_none());
        // Arrowing around an empty popup must not panic or leave the index
        // pointing past the end of a list that arrives later.
        popup.next();
        popup.previous();
        assert_eq!(popup.selected_index(), 0);
    }

    #[test]
    fn arrowing_past_either_end_wraps() {
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("MBTA"), suggestion("Movies")]);

        assert_eq!(popup.selected().unwrap().description, "MBTA");
        popup.next();
        assert_eq!(popup.selected().unwrap().description, "Movies");
        popup.next();
        assert_eq!(popup.selected().unwrap().description, "MBTA");
        popup.previous();
        assert_eq!(popup.selected().unwrap().description, "Movies");
    }

    /// Each keystroke replaces the whole list, so a selection carried over
    /// from the previous prefix would point at an unrelated suggestion.
    #[test]
    fn a_new_list_of_suggestions_resets_the_selection() {
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("MBTA"), suggestion("Movies")]);
        popup.next();
        assert_eq!(popup.selected_index(), 1);

        popup.set(vec![suggestion("Mortgage")]);
        assert_eq!(popup.selected_index(), 0);
        assert_eq!(popup.selected().unwrap().description, "Mortgage");
    }

    #[test]
    fn clearing_closes_the_popup() {
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("MBTA")]);
        assert!(popup.is_open());
        popup.clear();
        assert!(!popup.is_open());
    }

    /// The list hangs below a centered form, so a short terminal clips it. A
    /// cursor free to run past the last drawn row would let `Enter` apply a
    /// suggestion the user never saw, which in a ledger with no undo is a
    /// silent wrong write.
    #[test]
    fn arrowing_never_leaves_the_rows_that_were_drawn() {
        let mut popup = Autocomplete::default();
        popup.set(vec![
            suggestion("Mortgage"),
            suggestion("Movies"),
            suggestion("MBTA"),
            suggestion("Mobile"),
            suggestion("Museum"),
        ]);
        popup.set_visible(2);

        assert_eq!(popup.selected().unwrap().description, "Mortgage");
        popup.next();
        assert_eq!(popup.selected().unwrap().description, "Movies");
        popup.next();
        assert_eq!(
            popup.selected().unwrap().description,
            "Mortgage",
            "the cycle must wrap at the last drawn row, not the last suggestion"
        );
        popup.previous();
        assert_eq!(popup.selected().unwrap().description, "Movies");
    }

    #[test]
    fn a_window_that_shrinks_pulls_the_cursor_back_onto_a_drawn_row() {
        let mut popup = Autocomplete::default();
        popup.set(vec![
            suggestion("Mortgage"),
            suggestion("Movies"),
            suggestion("MBTA"),
        ]);
        popup.set_visible(3);
        popup.next();
        popup.next();
        assert_eq!(popup.selected_index(), 2);

        popup.set_visible(1);
        assert_eq!(popup.selected_index(), 0);
        assert_eq!(popup.selected().unwrap().description, "Mortgage");
    }

    /// At ten rows or fewer the popup fits its borders but no suggestion. It
    /// must then capture nothing, so `Enter` saves the form as the form's own
    /// title bar promises rather than silently applying suggestion zero.
    #[test]
    fn a_popup_with_no_drawn_rows_selects_nothing_and_consumes_nothing() {
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("Mortgage"), suggestion("Movies")]);
        popup.set_visible(0);

        assert_eq!(popup.visible(), 0);
        assert!(popup.selected().is_none());
        popup.next();
        popup.previous();
        assert!(popup.selected().is_none());
        assert!(
            popup.is_open(),
            "there is still a list to draw once the window has room again"
        );
    }

    /// `set_visible` is fed by the draw, which cannot know more than the list
    /// holds.
    #[test]
    fn more_visible_rows_than_suggestions_is_clamped_to_the_suggestions() {
        let mut popup = Autocomplete::default();
        popup.set(vec![suggestion("Mortgage")]);
        popup.set_visible(9);
        assert_eq!(popup.visible(), 1);

        popup.next();
        assert_eq!(popup.selected_index(), 0);
    }
}
