//! Every Planning line, and where its money goes.
//!
//! Built the way [`crate::gate::Gate`] is: one enum owning every half of a
//! pairing nothing else checks. A line's label, the field of
//! [`Lines`] it reads, the setting key naming its destination, and — for those
//! matched at import — the goal-name substring, all derive from the one
//! value, so none of them can be paired with another line's.

use crate::calc::planning::Lines;
use crate::db::setting::Key;
use crate::db::{AccountId, GoalId};
use crate::gate::Gate;
use crate::money::Cents;

/// One line of the Planning waterfall's allocation.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Line {
    Bills,
    CurrentHousing,
    Goals,
    Roth,
    FutureHousing,
    MomAndDad,
    EmergencyFund,
    Retirement,
    Investment,
}

/// Where a line's money lands.
///
/// An unset key means the money leaves the tracked system — a withdrawal from
/// checking. A key pointing at a goal or account that no longer exists is a
/// corrupt database and must be a loud error, never a silently reinterpreted
/// withdrawal: that would move real money to the wrong place.
#[derive(Copy, Clone, Debug)]
pub enum Destination {
    /// The container the named goal sits in. A goal already records where its
    /// money is, in `goal.container_account_id`, so reading the destination
    /// off the goal means the two cannot disagree.
    Goal(Key<GoalId>),
    /// An account named directly, for the lines with no goal behind them.
    Account(Key<AccountId>),
    /// The plug, spread over the open goals no other line claims.
    Spread,
}

impl Line {
    /// Every line, in the order the screens list them.
    pub const ALL: [Line; 9] = [
        Line::Bills,
        Line::CurrentHousing,
        Line::Goals,
        Line::Roth,
        Line::FutureHousing,
        Line::MomAndDad,
        Line::EmergencyFund,
        Line::Retirement,
        Line::Investment,
    ];

    /// The name the screens show.
    ///
    /// Not the name of the goal this line funds, and not derivable from it:
    /// `Bills` is the other monthly bills *plus* the bill-payment allocation,
    /// while the goal it lands in is named "Bill Payments".
    pub fn label(self) -> &'static str {
        match self {
            Line::Bills => "Bills",
            Line::CurrentHousing => "Current Housing",
            Line::Goals => "Goals",
            Line::Roth => "Roth",
            Line::FutureHousing => "Future Housing",
            Line::MomAndDad => "Mom & Dad",
            Line::EmergencyFund => "Emergency Fund",
            Line::Retirement => "Retirement",
            Line::Investment => "Investment",
        }
    }

    /// What this line moves, out of a computed plan.
    ///
    /// The exhaustive match that ties this enum to `calc`'s plainly-named
    /// fields. `calc` cannot name `Line` — that would put a `Key<GoalId>`,
    /// and so the database, inside a module that must not have one.
    pub fn amount(self, lines: &Lines) -> Cents {
        match self {
            Line::Bills => lines.bills,
            Line::CurrentHousing => lines.current_housing,
            Line::Goals => lines.goals,
            Line::Roth => lines.roth,
            Line::FutureHousing => lines.future_housing,
            Line::MomAndDad => lines.mom_and_dad,
            Line::EmergencyFund => lines.emergency_fund,
            Line::Retirement => lines.retirement,
            Line::Investment => lines.investment,
        }
    }

    /// Where this line's money goes.
    ///
    /// `Line::FutureHousing`'s key is what makes it a down payment rather
    /// than a mortgage payment: set, the money lands in the Brokerage goal;
    /// unset, it leaves as a withdrawal. That switch is the reason this
    /// module exists.
    pub fn destination(self) -> Destination {
        match self {
            Line::Bills => Destination::Goal(Key::new("planning.goal.bill_payments_id")),
            Line::CurrentHousing => Destination::Goal(Key::new("planning.goal.housing_id")),
            Line::Goals => Destination::Spread,
            Line::Roth => Destination::Goal(Gate::Roth.key()),
            // The stored string still says `down_payment`: it is the key an
            // existing database already holds this goal's id under, written
            // there when it was a gate.
            Line::FutureHousing => Destination::Goal(Key::new("planning.goal.down_payment_id")),
            Line::MomAndDad => Destination::Goal(Key::new("planning.goal.mom_and_dad_id")),
            Line::EmergencyFund => Destination::Goal(Gate::EmergencyFund.key()),
            Line::Retirement => Destination::Account(Key::new("planning.account.retirement_id")),
            Line::Investment => Destination::Account(Key::new("planning.account.investment_id")),
        }
    }

    /// The key and goal-name substring this line is matched by at import.
    ///
    /// `None` for the two lines whose goal a [`Gate`] already matches, and
    /// for the plug, which resolves through goals rather than a key. Name
    /// matching happens at import, once, and never at read time: goal names
    /// are not unique.
    pub fn owned_goal(self) -> Option<(Key<GoalId>, &'static str)> {
        let substring = match self {
            Line::Bills => "Bill Payments",
            Line::CurrentHousing => "Housing",
            Line::FutureHousing => "Down Payment",
            Line::MomAndDad => "Mom & Dad",
            _ => return None,
        };
        match self.destination() {
            Destination::Goal(key) => Some((key, substring)),
            _ => unreachable!("a line with a goal name must have a goal destination"),
        }
    }

    /// The gate whose goal this line funds, for the two lines a gate matches.
    ///
    /// They borrow the gate's key rather than declaring their own, so the
    /// line and the gate cannot come to name two different goals. The cost of
    /// that sharing is that writing the key is never only a destination
    /// change -- it decides whether the gate fires -- so the callers that
    /// *write* a destination have to know which lines these are.
    pub fn gate(self) -> Option<Gate> {
        match self {
            Line::Roth => Some(Gate::Roth),
            Line::EmergencyFund => Some(Gate::EmergencyFund),
            _ => None,
        }
    }

    /// The substring this line's goal name must contain at import.
    ///
    /// [`Line::owned_goal`]'s, or the gate's for the two lines whose goal a
    /// [`Gate`] matches — so every goal-backed line answers, which
    /// `owned_goal` alone does not. `None` for the plug and for the two lines
    /// naming an account, neither of which is matched by name at all.
    ///
    /// Advisory at read time and nothing more. Name matching that *resolves*
    /// a destination still happens exactly once, at import: goal names are
    /// not unique, so a reader that keyed on one would pick between the
    /// goals called "Lego" by luck.
    pub fn import_substring(self) -> Option<&'static str> {
        match self.gate() {
            Some(gate) => Some(gate.substring()),
            None => self.owned_goal().map(|(_, substring)| substring),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` is written out by hand, so a line added to the enum without
    /// being added here would silently drop out of every caller that
    /// iterates it — including the one that decides where money goes.
    #[test]
    fn all_covers_every_variant() {
        for line in Line::ALL {
            match line {
                Line::Bills
                | Line::CurrentHousing
                | Line::Goals
                | Line::Roth
                | Line::FutureHousing
                | Line::MomAndDad
                | Line::EmergencyFund
                | Line::Retirement
                | Line::Investment => {}
            }
        }
        assert_eq!(Line::ALL.len(), 9);
    }

    /// The suggestion the Planning screen offers for an unset line is only
    /// as good as this: a line with a goal behind it and no substring can
    /// never be suggested, and the miss looks like "nothing to suggest"
    /// rather than a bug.
    #[test]
    fn every_goal_backed_line_has_an_import_substring() {
        for line in Line::ALL {
            let expected = matches!(line.destination(), Destination::Goal(_));
            assert_eq!(
                line.import_substring().is_some(),
                expected,
                "{line:?} {:?}",
                line.import_substring()
            );
        }
    }

    /// The other half of what the gate pairs. `destination` and
    /// `import_substring` are two separate matches on `self`, so a line could
    /// take one gate's key and the other's text -- the exact mispairing
    /// `Gate` exists to make impossible, and the sibling test above only
    /// covers the key.
    #[test]
    fn a_gate_backed_line_takes_that_gates_substring() {
        assert_eq!(Line::Roth.import_substring(), Some(Gate::Roth.substring()));
        assert_eq!(
            Line::EmergencyFund.import_substring(),
            Some(Gate::EmergencyFund.substring())
        );
    }

    /// Two lines sharing a destination key would send one line's money where
    /// the other's belongs, with nothing in the output to show it.
    #[test]
    fn every_configured_line_has_its_own_destination_key() {
        let mut names: Vec<&str> = Line::ALL
            .iter()
            .filter_map(|line| match line.destination() {
                Destination::Goal(key) => Some(key.name()),
                Destination::Account(key) => Some(key.name()),
                Destination::Spread => None,
            })
            .collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "two lines share a destination key");
    }

    /// Exactly one line is the plug. Two would each be "whatever the others
    /// leave", which is not a definition.
    #[test]
    fn exactly_one_line_is_the_spread() {
        let spreads = Line::ALL
            .iter()
            .filter(|l| matches!(l.destination(), Destination::Spread))
            .count();
        assert_eq!(spreads, 1);
    }

    /// The screens show these, and two lines under one label is a screen
    /// nobody can read.
    #[test]
    fn every_line_has_its_own_label() {
        let mut labels: Vec<&str> = Line::ALL.iter().map(|l| l.label()).collect();
        let before = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), before, "two lines share a label");
    }

    /// The two gate-backed lines borrow the gate's key rather than declaring
    /// their own. A second copy of the string would be a second thing to keep
    /// in step, and the gate is the one that reads it at import.
    #[test]
    fn the_gate_backed_lines_resolve_through_the_gates_own_key() {
        match Line::Roth.destination() {
            Destination::Goal(key) => assert_eq!(key.name(), Gate::Roth.key().name()),
            other => panic!("Roth resolves to {other:?}"),
        }
        match Line::EmergencyFund.destination() {
            Destination::Goal(key) => {
                assert_eq!(key.name(), Gate::EmergencyFund.key().name())
            }
            other => panic!("Emergency Fund resolves to {other:?}"),
        }
    }

    /// A line matched at import owns both halves — the key and the text — so
    /// they cannot be paired wrongly. The gate-backed lines own neither: the
    /// gate does.
    #[test]
    fn only_the_lines_matched_at_import_own_a_goal_name() {
        let owned: Vec<&str> = Line::ALL
            .iter()
            .filter_map(|l| l.owned_goal().map(|(_, sub)| sub))
            .collect();
        assert_eq!(
            owned,
            vec!["Bill Payments", "Housing", "Down Payment", "Mom & Dad"]
        );
        assert!(Line::Roth.owned_goal().is_none());
        assert!(Line::EmergencyFund.owned_goal().is_none());
        assert!(Line::Goals.owned_goal().is_none());
    }

    /// `amount` is the exhaustive match that ties the enum to `Lines`'s
    /// fields, and reading two lines off the same field would double one
    /// destination and starve another.
    #[test]
    fn every_line_reads_its_own_field_of_lines() {
        let lines = Lines {
            bills: Cents(1),
            current_housing: Cents(2),
            goals: Cents(3),
            roth: Cents(4),
            future_housing: Cents(5),
            mom_and_dad: Cents(6),
            emergency_fund: Cents(7),
            retirement: Cents(8),
            investment: Cents(9),
        };
        let mut amounts: Vec<i64> = Line::ALL.iter().map(|l| l.amount(&lines).0).collect();
        amounts.sort_unstable();
        assert_eq!(amounts, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(
            Line::ALL.iter().map(|l| l.amount(&lines)).sum::<Cents>(),
            lines.total()
        );
    }
}
