//! The name filter every list screen opens on `/`.
//!
//! Four screens had their own copy of this: a `search` and a `searching`
//! field, the same seven one-line methods over them, and a fourth copy of the
//! four-key handler in `app.rs`. They had drifted -- three re-filtered after
//! every mutation and one did not, which is the difference between a screen
//! that filters in memory and one that filters in SQL, stated four times
//! instead of once.

use ratatui::crossterm::event::KeyCode;

/// A search box, without the list it narrows.
///
/// The rows stay on the screen that owns them, the same way [`Cursor`] leaves
/// them there: the needle is a substring, and matching it against a name is
/// the screen's own job.
///
/// [`Cursor`]: super::cursor::Cursor
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SearchBox {
    needle: String,
    open: bool,
}

impl SearchBox {
    pub(super) fn new() -> SearchBox {
        SearchBox::default()
    }

    /// What has been typed. Empty means "match everything", which is why
    /// leaving the box does not have to clear the list.
    pub(super) fn needle(&self) -> &str {
        &self.needle
    }

    /// Whether keys are going into the box rather than to the screen's own
    /// operators.
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn open(&mut self) {
        self.open = true;
    }

    /// Leave the box but keep the needle, so the list stays narrowed while
    /// the screen's row operators are used on what it left.
    pub(super) fn close(&mut self) {
        self.open = false;
    }

    pub(super) fn clear(&mut self) {
        self.needle.clear();
        self.open = false;
    }

    pub(super) fn push(&mut self, c: char) {
        self.needle.push(c);
    }

    pub(super) fn backspace(&mut self) {
        self.needle.pop();
    }
}

/// A screen with a search box in it, and what it does when the needle moves.
///
/// The one required pair is the box itself. `refilter` is the hook: the
/// screens that filter in memory rebuild their visible list in it, and the
/// Ledger -- which filters in SQL -- leaves it empty and has `App` re-query
/// after the key is consumed.
pub(super) trait Search {
    fn search_box(&self) -> &SearchBox;
    fn search_box_mut(&mut self) -> &mut SearchBox;

    /// Re-narrow the list to the new needle. Called after every mutation, so
    /// a screen never has to remember to call it itself.
    fn refilter(&mut self) {}

    fn is_searching(&self) -> bool {
        self.search_box().is_open()
    }

    fn search(&self) -> &str {
        self.search_box().needle()
    }

    fn begin_search(&mut self) {
        self.search_box_mut().open();
    }

    fn end_search(&mut self) {
        self.search_box_mut().close();
    }

    fn clear_search(&mut self) {
        self.search_box_mut().clear();
        self.refilter();
    }

    fn push_search(&mut self, c: char) {
        self.search_box_mut().push(c);
        self.refilter();
    }

    fn backspace_search(&mut self) {
        self.search_box_mut().backspace();
        self.refilter();
    }
}

/// Dispatch the search-box keys, reporting whether one was consumed.
///
/// A screen's key handler tries this while the box is open, ahead of its own
/// operators. `Esc` abandons the filter; `Enter` leaves the box and keeps it.
pub(super) fn search_key(target: &mut impl Search, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => target.clear_search(),
        KeyCode::Enter => target.end_search(),
        KeyCode::Backspace => target.backspace_search(),
        KeyCode::Char(c) => target.push_search(c),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen, reduced to the two things the trait asks of one: a box, and
    /// a list it narrows.
    #[derive(Default)]
    struct Screen {
        search: SearchBox,
        names: Vec<&'static str>,
        visible: Vec<&'static str>,
        refilters: usize,
    }

    impl Screen {
        fn new(names: Vec<&'static str>) -> Screen {
            let mut screen = Screen {
                names: names.clone(),
                visible: names,
                ..Screen::default()
            };
            screen.refilter();
            screen.refilters = 0;
            screen
        }
    }

    impl Search for Screen {
        fn search_box(&self) -> &SearchBox {
            &self.search
        }

        fn search_box_mut(&mut self) -> &mut SearchBox {
            &mut self.search
        }

        fn refilter(&mut self) {
            self.refilters += 1;
            let needle = self.search.needle().to_lowercase();
            self.visible = self
                .names
                .iter()
                .filter(|n| n.to_lowercase().contains(&needle))
                .copied()
                .collect();
        }
    }

    #[test]
    fn typing_narrows_the_list_and_backspace_widens_it_again() {
        let mut screen = Screen::new(vec!["Lego", "Legal Fees", "Dropbox"]);

        screen.begin_search();
        for c in "leg".chars() {
            screen.push_search(c);
        }
        assert_eq!(screen.visible, vec!["Lego", "Legal Fees"]);

        screen.backspace_search();
        assert_eq!(screen.visible, vec!["Lego", "Legal Fees"]);
        screen.backspace_search();
        screen.backspace_search();
        assert_eq!(screen.visible, vec!["Lego", "Legal Fees", "Dropbox"]);
    }

    /// The difference between the two ways out of the box: `Enter` keeps the
    /// list narrowed for the row operators, `Esc` gives it all back.
    #[test]
    fn enter_keeps_the_filter_and_esc_abandons_it() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();
        screen.push_search('L');

        screen.end_search();
        assert!(!screen.is_searching());
        assert_eq!(screen.search(), "L");
        assert_eq!(screen.visible, vec!["Lego"]);

        screen.begin_search();
        screen.clear_search();
        assert!(!screen.is_searching());
        assert_eq!(screen.search(), "");
        assert_eq!(screen.visible, vec!["Lego", "Dropbox"]);
    }

    /// The hook is what a screen would otherwise have to remember at each of
    /// its call sites -- which is where the drift this replaces came from.
    #[test]
    fn every_mutation_refilters_exactly_once() {
        let mut screen = Screen::new(vec!["Lego"]);
        screen.begin_search();
        assert_eq!(screen.refilters, 0, "opening the box changes no needle");

        screen.push_search('L');
        screen.backspace_search();
        screen.clear_search();
        assert_eq!(screen.refilters, 3);

        screen.end_search();
        assert_eq!(screen.refilters, 3, "leaving the box keeps the needle");
    }

    #[test]
    fn the_four_search_keys_are_consumed_and_nothing_else_is() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();

        assert!(search_key(&mut screen, KeyCode::Char('L')));
        assert_eq!(screen.search(), "L");
        assert!(search_key(&mut screen, KeyCode::Backspace));
        assert_eq!(screen.search(), "");
        assert!(search_key(&mut screen, KeyCode::Enter));
        assert!(!screen.is_searching());

        screen.begin_search();
        assert!(search_key(&mut screen, KeyCode::Esc));
        assert!(!screen.is_searching());

        // A key the box has no meaning for falls through to the screen.
        assert!(!search_key(&mut screen, KeyCode::Up));
    }
}
