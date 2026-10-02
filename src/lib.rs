pub mod account_label;
pub mod allocation;
pub mod calc;
pub mod config;
pub mod db;
pub mod default_source;
pub mod demo;
pub mod description;
pub mod fund;
pub mod fund_label;
pub mod gate;
pub mod goal;
/// The workbook importer. Behind the `import` feature, which is what makes
/// `calamine` an optional dependency: this is the only module that names it.
#[cfg(feature = "import")]
pub mod import;
pub mod mix;
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
pub mod retirement;
pub mod savings;
pub mod savings_block;
pub mod transfer;
pub mod tui;

/// MisterManager's backups: named `money-<timestamp>.db.zst` (zstd-compressed), as the
/// `mistermanager` profile, with the state at `$XDG_STATE_HOME/mistermanager/`.
pub const BACKUP: jluszcz_finance_utils::backup::Spec = jluszcz_finance_utils::backup::Spec {
    app: config::APP,
    stem: "money",
};

/// Fixtures the `mod tests` blocks share. Not compiled into the binary, and
/// not reachable from an integration test in `tests/`, which compiles as its
/// own crate -- those have `tests/common/mod.rs`.
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn backup_key_is_the_money_stem_and_a_utc_timestamp() {
        let now = Utc.with_ymd_and_hms(2026, 8, 20, 14, 3, 5).unwrap();
        assert_eq!(BACKUP.key_for(now), "money-20260820T140305Z.db.zst");
    }

    #[test]
    fn backup_app_name_is_mistermanager() {
        assert_eq!(BACKUP.app, "mistermanager");
    }

    #[test]
    fn backup_profile_defaults_to_the_app_name_when_unset() {
        let config = jluszcz_finance_utils::config::BackupConfig {
            bucket: "a-bucket".into(),
            profile: None,
            interval_days: 7,
        };
        assert_eq!(config.profile_or(BACKUP.app), "mistermanager");
    }
}
