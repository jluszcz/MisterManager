//! The goals the Planning waterfall gates on.

use crate::calc::planning::GATES;
use crate::db::setting::{self, Key, Value};
use crate::db::{Db, GoalId};
use crate::plan_line::Line;
use anyhow::{Result, anyhow, ensure};

/// One Planning gate.
///
/// [`Priority`] is the only thing that tells two gates apart to the waterfall:
/// how much one takes, what it takes it from and how it is drawn are the same
/// for all of them.
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
    /// Every gate, for callers that must cover them all, in the priority order a
    /// database that has not been given one takes.
    pub const ALL: [Gate; GATES] = [Gate::EmergencyFund, Gate::Roth];

    /// What `calc` knows this gate by: its place in [`Gate::ALL`], whatever its
    /// priority.
    pub fn index(self) -> usize {
        self as usize
    }

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

    /// How [`Priority`] spells this gate in the database.
    fn token(self) -> &'static str {
        match self {
            Gate::EmergencyFund => "emergency",
            Gate::Roth => "roth",
        }
    }

    /// The key holding the name the owner gave this gate. Unset is
    /// [`Gate::label`].
    pub fn name_key(self) -> Key<String> {
        match self {
            Gate::EmergencyFund => Key::new("planning.gate_name.emergency"),
            Gate::Roth => Key::new("planning.gate_name.roth"),
        }
    }

    /// The name the screens show for a gate the owner has not named.
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

/// The longest name a gate may take, which is what the Planning screen's label
/// column and the goal list's title hold beside everything else they draw.
pub const NAME_LIMIT: usize = 18;

/// What every gate is called: the owner's name where there is one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Names([Option<String>; GATES]);

impl Names {
    pub fn load(db: &Db) -> Result<Names> {
        let mut names = Names::default();
        for gate in Gate::ALL {
            names.0[gate.index()] = setting::get(db, gate.name_key())?;
        }
        Ok(names)
    }

    /// The name itself, for a form to prefill and a write to check.
    pub fn of(&self, gate: Gate) -> &str {
        self.0[gate.index()].as_deref().unwrap_or(gate.label())
    }

    /// The name on its way to a display. Only the owner's own is masked: the
    /// label is the app's word.
    pub fn drawn(&self, gate: Gate) -> String {
        match &self.0[gate.index()] {
            Some(name) => crate::demo::text(name).into_owned(),
            None => gate.label().to_string(),
        }
    }

    /// Write back what [`Names::load`] read, for `mm import`.
    pub fn store(&self, db: &Db) -> Result<()> {
        for gate in Gate::ALL {
            if let Some(name) = &self.0[gate.index()] {
                setting::set(db, gate.name_key(), name.clone())?;
            }
        }
        Ok(())
    }
}

/// Name a gate, or give it back its own label with an empty name.
///
/// Refuses a name another Planning line answers to: the two would be one label
/// over two rows. A gate's own label is refused the same way, since another
/// gate may have been given it in the meantime.
pub fn rename(db: &Db, gate: Gate, name: &str) -> Result<()> {
    let name = match name.trim() {
        "" => gate.label(),
        name => name,
    };
    ensure!(
        name.chars().count() <= NAME_LIMIT,
        "a gate's name is at most {NAME_LIMIT} characters"
    );
    let names = Names::load(db)?;
    for line in Line::ALL {
        let taken = match line.gate() {
            Some(other) if other == gate => continue,
            Some(other) => names.of(other),
            None => line.label(),
        };
        ensure!(
            taken.to_lowercase() != name.to_lowercase(),
            "another Planning line is already called that"
        );
    }
    if name == gate.label() {
        return setting::clear(db, gate.name_key());
    }
    setting::set(db, gate.name_key(), name.to_string())
}

/// The order the gates draw in, as the owner arranged it.
///
/// Not in any sheet, so `mm import` carries it across a `--replace` the way it
/// does the Accounts screen's settings.
pub const PRIORITY: Key<Priority> = Key::new("planning.gate_priority");

/// Every gate once, highest priority first.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Priority([Gate; GATES]);

impl Default for Priority {
    fn default() -> Priority {
        Priority(Gate::ALL)
    }
}

impl Priority {
    pub fn gates(&self) -> &[Gate; GATES] {
        &self.0
    }

    /// The order as `calc` takes it.
    pub fn indices(&self) -> [usize; GATES] {
        self.0.map(Gate::index)
    }

    /// The same order with `gate` at position `to`.
    pub fn moved(mut self, gate: Gate, to: usize) -> Priority {
        let from = self.0.iter().position(|g| *g == gate);
        let from = from.expect("a priority holds every gate");
        if from < to {
            self.0[from..=to].rotate_left(1);
        } else {
            self.0[to..=from].rotate_right(1);
        }
        self
    }
}

impl Value for Priority {
    fn encode(&self) -> String {
        self.0.map(Gate::token).join(",")
    }

    /// A gate the stored order does not name takes its place after the ones it
    /// does, so an order written before a gate existed still reads. A name it
    /// does not know, or one twice, is corrupt: which gate draws first moves
    /// real money.
    fn decode(raw: &str) -> Result<Priority> {
        let mut found: Vec<Gate> = Vec::with_capacity(GATES);
        for token in raw.split(',') {
            let gate = Gate::ALL
                .into_iter()
                .find(|g| g.token() == token && !found.contains(g))
                .ok_or_else(|| anyhow!("is not an order of the gates: {raw:?}"))?;
            found.push(gate);
        }
        for gate in Gate::ALL {
            if !found.contains(&gate) {
                found.push(gate);
            }
        }
        found
            .try_into()
            .map(Priority)
            .map_err(|_| anyhow!("is not an order of the gates: {raw:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_takes_a_name_and_gives_it_back_when_emptied() {
        let db = crate::db::open_in_memory().unwrap();
        rename(&db, Gate::Roth, " Nest Egg ").unwrap();
        let names = Names::load(&db).unwrap();
        assert_eq!(names.of(Gate::Roth), "Nest Egg");
        assert_eq!(names.of(Gate::EmergencyFund), "Emergency Fund");
        rename(&db, Gate::Roth, "").unwrap();
        assert_eq!(Names::load(&db).unwrap(), Names::default());
    }

    #[test]
    fn a_name_another_line_answers_to_or_past_the_limit_is_refused() {
        let db = crate::db::open_in_memory().unwrap();
        let long = "x".repeat(NAME_LIMIT + 1);
        for name in ["investment", "emergency fund", long.as_str()] {
            assert!(rename(&db, Gate::Roth, name).is_err(), "{name:?}");
        }
        assert_eq!(Names::load(&db).unwrap(), Names::default());
    }

    #[test]
    fn a_gate_cannot_take_its_own_label_back_from_a_gate_now_called_it() {
        let db = crate::db::open_in_memory().unwrap();
        rename(&db, Gate::Roth, "Nest Egg").unwrap();
        rename(&db, Gate::EmergencyFund, "Roth").unwrap();
        for name in ["", "Roth"] {
            assert!(rename(&db, Gate::Roth, name).is_err(), "{name:?}");
        }
        assert_eq!(Names::load(&db).unwrap().of(Gate::Roth), "Nest Egg");
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_masks_the_owners_name_and_not_the_apps_word() {
        crate::demo::install_with_salt(7);
        let db = crate::db::open_in_memory().unwrap();
        rename(&db, Gate::Roth, "Nest Egg").unwrap();
        let names = Names::load(&db).unwrap();
        assert_ne!(names.drawn(Gate::Roth), "Nest Egg");
        assert_eq!(names.drawn(Gate::EmergencyFund), "Emergency Fund");
    }

    #[test]
    fn an_order_moves_and_round_trips() {
        let swapped = Priority::default().moved(Gate::Roth, 0);
        assert_eq!(swapped.gates(), &[Gate::Roth, Gate::EmergencyFund]);
        assert_eq!(swapped.moved(Gate::Roth, 1), Priority::default());
        assert_eq!(Priority::decode(&swapped.encode()).unwrap(), swapped);
    }

    #[test]
    fn an_order_missing_a_gate_reads_with_it_last() {
        assert_eq!(
            Priority::decode("roth").unwrap().gates(),
            &[Gate::Roth, Gate::EmergencyFund]
        );
    }

    #[test]
    fn an_order_naming_a_stranger_or_one_gate_twice_is_an_error() {
        for raw in ["", "roth,nope", "roth,roth"] {
            assert!(Priority::decode(raw).is_err(), "{raw:?}");
        }
    }

    /// `ALL` is written out by hand beside the declaration, and a pair out of
    /// step would read one gate's progress and draw it under the other's name.
    #[test]
    fn every_gates_index_is_its_place_in_all() {
        for (i, gate) in Gate::ALL.into_iter().enumerate() {
            assert_eq!(gate.index(), i, "{gate:?}");
        }
    }

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
        assert_eq!(Gate::ALL.len(), GATES);
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
