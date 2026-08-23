pub mod account_label;
pub mod backup;
pub mod calc;
pub mod config;
pub mod db;
pub mod demo;
pub mod description;
pub mod fund;
pub mod gate;
pub mod goal;
pub mod import;
pub mod money;
pub mod overview;
pub mod palette;
pub mod plan;
pub mod plan_line;
pub mod plan_rows;
pub mod projection;
pub mod rate;
pub mod reading;
pub mod recurring_txn;
pub mod report;
pub mod savings;
pub mod savings_block;
pub mod transfer;
pub mod tui;

/// Fixtures the `mod tests` blocks share. Not compiled into the binary, and
/// not reachable from an integration test in `tests/`, which compiles as its
/// own crate -- those have `tests/common/mod.rs`.
#[cfg(test)]
mod test_support;
