//! The filter every list screen opens on `/`: finance-utils' `tui::search`,
//! and the needle drawn the way this application draws a caret.
//!
//! Every screen filters in memory. The Ledger's rows come out of SQL, but the
//! query is bounded by the window before a needle is typed, so the rows in
//! hand are the only rows a needle could reach -- see the [`Search`] impl on
//! [`Ledger`].
//!
//! [`Ledger`]: super::ledger::Ledger

pub(super) use jluszcz_finance_utils::tui::search::{
    Matcher, Search, SearchBox, escape_kept_filter, search_key,
};

use super::Label;
use super::form::Caret;
use super::widget::value_spans;
use ratatui::text::Span;

/// The needle as a screen draws it: the text, with the caret over one
/// character of it while the box is open -- see [`SearchBox::caret`].
pub(super) fn search_spans(target: &impl Search) -> Vec<Span<'static>> {
    let box_ = target.search_box();
    let caret = box_.caret().map(|at| Caret::In {
        at,
        text: box_.needle().to_string(),
    });
    value_spans(&Label::from(box_.needle()), caret)
}

#[cfg(all(test, feature = "demo"))]
mod tests {
    use super::*;
    use crate::money::Cents;
    use jluszcz_finance_utils::tui::search::searchable_amount;

    /// A needle is matched against the figure itself, never against what the
    /// screen draws -- so `mm --demo` narrows a list by amount exactly as an
    /// ordinary run does. Routing this through `demo` would turn every row's
    /// key into the mask and quietly leave the owner unable to find anything
    /// while demonstrating the app. The needle stays visible for the same
    /// reason: it is a query the owner is typing, not a figure off a row.
    #[test]
    fn a_demo_still_finds_a_row_by_its_amount() {
        crate::demo::install_with_salt(7);
        let amounts = [Cents(123_456), Cents(-5_000)];
        assert_eq!(searchable_amount(Cents(123_456)), "1234.56");
        assert!(Matcher::new("1234").matches("Lego", &amounts));
        assert!(!Matcher::new("9999").matches("Lego", &amounts));
    }
}
