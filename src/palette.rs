//! What a color *is*, in numbers.
//!
//! `account.color` holds an [`AccountColor`] -- a name -- and this is the one
//! place a name becomes a number. Which variant an account lands on is
//! [`AccountColor::derived`]'s to say, one layer down; what a variant looks
//! like is said here and nowhere else, so a re-tint is one edit rather than
//! one per medium. The funding ramp is here for the same reason: how funded a
//! goal is reads as a color on the Savings screen and on the Savings tab of
//! the report, and a second set of stops in either would drift on the first
//! re-tint.
//!
//! Medium-neutral on purpose. `tui::style` wraps these into a ratatui
//! `Color::Rgb` and the report formats them as `#rrggbb`; a second table in
//! either of them would be a second decision about what an account looks
//! like, and the two would drift on the first re-tint.

use crate::db::account::AccountColor;
use crate::rate::Percent;

/// A color as three channels. Not a `Color`: this module is below every
/// medium that draws one.
pub type Rgb = (u8, u8, u8);

/// The eight account colors.
///
/// Mid-tone and saturated so they stay legible against a light background and
/// a dark one both, and adjacent variants differ in hue rather than only in
/// brightness -- reordering can separate them, which is why `account.color`
/// holds a name instead of an index.
///
/// No red and no green: those two are spoken for by [`NEGATIVE`] and by the
/// percentage ramp, and an account tinted like a warning is a warning nobody
/// reads.
pub fn account(color: AccountColor) -> Rgb {
    match color {
        AccountColor::Blue => (70, 130, 180),
        AccountColor::Copper => (205, 133, 63),
        AccountColor::Violet => (150, 110, 200),
        AccountColor::Teal => (0, 150, 155),
        AccountColor::Rose => (200, 100, 150),
        AccountColor::Olive => (130, 140, 70),
        AccountColor::Indigo => (90, 110, 210),
        AccountColor::Tan => (160, 120, 90),
    }
}

/// A negative amount, in every medium that renders one.
pub const NEGATIVE: Rgb = (178, 34, 34);

/// The funding ramp's three stops: nothing saved, halfway, funded.
///
/// Two legs rather than one red-to-green interpolation, which would pass
/// through a muddy olive at the midpoint instead of the yellow the halfway
/// mark is supposed to read as.
const RAMP_LOW: Rgb = (200, 60, 60);
const RAMP_MID: Rgb = (200, 180, 60);
const RAMP_HIGH: Rgb = (70, 170, 70);

/// Where [`RAMP_MID`] sits, and so the width of each leg.
const RAMP_MIDPOINT: i64 = 50;

/// `step/span` of the way from `from` to `to`, per channel.
///
/// `span` is [`RAMP_MIDPOINT`] at both call sites -- a private constant, not a
/// setting -- so the divide cannot be by zero and needs no `div_ceil`.
fn lerp(from: Rgb, to: Rgb, step: i64, span: i64) -> Rgb {
    let channel = |a: u8, b: u8| (a as i64 + (b as i64 - a as i64) * step / span) as u8;
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// How funded a goal is, as a color: red at nothing, yellow at halfway, green
/// at fully funded.
///
/// Clamped to `0..=100` rather than extrapolated. A goal can sit outside that
/// range in both directions -- an emergency fund at 106% -- and the ramp has
/// nothing to say past its ends: "more than funded" is still green, and
/// overspent is still red.
pub fn percent(percent: Percent) -> Rgb {
    let clamped = percent.clamp(Percent::ZERO, Percent::ONE_HUNDRED).0;
    if clamped <= RAMP_MIDPOINT {
        lerp(RAMP_LOW, RAMP_MID, clamped, RAMP_MIDPOINT)
    } else {
        lerp(RAMP_MID, RAMP_HIGH, clamped - RAMP_MIDPOINT, RAMP_MIDPOINT)
    }
}

/// `#rrggbb`, for a medium that spells its colors.
pub fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eight accounts that all looked alike would defeat the point of
    /// coloring them at all.
    #[test]
    fn every_account_color_has_a_distinct_triple() {
        let mut seen = Vec::new();
        for color in AccountColor::ALL {
            let rgb = account(color);
            assert!(!seen.contains(&rgb), "{color:?} repeats a triple");
            seen.push(rgb);
        }
    }

    /// A channel below 16 needs its leading zero, or the string is five
    /// characters long and the browser reads a different color entirely.
    #[test]
    fn hex_pads_every_channel_to_two_digits() {
        assert_eq!(hex((0, 150, 155)), "#00969b");
        assert_eq!(hex((255, 255, 255)), "#ffffff");
    }

    /// The negative color is a warning, and a warning that reads as an
    /// account tint is a warning nobody sees.
    #[test]
    fn the_negative_color_is_not_one_of_the_account_colors() {
        for color in AccountColor::ALL {
            assert_ne!(account(color), NEGATIVE, "{color:?} is the negative color");
        }
    }

    #[test]
    fn the_funding_ramp_hits_its_three_stops_exactly() {
        assert_eq!(percent(Percent::ZERO), RAMP_LOW);
        assert_eq!(percent(Percent(50)), RAMP_MID);
        assert_eq!(percent(Percent::ONE_HUNDRED), RAMP_HIGH);
    }

    /// A quarter of the way along each leg, so the two legs are interpolated
    /// rather than stepped between the stops.
    #[test]
    fn the_funding_ramp_blends_between_its_stops() {
        assert_eq!(percent(Percent(25)), (200, 120, 60));
        assert_eq!(percent(Percent(75)), (135, 175, 65));
    }

    /// Goals live outside `0..=100` in both directions: an emergency fund is
    /// overfunded, and an overspent goal is negative. Extrapolating past the
    /// stops would run the channels out of range.
    #[test]
    fn a_percentage_outside_the_ramp_clamps_to_its_ends() {
        assert_eq!(percent(Percent(106)), RAMP_HIGH);
        assert_eq!(percent(Percent(10_000)), RAMP_HIGH);
        assert_eq!(percent(Percent(-15)), RAMP_LOW);
        assert_eq!(percent(Percent(i64::MIN)), RAMP_LOW);
    }
}
