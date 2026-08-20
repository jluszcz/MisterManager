//! The goals the Planning waterfall gates on.

use crate::db::GoalId;
use crate::db::setting::Key;

/// One Planning gate.
///
/// The setting key, the goal-name substring, and the label all derive from
/// this one value. The key and the substring are two halves of a pairing that
/// has no other check on it: a gate recorded under another gate's key
/// resolves to the wrong balance and changes where roughly half the
/// waterfall's money goes, with nothing in the output to show it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Gate {
    EmergencyFund,
    Roth,
}

impl Gate {
    /// Every gate, for callers that must cover them all.
    pub const ALL: [Gate; 2] = [Gate::EmergencyFund, Gate::Roth];

    /// The `setting` key holding this gate's goal id.
    ///
    /// The emergency gate's stored string says `emergency_id`. It is the key
    /// an existing database already holds the id under, and a renamed key
    /// reads as "not configured", which switches the gate off silently.
    pub fn key(self) -> Key<GoalId> {
        match self {
            Gate::EmergencyFund => Key::new("planning.goal.emergency_id"),
            Gate::Roth => Key::new("planning.goal.roth_id"),
        }
    }

    /// The name the screens show.
    pub fn label(self) -> &'static str {
        match self {
            Gate::EmergencyFund => "Emergency Fund",
            Gate::Roth => "Roth",
        }
    }

    /// The substring this gate's goal name must contain.
    ///
    /// Matched with `contains` rather than equality, and deliberately shorter
    /// than [`Gate::label`]: the live sheet names these "Roth IRA" and
    /// "Emergency Savings", so the match text is the part every plausible
    /// name shares. A substring that stops matching produces no error at
    /// import — just a key that reads as "not configured".
    pub fn substring(self) -> &'static str {
        match self {
            Gate::EmergencyFund => "Emergency",
            Gate::Roth => "Roth",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two gates sharing a key would write one gate's goal id over the
    /// other's, leaving one gate silently off and the other watching the
    /// wrong balance. A duplicated arm in `key` is a plausible way to get
    /// there and nothing else would catch it.
    #[test]
    fn every_gate_has_its_own_key() {
        let mut keys: Vec<&str> = Gate::ALL.iter().map(|g| g.key().name()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two gates share a setting key");
    }

    /// Likewise for the substrings: two gates matching the same text would
    /// make both resolve to whichever goal the sheet lists first.
    #[test]
    fn every_gate_has_its_own_substring() {
        let mut subs: Vec<&str> = Gate::ALL.iter().map(|g| g.substring()).collect();
        subs.sort_unstable();
        let before = subs.len();
        subs.dedup();
        assert_eq!(subs.len(), before, "two gates share a name substring");
    }

    /// `ALL` is written out by hand, so a gate added to the enum without
    /// being added here would silently drop out of every caller that
    /// iterates it.
    #[test]
    fn all_covers_every_variant() {
        // The match is exhaustive, so adding a variant stops this compiling
        // until it is added to `ALL` and counted here too.
        for gate in Gate::ALL {
            match gate {
                Gate::EmergencyFund | Gate::Roth => {}
            }
        }
        assert_eq!(Gate::ALL.len(), 2);
    }

    /// The match text is what the sheet's goal name must contain; the label
    /// is what the screens show. They are allowed to differ — the live sheet
    /// says "Emergency Savings" — but a match text the label itself would
    /// fail is a text that matches nothing.
    #[test]
    fn every_gates_substring_is_contained_in_its_label() {
        for gate in Gate::ALL {
            assert!(
                gate.label().contains(gate.substring()),
                "{:?}: label {:?} does not contain substring {:?}",
                gate,
                gate.label(),
                gate.substring()
            );
        }
    }
}
