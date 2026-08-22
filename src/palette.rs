//! What a color *is*, in numbers.
//!
//! `account.color` holds an [`AccountColor`] -- a name -- and this is the one
//! place a name becomes a number. Which variant an account lands on is
//! [`AccountColor::derived`]'s to say, one layer down; what a variant looks
//! like is said here and nowhere else, so a re-tint is one edit rather than
//! one per medium.
//!
//! Medium-neutral on purpose. `tui::style` wraps these into a ratatui
//! `Color::Rgb` and the report formats them as `#rrggbb`; a second table in
//! either of them would be a second decision about what an account looks
//! like, and the two would drift on the first re-tint.

use crate::db::account::AccountColor;

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
}
