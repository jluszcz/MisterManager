//! Where color is decided.
//!
//! `ratatui::style::Color` is named here and nowhere else in `src/tui/`, the
//! same way `ratatui` itself is named only inside this directory. Every
//! function is a total mapping from a value the screens already hold to a
//! color, so the choices are unit-testable without a terminal and no screen
//! grows its own opinion about what red means.
//!
//! The colors are `Color::Rgb`, not the ANSI names: a named color is whatever
//! the user's terminal theme says it is, and a red-to-green ramp needs the
//! shades between them to be the ones chosen here. That costs a 24-bit-color
//! terminal.

use crate::db::AccountId;
use crate::money::Cents;
use crate::rate::Percent;
use ratatui::style::Color;

/// The funding ramp's three stops: nothing saved, halfway, funded.
///
/// Two legs rather than one red-to-green interpolation, which would pass
/// through a muddy olive at the midpoint instead of the yellow the halfway
/// mark is supposed to read as.
const RAMP_LOW: (u8, u8, u8) = (200, 60, 60);
const RAMP_MID: (u8, u8, u8) = (200, 180, 60);
const RAMP_HIGH: (u8, u8, u8) = (70, 170, 70);

/// Where [`RAMP_MID`] sits, and so the width of each leg.
const RAMP_MIDPOINT: i64 = 50;

/// The colors accounts are drawn from, in order.
///
/// No red and no green: those two are spoken for by [`NEGATIVE`] and by
/// [`percent_color`]'s ramp, and an account tinted like a warning is a warning
/// nobody reads. Mid-tone and saturated so they stay legible against a light
/// terminal and a dark one both.
const ACCOUNTS: [Color; 8] = [
    Color::Rgb(70, 130, 180),
    Color::Rgb(205, 133, 63),
    Color::Rgb(150, 110, 200),
    Color::Rgb(0, 150, 155),
    Color::Rgb(200, 100, 150),
    Color::Rgb(130, 140, 70),
    Color::Rgb(90, 110, 210),
    Color::Rgb(160, 120, 90),
];

/// A negative amount, in every column the app renders one.
///
/// Public because Planning's value column is heterogeneous -- a figure, a
/// count, or a gate's verdict -- so it carries a `negative` flag rather than
/// the `Cents` [`amount_color`] would need. It reads the same constant, so
/// there is still one decision here about what a negative figure looks like.
pub const NEGATIVE: Color = Color::Rgb(178, 34, 34);

/// A figure that is *above* what it is being compared with, where being above
/// it is the good news: the ledger's reconciliation delta, and nothing else so
/// far.
///
/// Deliberately not the counterpart of [`NEGATIVE`] on every screen -- an
/// amount above zero is the ordinary case and takes no color at all, which is
/// what keeps a green one meaning something.
pub const POSITIVE: Color = Color::Rgb(70, 170, 70);

/// Something the owner probably meant to configure and has not.
///
/// Amber rather than red: [`NEGATIVE`] means a figure below zero and, on the
/// Planning screen, a plan that cannot be resolved at all. A Planning line
/// with no destination is neither -- the money leaves the tracked system,
/// which is exactly how the account-backed lines are meant to stand -- so a
/// suggestion worth looking at must not wear the color of a failure.
pub const WARNING: Color = Color::Rgb(230, 160, 30);

/// What a cell means, as far as color is concerned.
///
/// Planning's value column is heterogeneous -- a figure, a count, a gate's
/// verdict, a destination -- so the screen says what a cell *means* and this
/// module decides what that looks like, rather than the screen reaching for a
/// constant and deciding for itself.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Plain,
    /// A figure below zero, or a state that stops the plan resolving.
    Negative,
    /// Configuration missing where something is on offer to fill it.
    Warning,
}

/// The color a tone draws in, or `None` to leave the surrounding style alone.
pub fn tone_color(tone: Tone) -> Option<Color> {
    match tone {
        Tone::Plain => None,
        Tone::Negative => Some(NEGATIVE),
        Tone::Warning => Some(WARNING),
    }
}

/// The color for an amount, or `None` to leave the surrounding style alone.
///
/// `None` rather than `Color::Reset` so a cell composes with the style its row
/// already carries -- the ledger dims rows dated after today, and setting a
/// foreground must not clear that.
pub fn amount_color(cents: Cents) -> Option<Color> {
    (cents < Cents::ZERO).then_some(NEGATIVE)
}

/// The color for a reconciliation delta, or `None` for the reconciled case.
///
/// Zero is not a third color: the border draws it as a dash, and a color there
/// would read as one of the two states it is the absence of.
pub fn delta_color(cents: Cents) -> Option<Color> {
    match cents.cmp(&Cents::ZERO) {
        std::cmp::Ordering::Greater => Some(POSITIVE),
        std::cmp::Ordering::Less => Some(NEGATIVE),
        std::cmp::Ordering::Equal => None,
    }
}

/// An account's color, the same on every screen that names it.
///
/// Keyed on the id and not on the account's position in whatever list the
/// screen is holding: `Ledger` is handed a kind-filtered list, so "the third
/// account here" is a different account on Cash than it is on Credit, and the
/// colors would disagree between two screens showing the same ledger.
///
/// `rem_euclid` rather than `%` so a negative id -- which `account.id` being a
/// rowid rules out, but the type does not -- wraps into the palette instead of
/// panicking on a negative index. Ids run 1..n after an import, so accounts
/// take distinct colors until there are more than [`ACCOUNTS`] holds.
pub fn account_color(id: AccountId) -> Color {
    ACCOUNTS[id.0.rem_euclid(ACCOUNTS.len() as i64) as usize]
}

/// `step/span` of the way from `from` to `to`, per channel.
///
/// `span` is [`RAMP_MIDPOINT`] at both call sites -- a private constant, not a
/// setting -- so the divide cannot be by zero and needs no `div_ceil`.
fn lerp(from: (u8, u8, u8), to: (u8, u8, u8), step: i64, span: i64) -> Color {
    let channel = |a: u8, b: u8| (a as i64 + (b as i64 - a as i64) * step / span) as u8;
    Color::Rgb(
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// How funded a goal is, as a color: red at nothing, yellow at halfway, green
/// at fully funded.
///
/// Clamped to `0..=100` rather than extrapolated. A goal can sit outside that
/// range in both directions -- Emergency Savings is at 106% -- and the ramp
/// has nothing to say past its ends: "more than funded" is still green, and
/// overspent is still red.
pub fn percent_color(percent: Percent) -> Color {
    let clamped = percent.clamp(Percent::ZERO, Percent::ONE_HUNDRED).0;
    if clamped <= RAMP_MIDPOINT {
        lerp(RAMP_LOW, RAMP_MID, clamped, RAMP_MIDPOINT)
    } else {
        lerp(RAMP_MID, RAMP_HIGH, clamped - RAMP_MIDPOINT, RAMP_MIDPOINT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_negative_amount_is_colored() {
        assert_eq!(amount_color(Cents(-1)), Some(NEGATIVE));
        assert_eq!(amount_color(Cents(-100_000)), Some(NEGATIVE));
        assert_eq!(amount_color(Cents::ZERO), None);
        assert_eq!(amount_color(Cents(1)), None);
    }

    /// The whole point of keying on the id: the same account is the same
    /// color on Cash, Savings, Recurring Transactions and Overview alike.
    #[test]
    fn an_account_keeps_one_color_however_often_it_is_asked_for() {
        assert_eq!(account_color(AccountId(3)), account_color(AccountId(3)));
    }

    /// Ids run 1..n after an import, so a real database's accounts are all
    /// distinguishable -- which is the entire request.
    #[test]
    fn accounts_numbered_within_the_palette_all_differ() {
        let colors: Vec<Color> = (1..=ACCOUNTS.len() as i64)
            .map(|n| account_color(AccountId(n)))
            .collect();
        let mut unique = colors.clone();
        unique.sort_by_key(|c| format!("{c:?}"));
        unique.dedup();
        assert_eq!(unique.len(), colors.len(), "{colors:?}");
    }

    /// Past the palette the colors repeat rather than panicking on an index
    /// out of range. A collision is a cosmetic clash; a panic takes the
    /// screen down.
    #[test]
    fn more_accounts_than_colors_wrap_instead_of_panicking() {
        let wrapped = ACCOUNTS.len() as i64;
        assert_eq!(
            account_color(AccountId(wrapped)),
            account_color(AccountId(0))
        );
        assert_eq!(
            account_color(AccountId(wrapped + 1)),
            account_color(AccountId(1))
        );
        assert_eq!(
            account_color(AccountId(-1)),
            account_color(AccountId(wrapped - 1))
        );
    }

    /// Red and green mean "below zero" and "barely funded" everywhere else on
    /// screen. An account tinted like either reads as a warning it is not.
    #[test]
    fn no_account_color_is_mistakable_for_the_negative_red() {
        assert!(!ACCOUNTS.contains(&NEGATIVE));
        assert!(!ACCOUNTS.contains(&POSITIVE));
    }

    /// The reconciliation delta on the ledgers: above the target is money the
    /// owner has and had not counted, below it is money missing from the
    /// ledger.
    #[test]
    fn a_delta_above_its_target_reads_green_and_below_reads_red() {
        assert_eq!(delta_color(Cents(1)), Some(POSITIVE));
        assert_eq!(delta_color(Cents(-1)), Some(NEGATIVE));
    }

    /// A reconciled account is neither a gain nor a warning, and the border
    /// draws it as a plain dash. Coloring zero would make "done" look like
    /// one of the two states it is the absence of.
    #[test]
    fn a_reconciled_delta_takes_no_color_of_its_own() {
        assert_eq!(delta_color(Cents::ZERO), None);
    }

    /// The two carry opposite instructions -- "this plan will not run" and
    /// "this looks unfinished, have a look" -- so one drawn as the other
    /// turns a prompt into an alarm or an alarm into a prompt.
    #[test]
    fn a_warning_is_not_drawn_in_the_negative_red() {
        assert_ne!(WARNING, NEGATIVE);
        assert_eq!(tone_color(Tone::Negative), Some(NEGATIVE));
        assert_eq!(tone_color(Tone::Warning), Some(WARNING));
    }

    /// `None` rather than a color, so a plain cell composes with whatever
    /// style its row already carries instead of clearing it.
    #[test]
    fn a_plain_tone_leaves_the_surrounding_style_alone() {
        assert_eq!(tone_color(Tone::Plain), None);
    }

    fn rgb(stop: (u8, u8, u8)) -> Color {
        Color::Rgb(stop.0, stop.1, stop.2)
    }

    #[test]
    fn the_funding_ramp_hits_its_three_stops_exactly() {
        assert_eq!(percent_color(Percent::ZERO), rgb(RAMP_LOW));
        assert_eq!(percent_color(Percent(50)), rgb(RAMP_MID));
        assert_eq!(percent_color(Percent::ONE_HUNDRED), rgb(RAMP_HIGH));
    }

    /// A quarter of the way along each leg, so the two legs are interpolated
    /// rather than stepped between the stops.
    #[test]
    fn the_funding_ramp_blends_between_its_stops() {
        assert_eq!(percent_color(Percent(25)), Color::Rgb(200, 120, 60));
        assert_eq!(percent_color(Percent(75)), Color::Rgb(135, 175, 65));
    }

    /// Goals live outside `0..=100` in both directions: Emergency Savings is
    /// overfunded, and an overspent goal is negative. Extrapolating past the
    /// stops would run the channels out of range.
    #[test]
    fn a_percentage_outside_the_ramp_clamps_to_its_ends() {
        assert_eq!(percent_color(Percent(106)), rgb(RAMP_HIGH));
        assert_eq!(percent_color(Percent(10_000)), rgb(RAMP_HIGH));
        assert_eq!(percent_color(Percent(-15)), rgb(RAMP_LOW));
        assert_eq!(percent_color(Percent(i64::MIN)), rgb(RAMP_LOW));
    }
}
