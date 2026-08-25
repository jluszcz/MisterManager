//! What a demo draws in place of a figure.
//!
//! Two rules, both pure and both keyed on the run's salt. [`super`] is the
//! API a screen calls; [`super::figure`], [`super::whole_figure`],
//! [`super::truncated_figure`] and [`super::typed`] draw through it.
//!
//! **A digit is keyed on the value and its position relative to the decimal
//! point**, which is what makes a demo coherent rather than noisy: one
//! amount draws the same on every screen, all three figure rules agree about
//! the dollars they share, and two amounts an order of magnitude apart are
//! unrelated rather than one substitution away from each other. The key is
//! taken from the amount as it arrives, which is why the reading that drops
//! the cents from the *value* does so here rather than at its call site.
//!
//! It is obfuscation for a demonstration, not a security control. The salt
//! never leaves the process and a screenshot does not carry it; nothing more
//! than that is claimed, and nothing should be built on it.

use crate::money::Cents;
use std::hash::{DefaultHasher, Hash, Hasher};

/// A figure with its cents, every digit replaced.
pub(super) fn figure(salt: u64, cents: Cents) -> String {
    scramble(salt, cents.0, &cents.to_string())
}

/// The same figure with the cents dropped.
pub(super) fn whole_figure(salt: u64, cents: Cents) -> String {
    scramble(salt, cents.0, &cents.to_whole_dollars())
}

/// The same again with the cents taken off the value rather than off the
/// string -- and taken off *past* the key, which is the whole of what this
/// adds. A caller truncating on its own way in would hand `scramble` a
/// different value and draw whole dollars unrelated to the ones the same
/// amount draws wherever it is quoted in full.
pub(super) fn truncated_figure(salt: u64, cents: Cents) -> String {
    scramble(salt, cents.0, &cents.trunc_to_dollar().to_whole_dollars())
}

/// What a form shows in a field holding an amount.
///
/// Text that parses is keyed on the value it parses to, so a field prefilled
/// from a row draws that row's own scrambled figure. Text that does not parse
/// is keyed on itself: it is not a figure yet, and there is no value to agree
/// with.
pub(super) fn typed(salt: u64, raw: &str) -> String {
    let key = match raw.parse::<Cents>() {
        Ok(cents) => cents.0,
        Err(_) => hashed_text(salt, raw) as i64,
    };
    scramble(salt, key, raw)
}

/// Every ASCII digit in `text` replaced, everything else left where it is.
///
/// Positions are counted from the decimal point -- the ones digit is 0 and
/// the first cents digit is -1 -- so two renderings of one value agree about
/// every digit they both draw. The leading digit of a multi-digit integer
/// part is drawn from `1..=9`, because `0,834` reads as a rendering fault
/// rather than as money.
pub(super) fn scramble(salt: u64, key: i64, text: &str) -> String {
    let digits = text.chars().filter(char::is_ascii_digit).count();
    let after_point = match text.rfind('.') {
        Some(point) => text[point..].chars().filter(char::is_ascii_digit).count(),
        None => 0,
    };
    let whole = (digits - after_point) as i32;

    let mut out = String::with_capacity(text.len());
    let mut seen = 0i32;
    for c in text.chars() {
        if !c.is_ascii_digit() {
            out.push(c);
            continue;
        }
        let h = hashed(salt, key, whole - 1 - seen);
        let d = if seen == 0 && whole > 1 {
            1 + h % 9
        } else {
            h % 10
        };
        out.push(char::from(b'0' + d as u8));
        seen += 1;
    }
    out
}

/// The run's randomness, mixed with what is being drawn.
pub(super) fn hashed(salt: u64, key: i64, position: i32) -> u64 {
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    key.hash(&mut h);
    position.hash(&mut h);
    h.finish()
}

/// The same, for text that has no value behind it.
pub(super) fn hashed_text(salt: u64, text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

/// The letters a pseudoword alternates between. Fourteen consonants that
/// read cleanly beside any vowel, and the five vowels.
const CONSONANTS: [u8; 14] = *b"bdfgklmnprstvz";
const VOWELS: [u8; 5] = *b"aeiou";

/// Every word in `s` replaced by a pseudoword of the same length.
///
/// A *word* is a run of alphanumerics; everything between them -- spaces,
/// `—`, `/`, `&`, punctuation -- is the shape of the name rather than part of
/// it, and passes through where it stands. That is also what leaves
/// [`crate::description::render`]'s em dash alone.
pub(super) fn text(salt: u64, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            word.push(c);
            continue;
        }
        out.push_str(&pseudoword(salt, &word));
        word.clear();
        out.push(c);
    }
    out.push_str(&pseudoword(salt, &word));
    out
}

/// One word, keyed on itself.
///
/// Keying on the lowercased word rather than on the whole string is what
/// makes the screens hang together: the same word reads the same way
/// wherever it appears in the run, so an account named in a title and in a
/// ledger row is recognisably the same account.
///
/// A digit stays a digit and a letter stays a letter, so a code still reads
/// as a code and a year as a year. Case is copied character by character,
/// which is what keeps `CHK` shouting and `Everyday` capitalised.
fn pseudoword(salt: u64, word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    let key = hashed_text(salt, &word.to_lowercase()) as i64;
    word.chars()
        .enumerate()
        .map(|(i, c)| {
            let h = hashed(salt, key, i as i32);
            if c.is_numeric() {
                return char::from(b'0' + (h % 10) as u8);
            }
            let letters: &[u8] = if i % 2 == 0 { &CONSONANTS } else { &VOWELS };
            let letter = char::from(letters[(h % letters.len() as u64) as usize]);
            match c.is_uppercase() {
                true => letter.to_ascii_uppercase(),
                false => letter,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten thousand and a cent must not read differently in kind -- what
    /// changes is every digit, and nothing else in the string.
    #[test]
    fn a_scrambled_figure_keeps_its_punctuation_and_loses_its_digits() {
        let drawn = figure(7, Cents(123_456));
        assert_eq!(drawn.len(), "1,234.56".len());
        assert_eq!(drawn.matches(',').count(), 1);
        assert_eq!(drawn.matches('.').count(), 1);
        assert_ne!(drawn, "1,234.56");
        assert!(
            drawn
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
        );
    }

    /// The colour beside the figure already says it is negative, so the sign
    /// stays: dropping it would hide the glyph and leave the fact.
    #[test]
    fn a_scrambled_figure_keeps_the_sign_a_colour_would_have_given_away() {
        assert!(figure(7, Cents(-123_456)).starts_with('-'));
        assert!(whole_figure(7, Cents(-123_456)).starts_with('-'));
    }

    /// The Planning screen draws whole dollars and the ledger draws cents.
    /// One balance quoted twice must be one balance.
    #[test]
    fn whole_dollars_are_the_same_figure_with_its_cents_dropped() {
        let with_cents = figure(7, Cents(123_456));
        let whole = whole_figure(7, Cents(123_456));
        assert_eq!(with_cents.split('.').next().unwrap(), whole);
    }

    /// The same pairing for the truncating reading, which is the one a caller
    /// can break: it drops the cents from the value, so the key has to be
    /// taken before that and not after.
    #[test]
    fn a_truncated_figure_is_keyed_on_the_amount_it_came_from() {
        let with_cents = Cents(250_017);
        assert_eq!(
            truncated_figure(7, with_cents),
            figure(7, with_cents).split('.').next().unwrap(),
        );
        assert_ne!(
            truncated_figure(7, with_cents),
            truncated_figure(7, with_cents.trunc_to_dollar()),
            "truncating on the way in must not be the same call",
        );
    }

    /// The sign goes with the cents: a remainder of -$0.23 is nothing, and a
    /// scrambled digit under a minus would say something is owed. The
    /// untruncated reading keeps it, which is what the two are for.
    #[test]
    fn a_truncated_sub_dollar_figure_loses_its_sign_with_its_cents() {
        assert!(!truncated_figure(7, Cents(-23)).starts_with('-'));
        assert!(whole_figure(7, Cents(-23)).starts_with('-'));
    }

    /// Every screen quoting one amount quotes one scrambled amount.
    #[test]
    fn one_amount_scrambles_the_same_way_every_time() {
        assert_eq!(figure(7, Cents(123_456)), figure(7, Cents(123_456)));
    }

    /// Not a digit-for-digit substitution: one figure learned tells nothing
    /// about the next.
    #[test]
    fn two_amounts_sharing_a_digit_do_not_share_its_replacement() {
        let a = figure(7, Cents(111_111));
        let b = figure(7, Cents(211_111));
        assert_ne!(a[2..], b[2..], "{a} and {b} scrambled in lockstep");
    }

    /// The salt is what a screenshot does not carry.
    #[test]
    fn two_salts_scramble_one_amount_differently() {
        assert_ne!(figure(7, Cents(123_456)), figure(8, Cents(123_456)));
    }

    /// `0,834` reads as a rendering fault rather than as money.
    #[test]
    fn a_multi_digit_figure_never_scrambles_to_a_leading_zero() {
        for cents in 0..500i64 {
            let drawn = figure(cents as u64, Cents(123_456));
            assert!(!drawn.starts_with('0'), "{drawn}");
        }
    }

    /// Nothing about a real figure survives, including that it was nothing.
    #[test]
    fn a_zero_scrambles_like_any_other_figure() {
        assert_ne!(figure(7, Cents::ZERO), "0.00");
    }

    /// A form prefilled from a row shows that row's own scrambled figure:
    /// a form disagreeing with the row it opened on reads as a bug.
    #[test]
    fn a_typed_figure_agrees_with_the_row_it_was_prefilled_from() {
        assert_eq!(typed(7, "1,234.56"), figure(7, Cents(123_456)));
    }

    /// A half-typed figure still loses its digits and keeps its punctuation.
    #[test]
    fn text_that_is_not_yet_a_figure_still_loses_its_digits() {
        let drawn = typed(7, "12.");
        assert_eq!(drawn.len(), 3);
        assert!(drawn.ends_with('.'));
        assert_ne!(drawn, "12.");
    }

    /// A column is laid out for the name it draws, so the replacement is as
    /// wide as what it replaces.
    #[test]
    fn a_pseudonym_is_as_long_as_the_name_it_replaces() {
        for name in ["Everyday", "Rainy Day", "CHK", "Home Down Payment", "a"] {
            assert_eq!(
                text(7, name).chars().count(),
                name.chars().count(),
                "{name}"
            );
        }
    }

    /// Spaces and punctuation are the shape of a name rather than part of it.
    #[test]
    fn a_pseudonym_keeps_the_spaces_and_punctuation_around_its_words() {
        let drawn = text(7, "CHK — Everyday");
        assert_eq!(drawn.matches(' ').count(), 2);
        assert!(drawn.contains('—'));
        assert_eq!(text(7, "—"), "—");
        assert_eq!(text(7, "Mom & Dad").matches('&').count(), 1);
    }

    /// `Rainy Day` and `Rainy Fund` still share a word, so a demo reads as one
    /// person's accounts rather than as noise.
    #[test]
    fn one_word_reads_the_same_way_everywhere_in_a_run() {
        let day = text(7, "Rainy Day");
        let fund = text(7, "Rainy Fund");
        assert_eq!(
            day.split(' ').next().unwrap(),
            fund.split(' ').next().unwrap()
        );
    }

    /// A name is not its own pseudonym, and a screenshot does not carry the
    /// salt that made one.
    #[test]
    fn a_pseudonym_is_neither_the_name_nor_the_same_under_another_salt() {
        assert_ne!(text(7, "Everyday"), "Everyday");
        assert_ne!(text(7, "Everyday"), text(8, "Everyday"));
    }

    /// A code is recognisable as a code and a year as a year: what a character
    /// *is* survives, and only which one it is changes.
    #[test]
    fn a_pseudonym_keeps_case_and_keeps_a_digit_a_digit() {
        assert!(text(7, "CHK").chars().all(|c| c.is_ascii_uppercase()));
        let lego = text(7, "Lego 2026");
        let (word, year) = lego.split_once(' ').unwrap();
        assert!(word.starts_with(|c: char| c.is_uppercase()));
        assert!(word[1..].chars().all(|c| c.is_lowercase()));
        assert!(year.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(year, "2026");
    }

    /// Pronounceable is the whole point: a demo is read aloud across a table.
    #[test]
    fn a_pseudonym_alternates_consonants_and_vowels() {
        let drawn = text(7, "Everyday").to_lowercase();
        for (i, c) in drawn.chars().enumerate() {
            let vowel = VOWELS.contains(&(c as u8));
            assert_eq!(vowel, i % 2 == 1, "{drawn} breaks at {i}");
        }
    }
}
