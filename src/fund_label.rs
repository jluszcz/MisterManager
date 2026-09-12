//! What a fund's filed name reads as, in any medium.
//!
//! SEC's `genInfo/seriesName` is written for a regulator rather than for a
//! column: it ends in the word the column is headed with, and some filers
//! shout the whole of it. One rule rather than one per sink, the same split
//! [`crate::description`] makes for a transaction's text and
//! [`crate::palette`] makes for color: the Funds screen draws these today and
//! the report's holdings table is the obvious second reader, and a name
//! shortened two ways would read as two funds.
//!
//! **The stored name is never touched.** `db::fund_mix` holds what the filing
//! said, so a rule changed here redraws every row on the next frame rather
//! than needing a refetch -- and nothing here can lose a fact the filing
//! carried.
//!
//! **What is deliberately *not* here is a rule that drops the issuer.** It is
//! the least informative word in most of these names, and dropping it is what
//! would make the column narrow; the words that do the dropping are the fund
//! families the owner holds, which are brokerages, and no institution they
//! hold may be named in this repository. The column is sized for the whole
//! name instead -- see `tui::fund::FUND_NAME_WIDTH`.

/// A filed name with its trailing `Fund` dropped and its shouting undone, or
/// the name as the filer cased it.
///
/// The two rules run in that order, the trim being about a word and the
/// casing about how what is left was typed.
pub fn short(name: &str) -> String {
    title_case(trim_fund(name.trim()))
}

/// `name` without a trailing `Fund`, or `name` unchanged.
///
/// Every row in the column is a fund, and the column is headed `Fund`, so the
/// word is the one part of a filed name carrying no information at all --
/// while costing five characters at the end, which on this screen is where
/// the informative part of a fund name lives. `… Target Retirement 2045 Fund`
/// is told from the row above it by the year, and the year is what a cut
/// takes first.
///
/// Matched case-insensitively, since a filer who shouts shouts this too, and
/// only where a space precedes it -- `Growth Funds` and a fund actually named
/// `Fund` keep every letter. The last is the same refusal an issuer-only name
/// would earn: a blank cell says less than a redundant one.
///
/// Once, not repeatedly. `X Fund Fund` is not a name anyone files, and a loop
/// here would be a rule about one.
fn trim_fund(name: &str) -> &str {
    let Some(head) = name.get(..name.len().saturating_sub(FUND_SUFFIX.len())) else {
        return name;
    };
    let Some(tail) = name.get(head.len()..) else {
        return name;
    };
    match tail.eq_ignore_ascii_case(FUND_SUFFIX) && !head.is_empty() {
        true => head,
        false => name,
    }
}

/// The trailing word [`trim_fund`] drops, space included -- the space is what
/// makes it a word rather than the end of one.
const FUND_SUFFIX: &str = " Fund";

/// A shouted name in title case, or the name as the filer cased it.
///
/// **Only a name that is shouting is rewritten**, which is the whole of the
/// gate: a filer who wrote `Freedom Index 2045` has already made every casing
/// decision in it, and a title-caser run over that can only disagree with
/// one. A name with no lowercase letter anywhere has made no decisions to
/// disagree with.
///
/// Inside one, a word is title-cased only if it carries [`ACRONYM_LETTERS`]
/// letters or more. Shorter all-caps runs are initialisms rather than shouted
/// words -- `ETF`, `S&P`, `U.S.` -- and `S&p` is a worse answer than leaving
/// them be. Digits and punctuation are carried through whatever word they sit
/// in, so `2045` and `500` are never touched at all.
fn title_case(name: &str) -> String {
    if name.chars().any(char::is_lowercase) {
        return name.to_string();
    }
    name.split(' ')
        .map(
            |word| match word.chars().filter(|c| c.is_alphabetic()).count() {
                n if n >= ACRONYM_LETTERS => capitalize(word),
                _ => word.to_string(),
            },
        )
        .collect::<Vec<String>>()
        .join(" ")
}

/// The longest all-caps run still read as an initialism rather than a shouted
/// word, plus one.
///
/// Three covers the ones a fund name actually carries -- `ETF`, `S&P`, `USA`,
/// `U.S.` -- and four is where words start: `BOND`, `REIT`, `PLUS`. `REIT` is
/// the known cost, drawn as `Reit` in a name that shouts; it is one word in
/// one rare fund against every four-letter word in every common one.
const ACRONYM_LETTERS: usize = 4;

/// The word's first letter upper, the rest lower -- leading punctuation
/// carried through, so `(FUND)` capitalizes the `F` rather than giving up at
/// the bracket.
fn capitalize(word: &str) -> String {
    let mut done = false;
    word.chars()
        .map(|c| match c.is_alphabetic() && !done {
            true => {
                done = true;
                c.to_ascii_uppercase()
            }
            false => c.to_ascii_lowercase(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the rules exist for: a filer who shouts, and a last word the
    /// column heading already said.
    #[test]
    fn a_shouted_name_stops_shouting_and_drops_the_word_the_column_is_headed_with() {
        assert_eq!(
            short("ACME TARGET RETIREMENT 2045 FUND"),
            "Acme Target Retirement 2045",
        );
    }

    /// A filer who cased their own name made every decision in it, and the
    /// only thing a title-caser can do to it is disagree. The trailing word
    /// still goes: that rule is about the word, not about how it was typed.
    #[test]
    fn a_name_already_cased_keeps_every_letter_the_filer_chose() {
        assert_eq!(
            short("Borealis Freedom Index 2045 Fund"),
            "Borealis Freedom Index 2045"
        );
        assert_eq!(
            short("Acme iShares Core Fund"),
            "Acme iShares Core",
            "a deliberate lowercase initial did not survive"
        );
    }

    /// Short all-caps runs are initialisms, not shouting -- `S&p 500` is a
    /// worse answer than leaving them alone, and the digits beside them were
    /// never anyone's to case.
    #[test]
    fn an_initialism_survives_a_name_that_is_shouting_around_it() {
        assert_eq!(short("ACME S&P 500 INDEX ETF"), "Acme S&P 500 Index ETF");
        assert_eq!(short("ACME U.S. BOND FUND"), "Acme U.S. Bond");
    }

    /// A word, not the end of one, and only at the end: `Funds` is a
    /// different word and a `Fund` in the middle is part of the name.
    #[test]
    fn only_a_trailing_fund_is_trimmed_and_only_as_a_whole_word() {
        assert_eq!(short("Acme Growth Funds"), "Acme Growth Funds");
        assert_eq!(short("Acme Superfund"), "Acme Superfund");
        assert_eq!(short("Acme Fund of Choices"), "Acme Fund of Choices");
    }

    /// Trimming to nothing is the one case where the redundant word is worth
    /// keeping -- the same refusal a name that is only its issuer earns.
    #[test]
    fn a_name_that_is_only_the_word_fund_keeps_it() {
        assert_eq!(short("Fund"), "Fund");
        assert_eq!(short("FUND"), "Fund");
    }

    /// One word past the trim, and the casing rule still applies to it.
    #[test]
    fn a_single_word_name_is_cased_like_any_other() {
        assert_eq!(short("ACME"), "Acme");
        assert_eq!(short("Acme"), "Acme");
    }
}
