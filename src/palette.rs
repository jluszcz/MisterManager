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

/// The four classes, in [`crate::allocation::Class::ALL`]'s order -- the index
/// every reader looks one up by.
///
/// Here rather than beside the report that spells them, for the reason the
/// funding ramp is here: the portfolio's composition is drawn on the Funds
/// screen and again on the report's Funds tab, and a second table of colors
/// in either would have the terminal and the phone disagreeing about what
/// bonds look like on the first re-tint.
///
/// **An index is safe here where it is not for an account**, which holds a
/// name in the database precisely so a reordered array cannot repaint it:
/// nothing stores a class as a number, so the position is derived from the
/// enum on every read and a reorder moves both halves at once.
///
/// Bonds are the green, the two equities the blues one step apart in
/// lightness, and `Other` the neutral -- being what the age rule is not
/// asking about. A class reads as itself against the row beside it, which is
/// what the bars are for: four segments answering the four rows, rather than
/// a split the rows above them cannot make.
///
/// **Written in `Class::ALL`'s order**, which is what `Class::index` reads:
/// the two equities, then bonds, then the neutral. Reordering that enum
/// without reordering this array repaints every segment, which is the one
/// thing an index-keyed table can get wrong -- and the reason it is still
/// safe here is that nothing *stores* a class as a number, so the two move
/// together in one commit or not at all.
pub const CLASSES: [Rgb; 4] = [
    (45, 105, 175),
    (110, 170, 220),
    (55, 135, 100),
    (150, 150, 145),
];

/// The ink to write on a colored ground.
///
/// Near-black on a light ground and near-white on a dark one, by Rec. 601
/// luma -- the weighting that says green carries most of a color's apparent
/// brightness and blue almost none, which is what puts [`CLASSES`]' green and
/// its darker blue on the same side of the line despite reading as very
/// different colors.
///
/// Near-black rather than black is [`crate::tui::style::FAVORITE_FG`]'s
/// reason, and near-white rather than white is the same one turned over: a
/// pure extreme against a mid-tone reads as a hole punched in it.
///
/// Here rather than beside the one screen that draws on a ground, because
/// what contrasts with a color is a fact about the color -- the same split
/// this module already makes for every other decision in it.
pub fn on(ground: Rgb) -> Rgb {
    let (r, g, b) = ground;
    let luma = (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000;
    match luma >= MID_LUMA {
        true => (28, 30, 34),
        false => (238, 240, 244),
    }
}

/// Where a ground stops being dark and starts being light, in Rec. 601 luma.
///
/// Half of 255, which is where the two inks are equally far away. Nothing in
/// [`CLASSES`] sits near it -- the closest is twenty points clear -- so the
/// exact figure is not load-bearing and a color added later would have to be
/// chosen deliberately badly to land on it.
const MID_LUMA: u32 = 128;

/// `#rrggbb`, for a medium that spells its colors.
pub fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class color is a ground the bars write a share on, so each has
    /// to take an ink that can be read against it -- and the two inks are the
    /// only answers, so what this pins is that each class gets the right one
    /// of the two rather than the nearer one.
    #[test]
    fn every_class_color_takes_a_readable_ink() {
        let luma =
            |(r, g, b): Rgb| (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000;
        for (class, ground) in crate::allocation::Class::ALL.iter().zip(CLASSES) {
            let ink = on(ground);
            let gap = luma(ground).abs_diff(luma(ink));
            assert!(
                gap > 60,
                "{class:?}'s ink is {gap} from its ground, which is not a contrast"
            );
        }
    }

    /// The two ends, and the rule between them: a dark ground takes the pale
    /// ink and a light one takes the dark ink, whatever the hue.
    #[test]
    fn a_dark_ground_takes_pale_ink_and_a_light_ground_takes_dark() {
        assert_eq!(on((0, 0, 0)), (238, 240, 244));
        assert_eq!(on((255, 255, 255)), (28, 30, 34));
        // Blue carries almost no apparent brightness and green most of it,
        // which is why these two land on opposite sides despite the second
        // being the numerically smaller triple.
        assert_eq!(on((0, 0, 255)), (238, 240, 244));
        assert_eq!(on((0, 255, 0)), (28, 30, 34));
    }

    /// The class colors name the bar's segments *and* tint the class labels
    /// in the summary beside them, where the Δ column spells a shortfall in
    /// [`NEGATIVE`]. A class drawn in that red would read as a warning on
    /// every row it appeared in.
    #[test]
    fn no_class_color_is_the_negative_color() {
        for (class, rgb) in crate::allocation::Class::ALL.iter().zip(CLASSES) {
            assert_ne!(rgb, NEGATIVE, "{class:?} is the negative color");
        }
    }

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

    /// Four adjacent segments of one bar: two classes drawn alike would make
    /// it unreadable exactly where it says the most, the bar being the one
    /// picture of what the rows beside it state as figures.
    ///
    /// The length is checked against the enum for the reason the triples are
    /// checked against each other: the colors are reached by position, so a
    /// fifth class would take the color of nothing at all.
    #[test]
    fn every_band_color_has_a_distinct_triple() {
        assert_eq!(
            CLASSES.len(),
            crate::allocation::Class::ALL.len(),
            "a class has no color, or a color has no class"
        );
        let mut seen = Vec::new();
        for (class, rgb) in crate::allocation::Class::ALL.iter().zip(CLASSES) {
            assert!(!seen.contains(&rgb), "{class:?} repeats a triple");
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
