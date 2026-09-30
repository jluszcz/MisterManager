//! The configuration file, and the primary home of `serde` and `toml` -- named again in
//! `src/backup/state.rs`, deliberate leakage rather than an oversight.
//!
//! An absent file, or one missing a section, means that section's feature is
//! off -- the same rule an unset `setting` key follows, and what makes a clean
//! checkout and an unconfigured machine both do nothing. A file that is
//! present but does not parse is an error instead. `[report]`'s `dir` and
//! `[backup]`'s `bucket` have no default, so the typo that would otherwise
//! switch a feature off silently is a missing field. Keys nothing reads are
//! ignored, so a file written for another build still configures every key
//! this one does understand.

use anyhow::Result;
use jluszcz_finance_utils::config::{self as shared, BackupConfig, ReportConfig};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The application's name: its config and state directories, and the AWS
/// profile a `[backup]` section that names none authenticates as.
pub const APP: &str = "mistermanager";

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct Config {
    pub backup: Option<BackupConfig>,
    pub report: Option<ReportConfig>,
    pub sec: Option<Sec>,
}

/// What `mm mixes` puts in its `User-Agent`.
///
/// SEC refuses a request that declares no contact, so this is required rather
/// than defaulted -- and it is a setting rather than a constant because no
/// real address may be written into a file in this repository.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Sec {
    pub contact: String,
}

/// The instruction every refusal over a missing `[sec]` section shares --
/// `mm mixes` and `g`/`G` on the Funds screen each build their own sentence
/// around it, `mm mixes` naming the config file it looked in and the Funds
/// screen unable to, since `tui` does not otherwise name this module and
/// `sec_contact` reaches it as a bare `Option<String>`. What both already
/// say the same way lives here, the same reason `goal::NO_TAX_RATE` is one
/// constant rather than two hand-written sentences: the TOML key this names
/// has one spelling, and the sentence naming it should too. The Funds help
/// panel is the third reader and deliberately spells no key at all -- a
/// `detail` is a `&'static str` in a `const` table, so it could only carry
/// this by restating it, and what an owner pressing `?` needs is that the key
/// has a prerequisite rather than the TOML to satisfy it with. The refusal
/// itself is one keystroke away and says the rest.
pub const ADD_SEC_CONTACT: &str = "add a [sec] section with a contact line";

/// `$XDG_CONFIG_HOME/mistermanager/config.toml`, or `~/.config` when it is
/// unset or empty.
pub fn default_path() -> Result<PathBuf> {
    shared::default_path(APP)
}

pub fn load(path: &Path) -> Result<Config> {
    shared::load(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `body` to a uniquely named temp file and returns the path. The
    /// name carries the test's own label because several of these run in one
    /// process at once and a shared name would have them reading each other's
    /// fixtures.
    fn fixture(label: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mistermanager_config_{label}_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A section a later build might add, or an earlier one has dropped.
    /// Deliberately not `[report]`, which this build reads: the example has
    /// to name a section nothing here has a field for.
    #[test]
    fn an_unknown_section_does_not_stop_the_rest_of_the_file_loading() {
        let path = fixture(
            "section",
            "[charts]\nstyle = \"wide\"\n\n[backup]\nbucket = \"a-bucket\"\n",
        );
        assert_eq!(load(&path).unwrap().backup.unwrap().bucket, "a-bucket");
    }

    #[test]
    fn a_config_with_no_sec_section_parses_and_reports_no_contact() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.sec.is_none());
    }

    #[test]
    fn the_sec_contact_is_read_from_its_own_section() {
        let config: Config = toml::from_str("[sec]\ncontact = \"someone@example.com\"").unwrap();
        assert_eq!(config.sec.unwrap().contact, "someone@example.com");
    }
}
