//! What a transaction's description reads as, in any medium.
//!
//! A ledger row may be committed with no description -- a cash withdrawal, a
//! card charge whose merchant is on the receipt -- so "no description" is a
//! supported state rather than a row half entered. It draws as an em dash,
//! which is what every other absence in the app draws as: an account with no
//! color, a fund with no percentage, a recurring transaction with no horizon.
//!
//! Display only, and the stored value stays the empty string it was written
//! as. The callers are the places a description is put in front of a human --
//! the ledger's Description column, the status line a write reports on, the
//! label on the delete confirmation, and the report's own ledger tabs -- and
//! each would otherwise leave a gap the eye reads as a rendering fault rather
//! than as a blank the owner chose. What is deliberately *not* a caller is the
//! `/` filter, which matches the stored text: typing an em dash must not find
//! every unnamed row.
//!
//! It sits here rather than in `tui` because the report shows the same rows,
//! and a screen and a page disagreeing about what a description-less row looks
//! like is exactly what one shared rule prevents -- the same reason
//! [`crate::palette`] is not part of `tui::style`.

use std::borrow::Cow;

/// The text to draw for a stored description, `—` when there is none.
///
/// A demo replaces the text and leaves the em dash alone: an absence is not
/// something to hide, and a row with nothing in its Description column reads
/// the same in a demonstration as it does in an ordinary run.
pub fn render(raw: &str) -> Cow<'_, str> {
    if raw.trim().is_empty() {
        return Cow::Borrowed("—");
    }
    crate::demo::text(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_with_text_in_it_reads_as_itself() {
        assert_eq!(render("Whole Foods"), "Whole Foods");
    }

    #[test]
    fn a_description_that_was_never_typed_reads_as_an_em_dash() {
        assert_eq!(render(""), "—");
    }

    /// The form trims before it stores, so a stored description is never
    /// whitespace -- but a row written before that trim, or by hand in SQL,
    /// still has to draw as the blank it is rather than as a column of
    /// spaces.
    #[test]
    fn a_description_of_nothing_but_whitespace_reads_as_an_em_dash() {
        assert_eq!(render("   "), "—");
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_replaces_a_description_and_leaves_an_absence_alone() {
        crate::demo::install_with_salt(7);
        assert_ne!(render("Whole Foods"), "Whole Foods");
        assert_eq!(render(""), "—");
    }
}
