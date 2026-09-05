//! Whether a reader refuses a row it cannot resolve, or draws past it.
//!
//! The same query is wanted two ways, and which way follows from what the
//! caller is about to do with the answer rather than from what it is reading.
//! So it is a parameter and the reader is one function -- the split lives at
//! the one line that can act on it, and a reading cannot come to differ from
//! its twin in anything but the thing it names.
//!
//! Its readers are [`crate::goal::all_with_balances`] and the ones in
//! [`crate::transfer`] that build on it, which is why it sits here rather
//! than in either: `goal`'s unresolvable row is a taxed goal with no rate on
//! record, `transfer`'s is a setting key naming a goal that is gone, and the
//! rule for what to do about one is the same rule.
//!
//! What tolerance covers is the row, never the read. A query that fails is an
//! error under both readings; only a row whose *meaning* cannot be resolved
//! is skipped, or falls back to the figure the table actually holds.

/// How a reader treats a row it cannot resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Refuse: a row that cannot be resolved is an error naming what is
    /// missing.
    ///
    /// Every path that *spends* a figure reads this way, because money is
    /// about to move on the strength of the number.
    Strict,
    /// Read past it: the row is skipped, or takes the stored figure the
    /// derivation would have been made from.
    ///
    /// Every path that only *draws* one reads this way, because a screen
    /// cannot decline to render itself -- and a corrupt key is often the very
    /// thing the drawing has to explain. Sharper still on Savings, where
    /// `App::reload_savings` runs inside `App::new`: a strict read there stops
    /// the application starting, and the tax rate is set from inside the
    /// application, leaving no way back.
    Tolerant,
}
