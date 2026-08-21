//! The two blocks the `Savings` sheet carries, and which account each is.
//!
//! The sheet identifies its blocks by *position* alone -- `A:E` and `I:K` --
//! and carries no account code beside either, so the mapping cannot be read
//! out of the workbook the way every other account reference can. It is
//! configured once on the Accounts screen instead, and the import refuses to
//! read `Savings` until it has been.
//!
//! One value owns both halves for `gate::Gate`'s reason: a block recorded
//! under the other block's key sends a whole sheet's goals to the wrong
//! container, and every balance, gate and destination below follows it there
//! with nothing in the output to say so.

use crate::db::AccountId;
use crate::db::setting::Key;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Block {
    /// `Savings!A:E` — named goals with targets, dates, and a per-paycheck
    /// ask. Scanned the whole sheet's height, because the block has gaps.
    Goals,
    /// `Savings!I:K` — undated buckets. Contiguous, so the scan stops at the
    /// first blank name.
    Buckets,
}

impl Block {
    /// Every block, for callers that must cover both.
    pub const ALL: [Block; 2] = [Block::Goals, Block::Buckets];

    /// The `setting` key holding this block's container account id.
    pub fn key(self) -> Key<AccountId> {
        match self {
            Block::Goals => Key::new("savings.goals_container_account_id"),
            Block::Buckets => Key::new("savings.buckets_container_account_id"),
        }
    }

    /// What the Accounts screen calls this block. The contents, not the
    /// columns: the sheet's own `A:E`/`I:K` is what an *import* tells the two
    /// blocks apart by, and the owner picking a container is choosing between
    /// goals and buckets rather than between two spans of a spreadsheet.
    pub fn label(self) -> &'static str {
        match self {
            Block::Goals => "goals",
            Block::Buckets => "buckets",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two blocks sharing a key would write one container's id over the
    /// other's, sending both blocks' rows into one account.
    #[test]
    fn every_block_has_its_own_key() {
        let mut keys: Vec<&str> = Block::ALL.iter().map(|b| b.key().name()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two blocks share a setting key");
    }

    /// `ALL` is written out by hand, so a block added to the enum without
    /// being added here would drop out of every caller that iterates it.
    #[test]
    fn all_covers_every_variant() {
        // The match is exhaustive, so a third variant stops this compiling
        // until it is added to `ALL` and counted here too.
        for block in Block::ALL {
            match block {
                Block::Goals | Block::Buckets => {}
            }
        }
        assert_eq!(Block::ALL.len(), 2);
    }

    #[test]
    fn every_block_has_its_own_label() {
        let mut labels: Vec<&str> = Block::ALL.iter().map(|b| b.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "two blocks share a label");
    }
}
