//! Demo mode: the figures, blocked out.
//!
//! `mm --demo` draws the application exactly as an ordinary run does, with
//! every absolute dollar figure replaced by a fixed run of blocks. It is for
//! showing the app to someone -- a screenshot, a shared screen -- without
//! showing them what is in the accounts.
//!
//! One module decides what a hidden figure looks like, so no screen grows an
//! opinion of its own -- the same shape of rule as `tui::style`, which decides
//! color. It sits at the crate root rather than inside `tui` because two of
//! the strings a demo blocks are built below it: `transfer::diagnose` writes
//! the plug's figure into prose for the Planning screen to draw, and the
//! plug's own error quotes it as well.
//!
//! **Absolute figures only.** A percentage is a shape rather than a sum --
//! the Funds screen's allocation, the Savings screen's `%` -- so nothing here
//! touches `Percent` or `BasisPoints`, and nothing here touches a date, a
//! count, or a name. Neither does it touch a parser: what is typed into a
//! form still parses, so a demo can still be driven.

use crate::money::Cents;
use std::cell::Cell;

/// What every blocked figure prints. Fixed-width, because the number of
/// digits is itself a figure: a mask that kept the shape of `1,234.56` would
/// hide the balance and publish its order of magnitude.
///
/// Six blocks plus a sign is seven columns, which is exactly the narrowest
/// money column the screens lay out for -- the Savings screen's `$/Pay`. A
/// demo therefore moves no column and truncates nothing, and every width test
/// holds with the mask on or off.
const MASK: &str = "██████";

thread_local! {
    /// Whether this run is a demo.
    ///
    /// Set once by [`crate::tui::run`] before the first frame and never again, so
    /// it is a constant of the run rather than state a screen can reach --
    /// which is what lets every formatting function read it instead of taking
    /// it as an argument that could never differ between two calls.
    ///
    /// A thread-local rather than an `AtomicBool` because the app draws on one
    /// thread and the test suite does not: `cargo test` runs each test on its
    /// own thread, so a test that turns the mask on cannot be seen by the
    /// hundreds that assert real figures.
    static DEMO: Cell<bool> = const { Cell::new(false) };
}

/// Make this run a demo. Called once, before anything draws.
pub fn install(on: bool) {
    DEMO.set(on);
}

/// Whether figures are being blocked out.
pub(crate) fn enabled() -> bool {
    DEMO.get()
}

/// A figure with its cents: `1,234.56`, or the mask.
pub(crate) fn figure(cents: Cents) -> String {
    mask_or(cents, || cents.to_string())
}

/// The same figure with the cents dropped -- see [`Cents::to_whole_dollars`].
pub(crate) fn whole_figure(cents: Cents) -> String {
    mask_or(cents, || cents.to_whole_dollars())
}

/// What a form shows in a field holding an amount.
///
/// The mask ignores what was typed, exactly as it ignores what a balance is:
/// a field prefilled with the row's own amount would otherwise publish it to
/// whoever is watching, and a mask that tracked the length would publish it a
/// digit at a time. An empty field stays empty -- there is no figure there to
/// hide, and blocking it would say there was.
pub(crate) fn typed(raw: &str) -> String {
    if enabled() && !raw.is_empty() {
        return MASK.to_string();
    }
    raw.to_string()
}

/// The mask, signed as the figure is, or whatever `real` prints.
///
/// The sign survives because [`crate::tui::style::amount_color`] still sees the
/// real figure and still paints a negative red: dropping the `-` would hide
/// the glyph and leave the fact.
fn mask_or(cents: Cents, real: impl FnOnce() -> String) -> String {
    if !enabled() {
        return real();
    }
    let sign = if cents < Cents::ZERO { "-" } else { "" };
    format!("{sign}{MASK}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_run_prints_the_figure_itself() {
        assert_eq!(figure(Cents(123_456)), "1,234.56");
        assert_eq!(whole_figure(Cents(123_456)), "1,234");
        assert_eq!(typed("1,234.56"), "1,234.56");
    }

    #[test]
    fn a_demo_blocks_every_digit_of_a_figure() {
        install(true);
        assert_eq!(figure(Cents(123_456)), "██████");
        assert_eq!(whole_figure(Cents(123_456)), "██████");
    }

    #[test]
    fn a_demo_keeps_the_sign_a_color_would_have_given_away_anyway() {
        install(true);
        assert_eq!(figure(Cents(-123_456)), "-██████");
        assert_eq!(whole_figure(Cents(-123_456)), "-██████");
    }

    /// The whole point of a fixed-width mask: the number of digits is itself
    /// a figure, and two balances an order of magnitude apart must not read
    /// differently.
    #[test]
    fn a_demo_blocks_a_cent_and_a_million_identically() {
        install(true);
        assert_eq!(figure(Cents(1)), figure(Cents(100_000_000)));
        assert_eq!(whole_figure(Cents(1)), whole_figure(Cents(100_000_000)));
    }

    /// A field with nothing in it has no figure to hide, and blocking it
    /// would say the opposite -- that something is there.
    #[test]
    fn a_demo_leaves_an_empty_field_empty() {
        install(true);
        assert_eq!(typed(""), "");
        assert_eq!(typed("12.50"), "██████");
    }

    /// Whether the app is demonstrating itself is a fact about the run, and
    /// every screen reads the same answer.
    #[test]
    fn an_ordinary_run_is_not_a_demo() {
        assert!(!enabled());
        install(true);
        assert!(enabled());
    }
}
