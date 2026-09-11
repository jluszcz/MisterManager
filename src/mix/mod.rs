//! Fills `db::fund_mix` from an SEC N-PORT filing.
//!
//! [`classify`] is the whole of the feature's logic and has neither a
//! network nor a database in it -- that is what keeps the client module
//! thin, since a call the classifier never makes is a call it can't get
//! wrong.

mod classify;

pub use classify::{RawHolding, classify};
