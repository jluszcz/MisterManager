//! A line of text being typed, and the keys that edit it.
//!
//! One buffer serves every box in the app: a form's [`Field`], and the
//! [`SearchBox`] behind `/`. What they add to it is what differs -- a field
//! remembers whether the user has touched it, a search box whether it is
//! open -- and the caret, the editing, and the keys that drive them are the
//! same in both.
//!
//! [`Field`]: super::form::Field
//! [`SearchBox`]: super::search::SearchBox

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A line of text with a caret in it.
///
/// The caret is counted in **characters**, not bytes: every offset here is
/// turned into a byte index at the moment it is used, so a caret can never
/// come to rest inside a character and panic the slice the next keystroke
/// takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TextBuffer {
    value: String,
    /// How many characters sit before the caret.
    caret: usize,
}

impl TextBuffer {
    pub(super) fn value(&self) -> &str {
        &self.value
    }

    pub(super) fn caret(&self) -> usize {
        self.caret
    }

    /// How many characters the line holds -- the offset the caret rests at
    /// when it is at the end.
    pub(super) fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Replace the line, with the caret at the end of what replaced it.
    ///
    /// Every writer of a whole value goes through here, which is what stops a
    /// caret surviving a rewrite at an offset the new text is too short for.
    pub(super) fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.caret = self.len();
    }

    pub(super) fn clear(&mut self) {
        self.set("");
    }

    pub(super) fn insert(&mut self, c: char) {
        let at = self.byte(self.caret);
        self.value.insert(at, c);
        self.caret += 1;
    }

    /// Delete the character before the caret.
    pub(super) fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        self.caret -= 1;
        let at = self.byte(self.caret);
        self.value.remove(at);
    }

    /// Delete the character the caret is on.
    pub(super) fn delete(&mut self) {
        if self.caret < self.len() {
            let at = self.byte(self.caret);
            self.value.remove(at);
        }
    }

    /// Move the caret one character, back if `direction` is negative and
    /// forward otherwise. What `←`/`→` mean in a box whose text is text --
    /// the direction rather than a `form::Step`, because how far a *date*
    /// moves is a question one layer up from a line of characters.
    pub(super) fn step(&mut self, direction: isize) {
        match direction {
            ..0 => self.left(),
            _ => self.right(),
        }
    }

    fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.len());
    }

    pub(super) fn start(&mut self) {
        self.caret = 0;
    }

    pub(super) fn end(&mut self) {
        self.caret = self.len();
    }

    /// Delete the word before the caret, and any run of spaces between the
    /// caret and it.
    ///
    /// Whitespace is the only boundary, which is what `Ctrl`+`W` means in
    /// every shell: a punctuated word goes in one press rather than three.
    pub(super) fn delete_word_back(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut start = self.caret;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.replace_range(start, self.caret);
    }

    pub(super) fn kill_to_start(&mut self) {
        self.replace_range(0, self.caret);
    }

    pub(super) fn kill_to_end(&mut self) {
        let end = self.len();
        self.replace_range(self.caret, end);
    }

    /// Cut `from..to`, counted in characters, and leave the caret where the
    /// cut began.
    fn replace_range(&mut self, from: usize, to: usize) {
        let (from_byte, to_byte) = (self.byte(from), self.byte(to));
        self.value.replace_range(from_byte..to_byte, "");
        self.caret = from;
    }

    /// The byte index `chars` characters into the line, or its length past
    /// the end.
    fn byte(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(at, _)| at)
    }
}

/// What a keystroke did to a buffer.
///
/// The three are told apart because the box around the buffer has work of its
/// own to do: a search box refilters its list and a form re-asks for its
/// suggestions on [`Edit::Changed`] alone, and a key nobody here answered
/// still has to reach whatever the caller does with it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Edit {
    /// Not an editing key. The caller decides what it means.
    Ignored,
    /// Ours, and the text is as it was: a motion, or a kill with nothing left
    /// to kill.
    Moved,
    /// Ours, and the text is different.
    Changed,
}

/// Dispatch the editing keys over a buffer, saying what they did.
///
/// The same shape as [`scroll_key`] and [`search_key`], and for the same
/// reason: `Ctrl`+`W` deletes a word in every box in the app because it is
/// written here once rather than in each of them.
///
/// **The arrows are deliberately not here.** In a form they step a date and
/// cycle a selector, so only a caller that knows a text field has the focus
/// may read them as the caret -- which is exactly the call the caller is in a
/// position to make and this function is not.
///
/// [`scroll_key`]: super::cursor::scroll_key
/// [`search_key`]: super::search::search_key
pub(super) fn edit_key(text: &mut TextBuffer, key: KeyEvent) -> Edit {
    let before = text.len();
    match key.code {
        // `Ctrl` is the app's second modifier and means editing text, so a
        // combination with no binding here is dropped rather than typed: as a
        // bare `Char` it would arrive in the buffer as its own letter.
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match c.to_ascii_lowercase() {
                'a' => text.start(),
                'e' => text.end(),
                'b' => text.left(),
                'f' => text.right(),
                'w' => text.delete_word_back(),
                'u' => text.kill_to_start(),
                'k' => text.kill_to_end(),
                'd' => text.delete(),
                _ => return Edit::Ignored,
            }
        }
        // `Shift` is how a capital arrives, so it is the one modifier a
        // typed character may carry. Everything else falls through to the
        // drop below rather than being typed as its bare letter: `Alt`+`B`
        // is the word motion a hand that has just learned `Ctrl`+`B` reaches
        // for next, and a terminal that delivers it must not answer with a
        // `b` in the middle of the field.
        KeyCode::Char(c) if is_bare(key) => text.insert(c),
        KeyCode::Backspace => text.backspace(),
        KeyCode::Delete => text.delete(),
        _ => return Edit::Ignored,
    }
    if text.len() == before {
        Edit::Moved
    } else {
        Edit::Changed
    }
}

/// Whether a key press is the character it appears to be: nothing held but
/// `Shift`, which is how a capital arrives.
///
/// The app's own keys are read through this too -- see `App::dispatch` --
/// because a modifier nothing binds must not fall through to the bare
/// letter's meaning, and outside a text box that meaning is an operator: a
/// `Ctrl`+`F` typed a beat after `Esc` closed a form would otherwise be the
/// `f` that toggles a favorite.
pub(super) fn is_bare(key: KeyEvent) -> bool {
    key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

impl<T: Into<String>> From<T> for TextBuffer {
    fn from(value: T) -> TextBuffer {
        let mut text = TextBuffer::default();
        text.set(value);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(value: &str) -> TextBuffer {
        TextBuffer::from(value)
    }

    fn ctrl(text: &mut TextBuffer, c: char) -> Edit {
        edit_key(text, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn typed(text: &mut TextBuffer, c: char) -> Edit {
        edit_key(text, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    #[test]
    fn a_typed_character_lands_in_the_buffer() {
        let mut text = buffer("rent");
        assert_eq!(typed(&mut text, '!'), Edit::Changed);
        assert_eq!(text.value(), "rent!");
    }

    #[test]
    fn ctrl_a_and_ctrl_e_jump_to_the_ends_of_the_line() {
        let mut text = buffer("rent");
        assert_eq!(ctrl(&mut text, 'a'), Edit::Moved);
        assert_eq!(text.caret(), 0);
        assert_eq!(ctrl(&mut text, 'e'), Edit::Moved);
        assert_eq!(text.caret(), 4);
    }

    #[test]
    fn ctrl_b_and_ctrl_f_move_one_character() {
        let mut text = buffer("rent");
        ctrl(&mut text, 'b');
        ctrl(&mut text, 'b');
        assert_eq!(text.caret(), 2);
        ctrl(&mut text, 'f');
        assert_eq!(text.caret(), 3);
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_caret() {
        let mut text = buffer("weekly grocery run");
        assert_eq!(ctrl(&mut text, 'w'), Edit::Changed);
        assert_eq!(text.value(), "weekly grocery ");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_kill_to_the_start_and_the_end() {
        let mut text = buffer("weekly grocery run");
        ctrl(&mut text, 'a');
        ctrl(&mut text, 'f');
        assert_eq!(ctrl(&mut text, 'k'), Edit::Changed);
        assert_eq!(text.value(), "w");

        let mut text = buffer("weekly grocery run");
        ctrl(&mut text, 'u');
        assert_eq!(text.value(), "");
    }

    #[test]
    fn ctrl_d_and_delete_both_take_the_character_the_caret_is_on() {
        let mut text = buffer("rent");
        ctrl(&mut text, 'a');
        assert_eq!(ctrl(&mut text, 'd'), Edit::Changed);
        assert_eq!(text.value(), "ent");

        let deleted = edit_key(
            &mut text,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(deleted, Edit::Changed);
        assert_eq!(text.value(), "nt");
    }

    #[test]
    fn backspace_is_answered_here_as_well() {
        let mut text = buffer("rent");
        let pressed = edit_key(
            &mut text,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(pressed, Edit::Changed);
        assert_eq!(text.value(), "ren");
    }

    /// `Ctrl` is held for the editing keys and nothing else, so a `Ctrl`
    /// nobody has bound must not arrive in the buffer as its bare letter --
    /// which is what `Ctrl`+`C` used to do.
    #[test]
    fn a_ctrl_letter_with_no_binding_is_not_typed_into_the_buffer() {
        let mut text = buffer("rent");
        assert_eq!(ctrl(&mut text, 'c'), Edit::Ignored);
        assert_eq!(text.value(), "rent");
    }

    /// `Alt` is bound nowhere in the app, and an unbound modifier is dropped
    /// rather than typed for the reason a `Ctrl` is: `Alt`+`B` is the word
    /// motion a hand that has just learned `Ctrl`+`B` reaches for next, and a
    /// terminal that delivers it must not answer with a `b` mid-field.
    #[test]
    fn an_alt_letter_is_not_typed_into_the_buffer() {
        let mut text = buffer("rent");
        let pressed = edit_key(
            &mut text,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
        );
        assert_eq!(pressed, Edit::Ignored);
        assert_eq!(text.value(), "rent");
    }

    /// The one modifier a typed character may carry, since it is how a
    /// capital arrives at all.
    #[test]
    fn shift_is_the_modifier_a_typed_character_keeps() {
        let mut text = buffer("");
        let pressed = edit_key(
            &mut text,
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
        );
        assert_eq!(pressed, Edit::Changed);
        assert_eq!(text.value(), "R");
    }

    /// A screen refilters and a form re-asks for suggestions on `Changed`
    /// alone, so a kill with nothing to kill must not claim one.
    #[test]
    fn an_edit_that_removes_nothing_reports_a_move_rather_than_a_change() {
        let mut text = buffer("rent");
        assert_eq!(ctrl(&mut text, 'k'), Edit::Moved);
        assert_eq!(text.value(), "rent");
    }

    /// The arrows are the caller's: in a form they step a date and cycle a
    /// selector, and only a text field may read them as the caret.
    #[test]
    fn the_arrows_are_left_to_the_caller() {
        let mut text = buffer("rent");
        let pressed = edit_key(&mut text, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(pressed, Edit::Ignored);
        assert_eq!(text.caret(), 4);
    }

    #[test]
    fn a_buffer_opens_with_its_caret_at_the_end() {
        let mut text = buffer("grocer");
        text.insert('y');
        assert_eq!(text.value(), "grocery");
    }

    #[test]
    fn typing_lands_where_the_caret_is() {
        let mut text = buffer("grocery");
        text.start();
        text.insert('a');
        assert_eq!(text.value(), "agrocery");
        assert_eq!(text.caret(), 1);
    }

    #[test]
    fn backspace_takes_the_character_before_the_caret() {
        let mut text = buffer("grocery");
        text.left();
        text.backspace();
        assert_eq!(text.value(), "grocey");
        assert_eq!(text.caret(), 5);
    }

    #[test]
    fn delete_takes_the_character_the_caret_is_on() {
        let mut text = buffer("grocery");
        text.start();
        text.delete();
        assert_eq!(text.value(), "rocery");
        assert_eq!(text.caret(), 0);
    }

    #[test]
    fn the_caret_stops_at_both_ends() {
        let mut text = buffer("ab");
        text.start();
        text.left();
        text.backspace();
        assert_eq!((text.value(), text.caret()), ("ab", 0));

        text.end();
        text.right();
        text.delete();
        assert_eq!((text.value(), text.caret()), ("ab", 2));
    }

    #[test]
    fn deleting_a_word_takes_the_word_before_the_caret() {
        let mut text = buffer("weekly grocery run");
        text.delete_word_back();
        assert_eq!(text.value(), "weekly grocery ");
        text.delete_word_back();
        assert_eq!(text.value(), "weekly ");
    }

    /// The trailing run of spaces goes with the word, so a caret parked past
    /// one does not spend a keystroke clearing the gap first.
    #[test]
    fn deleting_a_word_from_past_a_run_of_spaces_takes_the_word_as_well() {
        let mut text = buffer("weekly grocery   ");
        text.delete_word_back();
        assert_eq!(text.value(), "weekly ");
    }

    #[test]
    fn deleting_a_word_takes_only_what_is_before_the_caret() {
        let mut text = buffer("weekly grocery run");
        text.start();
        for _ in 0..13 {
            text.right();
        }
        text.delete_word_back();
        assert_eq!(text.value(), "weekly y run");
        assert_eq!(text.caret(), 7);
    }

    #[test]
    fn killing_to_the_end_leaves_what_is_before_the_caret() {
        let mut text = buffer("weekly grocery run");
        text.start();
        for _ in 0..7 {
            text.right();
        }
        text.kill_to_end();
        assert_eq!(text.value(), "weekly ");
        assert_eq!(text.caret(), 7);
    }

    #[test]
    fn killing_to_the_start_leaves_what_is_after_the_caret() {
        let mut text = buffer("weekly grocery run");
        text.start();
        for _ in 0..7 {
            text.right();
        }
        text.kill_to_start();
        assert_eq!(text.value(), "grocery run");
        assert_eq!(text.caret(), 0);
    }

    /// Every offset here is a character rather than a byte: a caret landing
    /// mid-character would panic the slice the next keystroke takes.
    #[test]
    fn a_multi_byte_character_counts_as_one() {
        let mut text = buffer("café");
        text.left();
        assert_eq!(text.caret(), 3);
        text.backspace();
        assert_eq!(text.value(), "caé");

        text.start();
        text.delete();
        assert_eq!(text.value(), "aé");
    }

    #[test]
    fn setting_the_value_puts_the_caret_at_the_end() {
        let mut text = buffer("weekly grocery run");
        text.start();
        text.set("rent");
        assert_eq!(text.caret(), 4);
        text.insert('!');
        assert_eq!(text.value(), "rent!");
    }
}
