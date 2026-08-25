//! Demo mode: a figure's digits, scrambled.
//!
//! `mm --demo` draws the application exactly as an ordinary run does, with
//! every absolute dollar figure's digits replaced by others drawn from this
//! run's own salt. It is for showing the app to someone -- a screenshot, a
//! shared screen -- without showing them what is in the accounts.
//!
//! `mask` decides what a scrambled figure looks like, so no screen grows an
//! opinion of its own -- the same shape of rule as `tui::style`, which decides
//! color. This module sits at the crate root rather than inside `tui` because
//! several of the strings a demo scrambles are built below it:
//! `transfer::diagnose` writes the plug's figure into prose for the Planning
//! screen to draw, the plug's own error quotes it as well, and the refusals
//! `goal::target`, `db::account` and `db::goal` raise each name an account or
//! a goal in prose a screen prints verbatim -- the Planning screen in place
//! of the plan, or the status line. A message masked where it is built is one
//! message rather than one rewrite per caller.
//!
//! **Absolute figures, and a name the owner typed.** `text` draws a
//! pseudonym of the same length in place of an account or goal name, the way
//! `figure` draws another figure's digits in place of an amount. Everything
//! else is left alone: a percentage is a shape rather than a sum -- the
//! Funds screen's allocation, the Savings screen's `%` -- so nothing here
//! touches `Percent` or `BasisPoints`, and nothing here touches a date or a
//! count either. Nor does it touch the app's own vocabulary or a match key --
//! a screen's own labels, and the substring `gate::Gate` and `plan_line::Line`
//! match a goal name against at import -- since neither is something the
//! owner typed, and a masked match key would silently stop matching at the
//! next import. Neither does it touch a parser: what is typed into a form
//! still parses, so a demo can still be driven.

use crate::money::Cents;
use std::borrow::Cow;
#[cfg(feature = "demo")]
use std::cell::Cell;

#[cfg(feature = "demo")]
mod mask;

#[cfg(feature = "demo")]
thread_local! {
    /// This run's salt, and whether it is a demo at all.
    ///
    /// `None` *is* "not a demo", so one cell carries both facts and there is
    /// no state where the mask is on with nothing behind it.
    ///
    /// A thread-local rather than an `AtomicU64` because the app draws on one
    /// thread and the test suite does not: `cargo test` runs each test on its
    /// own thread, so a test that turns the mask on cannot be seen by the
    /// hundreds that assert real figures.
    static SALT: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Make this run a demo. Called once, before anything draws.
///
/// A no-op without the `demo` feature, which is what lets `tui::run` keep one
/// signature and every caller below it stay unaware the feature exists.
pub fn install(on: bool) {
    #[cfg(feature = "demo")]
    SALT.set(on.then(draw_salt));
    #[cfg(not(feature = "demo"))]
    let _ = on;
}

/// This run's salt, or `None` when this is not a demo.
#[cfg(feature = "demo")]
fn salt() -> Option<u64> {
    SALT.get()
}

/// A salt for this run, from the standard library's own randomly seeded
/// hasher.
///
/// `rand` would be a whole dependency for one `u64` drawn once, and this is
/// obfuscation rather than key material.
#[cfg(feature = "demo")]
fn draw_salt() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one("mm --demo")
}

/// A demo whose salt is known, so a test can assert an exact output.
///
/// Test-only for the reason the salt is drawn per run: a fixed salt is a
/// mapping that survives from one run to the next, which is the one property
/// the drawn salt exists to deny.
#[cfg(all(test, feature = "demo"))]
pub(crate) fn install_with_salt(salt: u64) {
    SALT.set(Some(salt));
}

/// What a screen draws in place of a name the owner typed.
///
/// Borrowed when this is not a demo, which is what keeps it callable in a
/// draw path that has a `&str` and wants one back.
///
/// **Called where text becomes a `Label` or a `Cell`, never where a screen
/// builds its rows.** A form prefills from the row a screen is holding --
/// `App::open_goal_edit` hands `GoalForm` the selected row's own name -- so a
/// pseudonym written into view state is a pseudonym `Enter` would commit.
pub(crate) fn text(s: &str) -> Cow<'_, str> {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return Cow::Owned(mask::text(salt, s));
    }
    Cow::Borrowed(s)
}

/// A figure with its cents: `1,234.56`, or its digits scrambled.
pub(crate) fn figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::figure(salt, cents);
    }
    cents.to_string()
}

/// The same figure with the cents dropped -- see [`Cents::to_whole_dollars`].
pub(crate) fn whole_figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::whole_figure(salt, cents);
    }
    cents.to_whole_dollars()
}

/// The same again for a figure whose color is chosen from its own truncation
/// -- see [`Cents::trunc_to_dollar`]. The cents come off the value rather
/// than off the string, so a sub-dollar remainder reads as the nothing it is
/// instead of as a `-0` painted red.
///
/// **Takes the amount with its cents still on**, which is the whole reason it
/// is here rather than a `trunc_to_dollar()` at the call site: a demo keys a
/// figure's digits on the value it is handed, so a caller truncating first
/// would draw whole dollars unrelated to the ones the same amount draws
/// wherever it is quoted in full.
pub(crate) fn truncated_figure(cents: Cents) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt() {
        return mask::truncated_figure(salt, cents);
    }
    cents.trunc_to_dollar().to_whole_dollars()
}

/// What a form shows in a field holding an amount.
pub(crate) fn typed(raw: &str) -> String {
    #[cfg(feature = "demo")]
    if let Some(salt) = salt()
        && !raw.is_empty()
    {
        return mask::typed(salt, raw);
    }
    raw.to_string()
}

#[cfg(all(test, feature = "demo"))]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_run_prints_the_figure_itself() {
        assert_eq!(figure(Cents(123_456)), "1,234.56");
        assert_eq!(whole_figure(Cents(123_456)), "1,234");
        assert_eq!(truncated_figure(Cents(123_456)), "1,234");
        assert_eq!(truncated_figure(Cents(-23)), "0");
        assert_eq!(typed("1,234.56"), "1,234.56");
    }

    #[test]
    fn a_demo_scrambles_every_digit_of_a_figure() {
        install_with_salt(7);
        assert_ne!(figure(Cents(123_456)), "1,234.56");
        assert_eq!(figure(Cents(123_456)), mask::figure(7, Cents(123_456)));
        assert_eq!(
            whole_figure(Cents(123_456)),
            mask::whole_figure(7, Cents(123_456))
        );
    }

    #[test]
    fn a_demo_keeps_the_sign_a_colour_would_have_given_away_anyway() {
        install_with_salt(7);
        assert!(figure(Cents(-123_456)).starts_with('-'));
        assert!(whole_figure(Cents(-123_456)).starts_with('-'));
    }

    /// The width a figure draws at is the width it would have drawn at, which
    /// is what lets every column stay where it was laid out.
    #[test]
    fn a_scrambled_figure_is_as_wide_as_the_figure_it_replaces() {
        install_with_salt(7);
        assert_eq!(figure(Cents(1)).len(), "0.01".len());
        assert_eq!(figure(Cents(100_000_000)).len(), "1,000,000.00".len());
    }

    /// A field with nothing in it has no figure to hide, and scrambling it
    /// would say the opposite -- that something is there.
    #[test]
    fn a_demo_leaves_an_empty_field_empty() {
        install_with_salt(7);
        assert_eq!(typed(""), "");
        assert_ne!(typed("12.50"), "12.50");
    }

    /// Whether the app is demonstrating itself is a fact about the run, and
    /// every formatting function reads the same answer.
    #[test]
    fn an_ordinary_run_has_no_salt_and_a_demo_has_one() {
        assert!(salt().is_none());
        install(true);
        assert!(salt().is_some());
    }

    /// The salt is what a screenshot does not carry, so it cannot be a constant.
    #[test]
    fn two_runs_draw_two_salts() {
        assert_ne!(draw_salt(), draw_salt());
    }

    /// A test that needs one figure to scramble a known way installs the
    /// salt behind it rather than drawing a random one.
    #[test]
    fn installing_a_known_salt_makes_it_the_runs_salt() {
        install_with_salt(42);
        assert_eq!(salt(), Some(42));
    }

    #[test]
    fn an_ordinary_run_prints_the_name_itself() {
        assert_eq!(text("Rainy Day"), "Rainy Day");
        assert!(matches!(text("Rainy Day"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_demo_replaces_a_name_with_a_pseudonym_of_its_own_length() {
        install_with_salt(7);
        assert_eq!(text("Rainy Day"), mask::text(7, "Rainy Day"));
        assert_ne!(text("Rainy Day"), "Rainy Day");
    }
}
