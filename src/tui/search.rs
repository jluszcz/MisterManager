//! The filter every list screen opens on `/`.
//!
//! Three things live here, and all three are shared because a screen holding
//! its own copy of any of them is a screen that can drift from the others:
//! the box ([`SearchBox`]), the keys that drive it ([`search_key`]), and what
//! a needle *matches* ([`Matcher`]). What a screen still owns is which rows it
//! holds up to the matcher, and which of their figures it offers -- that is
//! [`Search::refilter`], and it is the only part that differs.
//!
//! Every screen filters in memory. The Ledger's rows come out of SQL, but the
//! query is bounded by the window before a needle is typed, so the rows in
//! hand are the only rows a needle could reach -- see the [`Search`] impl on
//! [`Ledger`].
//!
//! [`Ledger`]: super::ledger::Ledger

use super::Label;
use super::form::{Caret, Step};
use super::text::{self, Edit, TextBuffer};
use super::widget::value_spans;
use crate::money::Cents;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Span;

/// A search box, without the list it narrows.
///
/// The rows stay on the screen that owns them, the same way [`Cursor`] leaves
/// them there: the needle is a substring, and matching it against a name is
/// the screen's own job.
///
/// [`Cursor`]: super::cursor::Cursor
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SearchBox {
    needle: TextBuffer,
    open: bool,
}

impl SearchBox {
    pub(super) fn new() -> SearchBox {
        SearchBox::default()
    }

    /// What has been typed. Empty means "match everything", which is why
    /// leaving the box does not have to clear the list.
    pub(super) fn needle(&self) -> &str {
        self.needle.value()
    }

    /// Where the box draws its caret: on the needle while it is open, and
    /// nowhere once `Enter` has left it narrowing the list, since a kept
    /// filter takes no keystrokes.
    pub(super) fn caret(&self) -> Option<Caret> {
        self.open.then(|| Caret::in_buffer(&self.needle))
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
        self.needle.insert(c);
    }

    /// Answer an editing key over the needle, and say what it did.
    pub(super) fn edit(&mut self, key: KeyEvent) -> Edit {
        text::edit_key(&mut self.needle, key)
    }

    /// Move the caret one character. A search box has no date and no
    /// selector, so `←`/`→` are the caret's here with nothing to share them
    /// with.
    pub(super) fn step_caret(&mut self, step: Step) {
        self.needle.step(step.direction());
    }
}

/// A figure as a needle has to be typed: `1,234.56` becomes `1234.56`.
///
/// The separators are drawn for reading, and leaving them in would mean
/// typing one exactly to match -- which is the one thing nobody does when
/// hunting for an amount. Everything else `Cents` prints is kept, the sign
/// included, so `-50` finds a withdrawal and nothing else.
pub(super) fn searchable_amount(cents: Cents) -> String {
    cents.to_string().replace(',', "")
}

/// The needle, folded once, and what a row has to offer to match it.
///
/// Built per re-filter rather than per row: folding is the same work whatever
/// the list is, and "an empty needle matches everything" becomes one decision
/// here instead of an `is_empty()` guard repeated in every screen's
/// [`Search::refilter`].
pub(super) struct Matcher {
    /// Already lowercased, which is why this is a `String` and not a `&str`.
    needle: String,
}

impl Matcher {
    pub(super) fn new(needle: &str) -> Matcher {
        Matcher {
            needle: needle.to_lowercase(),
        }
    }

    /// Whether a row named `text` and carrying `amounts` survives the needle.
    ///
    /// A screen offers the figures its rows are *about*, not every figure it
    /// draws: a derived readout would let a short needle hit almost every row
    /// through one column or another, which is a filter that has stopped
    /// narrowing anything.
    pub(super) fn matches(&self, text: &str, amounts: &[Cents]) -> bool {
        self.needle.is_empty()
            || text.to_lowercase().contains(&self.needle)
            || amounts
                .iter()
                .any(|c| searchable_amount(*c).contains(&self.needle))
    }
}

/// A screen with a search box in it, and what it does when the needle moves.
///
/// Two of the three are plumbing -- hand over the box. The third is the whole
/// of what a screen decides for itself.
pub(super) trait Search {
    fn search_box(&self) -> &SearchBox;
    fn search_box_mut(&mut self) -> &mut SearchBox;

    /// Re-narrow the list to the new needle, holding each row up to
    /// [`Search::matcher`]. Called after every mutation, so a screen never has
    /// to remember to call it itself.
    ///
    /// Required rather than defaulted to a no-op: a screen that filtered
    /// nowhere would compile and then quietly ignore every keystroke typed
    /// into its box.
    fn refilter(&mut self);

    fn is_searching(&self) -> bool {
        self.search_box().is_open()
    }

    fn search(&self) -> &str {
        self.search_box().needle()
    }

    /// The needle as a screen draws it: the text, with the caret over one
    /// character of it while the box is open -- see [`SearchBox::caret`].
    fn search_spans(&self) -> Vec<Span<'static>> {
        let box_ = self.search_box();
        value_spans(&Label::from(box_.needle()), box_.caret())
    }

    /// The needle as something a row can be held up against -- see
    /// [`Matcher`]. A `refilter` builds one and asks it about every row.
    fn matcher(&self) -> Matcher {
        Matcher::new(self.search())
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

    /// Answer an editing key, re-narrowing the list only when the needle
    /// actually changed: a caret moved across it leaves every row where it
    /// was.
    fn edit_search(&mut self, key: KeyEvent) -> Edit {
        let edit = self.search_box_mut().edit(key);
        if edit == Edit::Changed {
            self.refilter();
        }
        edit
    }

    fn step_search_caret(&mut self, step: Step) {
        self.search_box_mut().step_caret(step);
    }
}

/// Dispatch the search-box keys, reporting whether one was consumed.
///
/// A screen's key handler tries this while the box is open, ahead of its own
/// operators. `Esc` abandons the filter; `Enter` leaves the box and keeps it.
pub(super) fn search_key(target: &mut impl Search, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => target.clear_search(),
        KeyCode::Enter => target.end_search(),
        KeyCode::Left => target.step_search_caret(Step::PREVIOUS),
        KeyCode::Right => target.step_search_caret(Step::NEXT),
        _ => return target.edit_search(key) != Edit::Ignored,
    }
    true
}

/// `Esc` where the box is **not** open: clear a filter that was kept, and say
/// whether that is what the key just meant.
///
/// A screen tries this in its own `Esc` arm before doing whatever `Esc` means
/// there. The vocabulary is that `Esc` backs out of the innermost thing, and a
/// filter left narrowing the list by `Enter` is inside the screen's own state:
/// a ledger's window and Savings' month are what `Esc` reaches once the needle
/// is gone. Without it a kept filter could only be cleared by opening the box
/// again to close it a second way, which is the one route nothing on screen
/// suggests.
pub(super) fn escape_kept_filter(target: &mut impl Search) -> bool {
    if target.is_searching() || target.search().is_empty() {
        return false;
    }
    target.clear_search();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::form::backspace_key;
    use ratatui::crossterm::event::KeyModifiers;

    /// The four figures the amount tests below are written against, as a row
    /// would offer them.
    fn amounts() -> Vec<Cents> {
        vec![Cents(123_456), Cents(-5_000), Cents(50), Cents(0)]
    }

    #[test]
    fn a_needle_matches_a_name_whatever_the_case_of_either() {
        let m = Matcher::new("LEG");
        assert!(m.matches("Lego", &[]));
        assert!(Matcher::new("lego").matches("LEGO", &[]));
        assert!(!m.matches("Dropbox", &[]));
    }

    /// The separators are drawn for reading and would have to be typed
    /// exactly to match; taking them out is what lets `1234` find `1,234.56`.
    /// A needle is matched against the figure itself, never against what the
    /// screen draws -- so `mm --demo` narrows a list by amount exactly as an
    /// ordinary run does. Routing this through `demo` would turn every row's
    /// key into the mask and quietly leave the owner unable to find anything
    /// while demonstrating the app. The needle stays visible for the same
    /// reason: it is a query the owner is typing, not a figure off a row.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_still_finds_a_row_by_its_amount() {
        crate::demo::install_with_salt(7);
        assert_eq!(searchable_amount(Cents(123_456)), "1234.56");
        assert!(Matcher::new("1234").matches("Lego", &amounts()));
        assert!(!Matcher::new("9999").matches("Lego", &amounts()));
    }

    #[test]
    fn a_needle_matches_an_amount_with_its_separators_taken_out() {
        assert_eq!(searchable_amount(Cents(123_456)), "1234.56");
        assert!(Matcher::new("1234").matches("Lego", &amounts()));
        assert!(!Matcher::new("1,234").matches("Lego", &amounts()));
    }

    /// A substring, not a prefix: the needle may straddle the decimal point.
    #[test]
    fn a_needle_matches_anywhere_in_an_amount() {
        assert!(Matcher::new("34.5").matches("Lego", &amounts()));
        assert!(Matcher::new(".56").matches("Lego", &amounts()));
    }

    #[test]
    fn a_negative_amount_carries_its_sign_into_the_needle() {
        assert_eq!(searchable_amount(Cents(-5_000)), "-50.00");
        assert!(Matcher::new("-50").matches("Lego", &amounts()));
        assert!(!Matcher::new("-50").matches("Lego", &[Cents(5_000)]));
    }

    /// The rule the whole box rests on: leaving it does not have to clear
    /// the list, because nothing typed matches everything.
    #[test]
    fn an_empty_needle_matches_every_row() {
        assert!(Matcher::new("").matches("Lego", &[]));
        assert!(Matcher::new("").matches("", &[]));
    }

    #[test]
    fn a_needle_in_neither_the_name_nor_an_amount_matches_nothing() {
        assert!(!Matcher::new("999").matches("Lego", &amounts()));
        assert!(!Matcher::new("zebra").matches("Lego", &amounts()));
    }

    /// A row with no figures is still a row: the destination chooser offers
    /// goals with nothing but a name.
    #[test]
    fn a_row_offering_no_amounts_is_matched_on_its_name_alone() {
        assert!(Matcher::new("lego").matches("Lego", &[]));
        assert!(!Matcher::new("50").matches("Lego", &[]));
    }

    #[test]
    fn escape_outside_the_box_clears_a_kept_filter() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();
        screen.push_search('L');
        screen.end_search();
        assert_eq!(screen.visible, vec!["Lego"]);

        assert!(escape_kept_filter(&mut screen));
        assert_eq!(screen.search(), "");
        assert_eq!(screen.visible, vec!["Lego", "Dropbox"]);
    }

    /// Nothing kept means the key is the screen's own -- `Esc` still returns
    /// a ledger to today's window and Savings to every month.
    #[test]
    fn escape_is_left_alone_when_no_filter_is_kept() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        assert!(!escape_kept_filter(&mut screen));

        screen.begin_search();
        screen.push_search('L');
        screen.clear_search();
        assert!(!escape_kept_filter(&mut screen));
    }

    /// While the box is open `Esc` is the box's, and `search_key` has already
    /// answered it. This says so standalone rather than relying on the caller.
    #[test]
    fn escape_is_left_alone_while_the_box_is_still_open() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();
        screen.push_search('L');
        assert!(!escape_kept_filter(&mut screen));
        assert_eq!(screen.search(), "L");
    }

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

        screen.edit_search(backspace_key());
        assert_eq!(screen.visible, vec!["Lego", "Legal Fees"]);
        screen.edit_search(backspace_key());
        screen.edit_search(backspace_key());
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
        screen.edit_search(backspace_key());
        screen.clear_search();
        assert_eq!(screen.refilters, 3);

        screen.end_search();
        assert_eq!(screen.refilters, 3, "leaving the box keeps the needle");
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_box_consumes_the_keys_it_answers_and_nothing_else() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();

        assert!(search_key(&mut screen, press(KeyCode::Char('L'))));
        assert_eq!(screen.search(), "L");
        assert!(search_key(&mut screen, press(KeyCode::Backspace)));
        assert_eq!(screen.search(), "");
        assert!(search_key(&mut screen, press(KeyCode::Enter)));
        assert!(!screen.is_searching());

        screen.begin_search();
        assert!(search_key(&mut screen, press(KeyCode::Esc)));
        assert!(!screen.is_searching());

        // A key the box has no meaning for falls through to the screen.
        assert!(!search_key(&mut screen, press(KeyCode::Up)));
    }

    /// A needle narrowed the list on the way in, so a caret moved back
    /// across it must not send every row through the matcher again.
    #[test]
    fn moving_the_caret_in_the_box_refilters_nothing() {
        let mut screen = Screen::new(vec!["Lego", "Dropbox"]);
        screen.begin_search();
        search_key(&mut screen, press(KeyCode::Char('L')));
        let refilters = screen.refilters;

        assert!(search_key(&mut screen, press(KeyCode::Left)));
        assert!(search_key(
            &mut screen,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ));
        assert_eq!(screen.refilters, refilters);
    }
}
