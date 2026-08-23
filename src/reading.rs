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

/// How a reader treats a row it cannot resolve.
///
/// [`Strict`](Reading::Strict) is every path that *spends* a figure:
/// `transfer::plan`, both worksheet prefills, and `plan::remaining` behind
/// every Planning gate. Money is about to move on the strength of the number,
/// so the owner is told the moment it cannot be derived -- an unset
/// `key::TAX_RATE` under a goal flagged taxed, or a destination key naming a
/// goal that no longer exists, are corrupt state rather than features
/// switched off.
///
/// [`Tolerant`](Reading::Tolerant) is every path that only *draws* one. A
/// screen cannot decline to render itself -- that is the failure
/// `transfer::wiring` exists to prevent, since a corrupt key is often the
/// very thing the drawing has to explain. It is sharper still on Savings,
/// where `App::reload_savings` runs inside `App::new`: a strict read there
/// stops the application starting, and the tax rate is set from inside the
/// application, leaving no way back.
///
/// What tolerance covers is the row, never the read. A query that fails is an
/// error under both readings; only a row whose *meaning* cannot be resolved
/// is skipped, or falls back to the figure the table actually holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Refuse: a row that cannot be resolved is an error naming what is
    /// missing.
    Strict,
    /// Read past it: the row is skipped, or takes the stored figure the
    /// derivation would have been made from.
    Tolerant,
}
