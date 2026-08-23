//! Fixtures every module's `mod tests` builds from.
//!
//! Two things live here, and they are here for the same reason: both were
//! written out once per test module, so a change to either meant finding
//! twenty copies. What does *not* live here is anything a module chose --
//! each `today()` names a different day, picked for the schedule or the
//! deadline that module's tests turn on, and a shared one would be a fixture
//! nobody selected.
//!
//! Every figure reachable from here is invented; see the no-real-data rule in
//! `CLAUDE.md`.

use crate::db::AccountId;
use crate::db::account::{Account, Group, Kind};
use chrono::NaiveDate;

/// A date, without the `unwrap` at every call site.
pub fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap_or_else(|| panic!("{y}-{m}-{d} is not a date"))
}

/// The name `CLAUDE.md`'s fixture vocabulary pairs with a code, by kind.
///
/// The pairing is stated once here rather than at each fixture, because a
/// second reading of it -- `SAV` named `Savings`, say -- is exactly what the
/// table forbids: `Group::label` already prints that word, and an account
/// sharing a band's label makes an Overview row ambiguous to assert against.
/// An unknown code panics rather than inventing a name, since a fixture
/// reaching for one is a fixture that has left the vocabulary.
fn name_of(kind: Kind, code: &str) -> &'static str {
    let named = match kind {
        Kind::Cash => match code {
            "CHK" => Some("Everyday"),
            "SAV" => Some("Rainy Day"),
            "BKR" => Some("Brokerage"),
            "NST" => Some("Nest Egg"),
            _ => None,
        },
        Kind::Credit => match code {
            "CC1" => Some("Card One"),
            "CC2" => Some("Card Two"),
            "CC3" => Some("Card Three"),
            "CHK" => Some("Everyday Card"),
            _ => None,
        },
    };
    named.unwrap_or_else(|| {
        panic!(
            "{code} is not a {} code in CLAUDE.md's fixture table",
            kind.as_str()
        )
    })
}

/// An account as a fresh import would have written it: named for its code out
/// of the vocabulary table, banded where its kind defaults, sorted by id, and
/// no color.
///
/// A fixture wanting one of those four different says so with struct-update
/// syntax -- `Account { group: Group::Checking, ..cash(1, "CHK") }` -- so what
/// a test varies is the only thing it spells out.
fn account(id: i64, code: &str, kind: Kind) -> Account {
    Account {
        id: AccountId(id),
        code: code.into(),
        name: name_of(kind, code).into(),
        kind,
        // One less than the id, so a fixture's accounts come back in the order
        // it listed them without every caller counting.
        sort: id - 1,
        group: crate::db::account::default_group(kind),
        color: None,
    }
}

/// A cash account: `CHK`, `SAV`, `BKR` or `NST`, in the `Savings` band every
/// imported cash account starts in.
pub fn cash(id: i64, code: &str) -> Account {
    account(id, code, Kind::Cash)
}

/// A credit account: `CC1`, `CC2`, `CC3`, or `CHK` again -- the one code
/// naming two accounts, which is what `UNIQUE (code, kind)` exists for.
pub fn credit(id: i64, code: &str) -> Account {
    account(id, code, Kind::Credit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::AccountColor;

    #[test]
    fn one_code_names_a_cash_account_and_the_card_drawn_on_it() {
        assert_eq!(cash(1, "CHK").name.as_str(), "Everyday");
        assert_eq!(credit(2, "CHK").name.as_str(), "Everyday Card");
    }

    /// The band a fixture does not name is the one an import would have
    /// written, so a fixture and a seeded database describe the same account.
    #[test]
    fn an_account_defaults_to_its_kinds_band() {
        assert_eq!(cash(1, "SAV").group, Group::Savings);
        assert_eq!(credit(1, "CC1").group, Group::Credit);
    }

    #[test]
    fn struct_update_syntax_overrides_only_what_it_names() {
        let a = Account {
            group: Group::Checking,
            color: Some(AccountColor::Teal),
            ..cash(2, "NST")
        };

        assert_eq!(a.id, AccountId(2));
        assert_eq!(a.name.as_str(), "Nest Egg");
        assert_eq!(a.sort, 1);
        assert_eq!(a.group, Group::Checking);
        assert_eq!(a.color, Some(AccountColor::Teal));
    }

    #[test]
    #[should_panic(expected = "XYZ is not a cash code")]
    fn a_code_outside_the_vocabulary_panics() {
        cash(1, "XYZ");
    }
}
