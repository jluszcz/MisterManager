//! Where color is decided.
//!
//! `ratatui::style::Color` is *decided* here and nowhere else in `src/tui/`,
//! the same way `ratatui` itself is named only inside this directory. Every
//! function is a total mapping from a value the screens already hold to a
//! color, so the choices are unit-testable without a terminal and no screen
//! grows its own opinion about what red means. The type itself is re-exported
//! below, because the helpers that carry one of these decisions to a cell have
//! to name it to pass it -- reaching it as `style::Color` keeps every such
//! mention visibly routed through the module that made the choice.
//!
//! The colors are `Color::Rgb`, not the ANSI names: a named color is whatever
//! the user's terminal theme says it is, and a red-to-green ramp needs the
//! shades between them to be the ones chosen here. That costs a 24-bit-color
//! terminal.

use crate::db::AccountId;
use crate::db::account::AccountColor;
use crate::money::Cents;
use crate::rate::Percent;
// Re-exported rather than merely used: the helpers that carry one of these
// decisions to a cell -- `tui::tinted`, `form::field_line_tinted` -- have to
// name the type to pass it, and routing every such mention through `style`
// keeps the decisions themselves in one place while the plumbing stays
// visibly plumbing.
pub use ratatui::style::Color;
use ratatui::style::Style;

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

/// What each [`AccountColor`] actually looks like.
///
/// A total match rather than an array indexed by the enum: the name the
/// database stores and the shade it draws in are then one fact, and no
/// reordering can separate them. Which is the whole reason `account.color`
/// holds a name instead of a palette index.
///
/// No red and no green: those two are spoken for by [`NEGATIVE`] and by
/// [`percent_color`]'s ramp, and an account tinted like a warning is a warning
/// nobody reads. Mid-tone and saturated so they stay legible against a light
/// terminal and a dark one both.
fn palette(color: AccountColor) -> Color {
    match color {
        AccountColor::Blue => Color::Rgb(70, 130, 180),
        AccountColor::Copper => Color::Rgb(205, 133, 63),
        AccountColor::Violet => Color::Rgb(150, 110, 200),
        AccountColor::Teal => Color::Rgb(0, 150, 155),
        AccountColor::Rose => Color::Rgb(200, 100, 150),
        AccountColor::Olive => Color::Rgb(130, 140, 70),
        AccountColor::Indigo => Color::Rgb(90, 110, 210),
        AccountColor::Tan => Color::Rgb(160, 120, 90),
    }
}

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

/// The band behind a favorited row on the Savings screen.
///
/// A pale slate, and the lightest thing the app draws. Every other color here
/// is a mid-tone chosen to read against a terminal's own background, and they
/// all have to stay legible drawn *on* this -- which means the band cannot sit
/// among them. It has to be at one end of the range or the other, and the
/// light end is the end with room: from here the ramp's halfway yellow, the
/// lightest thing ever drawn on the band, is 45 points of luma below. A dark
/// band clears its own tightest neighbour, [`NEGATIVE`], by eight.
///
/// Cool rather than a flat grey, which reads as dirt beside the saturated
/// colors around it -- but a *cast* and not a hue: its channels spread 12
/// where the least saturated entry in [`palette`] spreads 70, so no account
/// is drawn on a band of its own shade.
pub const FAVORITE_BG: Color = Color::Rgb(214, 218, 226);

/// The text on that band, for every cell that sets no color of its own.
///
/// The band has to bring its own foreground: the terminal's default text is
/// dark on a light theme and light on a dark one, so a background alone would
/// be readable on exactly one of the two. Near-black rather than black, which
/// reads as a hole punched in the row beside the mid-tones around it.
pub const FAVORITE_FG: Color = Color::Rgb(32, 38, 48);

/// The style a favorited row is drawn in, as one value.
///
/// A `Style` rather than the two colors, so the screen applying it never has
/// to know that a band is a pair -- the same reason the ramp is a function
/// and not three stops.
pub fn favorite() -> Style {
    Style::default().bg(FAVORITE_BG).fg(FAVORITE_FG)
}

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
/// The owner's choice where there is one, and otherwise the shade the id
/// derives. Deriving is what makes the field an *override* rather than a step
/// the owner has to complete first: a freshly imported database has a
/// distinct color per account before anybody has opened the Accounts screen,
/// and clearing a color back to `—` returns exactly that.
///
/// Which variant an unset account falls back to is [`AccountColor::derived`]'s
/// to say -- a fact about the enum rather than about what a color looks like.
/// This is the one place the two halves meet.
pub fn account_color(id: AccountId, chosen: Option<AccountColor>) -> Color {
    palette(chosen.unwrap_or_else(|| AccountColor::derived(id)))
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

    /// Perceived brightness, on the usual 299/587/114 weighting and left
    /// unscaled -- every use of it here is a comparison between two of these.
    fn luma(c: Color) -> u32 {
        match c {
            Color::Rgb(r, g, b) => u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114,
            other => panic!("{other:?} is not an Rgb color"),
        }
    }

    /// The band is a background, so it has to bring its own foreground: the
    /// terminal's default text color is dark on a light theme and light on a
    /// dark one, and either one alone would be unreadable against a fixed
    /// band. Both halves or neither.
    #[test]
    fn the_favorite_band_sets_both_halves_of_its_own_contrast() {
        let band = favorite();
        assert_eq!(band.bg, Some(FAVORITE_BG));
        assert_eq!(band.fg, Some(FAVORITE_FG));
    }

    /// Every color a cell may set is a mid-tone drawn *on* the band, so the
    /// band has to sit at one end of the range rather than among them -- and
    /// it sits at the light end, which is the end with room. It must also be
    /// neutral, or an account whose shade is near it would be the one account
    /// the band hides.
    #[test]
    fn the_favorite_band_is_lighter_than_every_color_drawn_on_it() {
        let band = luma(FAVORITE_BG);
        for color in AccountColor::ALL {
            assert!(luma(palette(color)) < band, "{color:?}");
        }
        for on_band in [NEGATIVE, POSITIVE, WARNING, FAVORITE_FG] {
            assert!(luma(on_band) < band, "{on_band:?}");
        }
        for percent in [Percent::ZERO, Percent(50), Percent::ONE_HUNDRED] {
            assert!(luma(percent_color(percent)) < band, "{percent:?}");
        }
    }

    /// The band may carry a cast but not a hue, or it would be the one band
    /// that hides the account whose shade sits nearest it. Measured against
    /// the palette rather than a number picked out of the air: the least
    /// saturated color the band could be confused with is what says how
    /// desaturated "desaturated" has to be.
    #[test]
    fn the_favorite_band_is_far_less_saturated_than_any_color_it_could_hide() {
        let spread = |c: Color| match c {
            Color::Rgb(r, g, b) => u32::from(r.max(g).max(b) - r.min(g).min(b)),
            other => panic!("{other:?} is not an Rgb color"),
        };
        let flattest = AccountColor::ALL
            .iter()
            .map(|c| spread(palette(*c)))
            .min()
            .expect("the palette is not empty");
        let band = spread(FAVORITE_BG);
        assert!(
            band * 3 < flattest,
            "{FAVORITE_BG:?} spreads {band}, against a palette whose flattest spreads {flattest}"
        );
    }

    /// Which color the band is about to collide with if it is ever softened.
    /// The ramp's halfway yellow is the lightest thing drawn on the band, so
    /// it is the one that runs out of contrast first -- and naming it here
    /// means a future tweak reads the constraint rather than rediscovering
    /// it.
    #[test]
    fn the_halfway_yellow_is_the_lightest_thing_the_band_has_to_clear() {
        let halfway = percent_color(Percent(50));
        let on_band = AccountColor::ALL
            .iter()
            .map(|c| palette(*c))
            .chain([NEGATIVE, POSITIVE, WARNING, FAVORITE_FG])
            .chain([Percent::ZERO, Percent::ONE_HUNDRED].map(percent_color));
        for color in on_band {
            assert!(
                luma(color) < luma(halfway),
                "{color:?} is lighter than the halfway yellow"
            );
        }
        assert!(luma(halfway) < luma(FAVORITE_BG));
    }

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
        assert_eq!(
            account_color(AccountId(3), None),
            account_color(AccountId(3), None)
        );
    }

    /// Ids run 1..n after an import, so a real database's accounts are all
    /// distinguishable -- which is the entire request.
    #[test]
    fn accounts_numbered_within_the_palette_all_differ() {
        let colors: Vec<Color> = (1..=AccountColor::ALL.len() as i64)
            .map(|n| account_color(AccountId(n), None))
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
        let wrapped = AccountColor::ALL.len() as i64;
        assert_eq!(
            account_color(AccountId(wrapped), None),
            account_color(AccountId(0), None)
        );
        assert_eq!(
            account_color(AccountId(wrapped + 1), None),
            account_color(AccountId(1), None)
        );
        assert_eq!(
            account_color(AccountId(-1), None),
            account_color(AccountId(wrapped - 1), None)
        );
    }

    /// A color the owner picked outranks the one the id derives -- that is
    /// the whole of what the Accounts screen's `Color` field does.
    #[test]
    fn a_chosen_color_overrides_the_one_the_id_derives() {
        for color in AccountColor::ALL {
            assert_eq!(account_color(AccountId(3), Some(color)), palette(color));
        }
    }

    /// Clearing the field back to `—` has to put an account exactly where it
    /// started, or the choice would be one the owner could not take back.
    #[test]
    fn clearing_a_color_returns_the_one_the_id_derives() {
        let derived = account_color(AccountId(5), None);
        assert_eq!(
            account_color(AccountId(5), Some(AccountColor::derived(AccountId(5)))),
            derived
        );
    }

    /// Two accounts with the same chosen color are indistinguishable, which
    /// is the owner's business -- but no *name* may draw as another name's
    /// shade, or the eight choices would be fewer than eight.
    #[test]
    fn every_named_color_is_a_different_shade() {
        let mut shades: Vec<String> = AccountColor::ALL
            .iter()
            .map(|c| format!("{:?}", palette(*c)))
            .collect();
        shades.sort();
        shades.dedup();
        assert_eq!(shades.len(), AccountColor::ALL.len(), "{shades:?}");
    }

    /// Red and green mean "below zero" and "barely funded" everywhere else on
    /// screen. An account tinted like either reads as a warning it is not.
    #[test]
    fn no_account_color_is_mistakable_for_the_negative_red() {
        for color in AccountColor::ALL {
            assert_ne!(palette(color), NEGATIVE, "{color:?}");
            assert_ne!(palette(color), POSITIVE, "{color:?}");
        }
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
