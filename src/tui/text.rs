//! A line of text being typed, and the keys that edit it.
//!
//! One buffer serves every box in the app: a form's [`Field`], and the
//! [`SearchBox`] behind `/`. What they add to it is what differs -- a field
//! remembers whether the user has touched it, a search box whether it is
//! open -- and the caret, the editing, and the keys that drive them are the
//! same in both.
//!
//! [`Field`]: super::form::Field
//! [`SearchBox`]: super::search::SearchBox

pub(super) use jluszcz_finance_utils::tui::text::{Edit, TextBuffer, edit_key, is_bare};
