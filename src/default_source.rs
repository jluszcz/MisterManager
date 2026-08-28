//! Which account a money form's `From` opens on, and the two forms that have
//! one.
//!
//! `t` and `p` both move cash out of an account the owner holds, and both
//! used to open on whichever account sorted first. On the cash ledger that is
//! also the account the `To` selector opens on, so `t` opened on a transfer
//! from an account to itself -- a form whose first two fields are a
//! contradiction the owner has to notice and undo.
//!
//! Which account each starts from is a fact about how the owner banks rather
//! than anything the workbook carries -- no cell names it, the same as the
//! two `Savings` blocks -- so it is configured on the Accounts screen and
//! read back from `setting`.
//!
//! One value owns both halves for [`crate::savings_block::Block`]'s reason:
//! a key and what it means cannot be paired wrongly if they arrive together.
//! The two are separate keys rather than one, because the account a card is
//! paid from and the account savings leave are two different decisions --
//! which is also what lets one account answer for both.

use crate::db::AccountId;
use crate::db::setting::Key;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Source {
    /// `t` on the cash ledger: the account a transfer leaves.
    Transfer,
    /// `p` on either ledger: the account a card is paid from.
    Payment,
}

impl Source {
    /// Every source, for callers that must cover both.
    pub const ALL: [Source; 2] = [Source::Transfer, Source::Payment];

    /// The `setting` key holding the account this form's `From` opens on.
    pub fn key(self) -> Key<AccountId> {
        match self {
            Source::Transfer => Key::new("defaults.transfer_from_account_id"),
            Source::Payment => Key::new("defaults.payment_from_account_id"),
        }
    }

    /// What the Accounts screen calls this default: the key that opens the
    /// form, in the words the ledger's own footer uses for it.
    pub fn label(self) -> &'static str {
        match self {
            Source::Transfer => "transfer",
            Source::Payment => "payment",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two sources sharing a key would have `p`'s account written over `t`'s,
    /// and every form open from whichever was set last.
    #[test]
    fn every_source_has_its_own_key() {
        let mut keys: Vec<&str> = Source::ALL.iter().map(|s| s.key().name()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two sources share a setting key");
    }

    /// `ALL` is written out by hand, so a source added to the enum without
    /// being added here would drop out of every caller that iterates it.
    #[test]
    fn all_covers_every_variant() {
        // The match is exhaustive, so a third variant stops this compiling
        // until it is added to `ALL` and counted here too.
        for source in Source::ALL {
            match source {
                Source::Transfer | Source::Payment => {}
            }
        }
        assert_eq!(Source::ALL.len(), 2);
    }

    #[test]
    fn every_source_has_its_own_label() {
        let mut labels: Vec<&str> = Source::ALL.iter().map(|s| s.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "two sources share a label");
    }
}
