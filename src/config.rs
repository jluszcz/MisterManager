//! The configuration file, and the primary home of `serde` and `toml` -- named again in
//! `src/backup/state.rs`, deliberate leakage rather than an oversight.
//!
//! Two failure modes that look alike are deliberately opposite. A file that
//! is absent, or that carries no `[backup]` section, means the feature is
//! off -- the same rule an unset `setting` key follows, and what makes a
//! clean checkout and an unconfigured machine both do nothing. A file that
//! is present but does not parse is an error instead, because the
//! alternative is that a misspelled key leaves `bucket` unset and reads as
//! "off": a backup that quietly stops running is the one failure nothing
//! downstream ever notices.
//!
//! A key nothing reads is neither -- it is ignored, so a file written for
//! another build still configures every key this one does understand. What
//! keeps the paragraph above true is that `bucket` has **no default**: the
//! typo that would switch backups off silently is a missing required field,
//! and still an error. Refusing the whole file would only ever have caught
//! keys that were additionally wrong.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct Config {
    pub backup: Option<Backup>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Backup {
    /// Where backups are written. No default: it names where the owner's
    /// finances are kept, so it cannot be a literal in a public repository.
    pub bucket: String,
    /// The `~/.aws/credentials` profile to authenticate as.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Zero means every run uploads, which is a legitimate thing to ask for
    /// while setting this up.
    #[serde(default = "default_interval_days")]
    pub interval_days: u32,
}

fn default_profile() -> String {
    "mistermanager".to_string()
}

fn default_interval_days() -> u32 {
    7
}

/// `$XDG_CONFIG_HOME/mistermanager/config.toml`, or `~/.config` when it is
/// unset or empty.
pub fn default_path() -> Result<PathBuf> {
    let dir = match std::env::var("XDG_CONFIG_HOME") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(dir.join("mistermanager").join("config.toml"))
}

pub fn load(path: &Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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

    #[test]
    fn a_fully_specified_backup_section_parses() {
        let path = fixture(
            "full",
            r#"
            [backup]
            bucket = "a-bucket"
            profile = "a-profile"
            interval_days = 3
            "#,
        );
        let cfg = load(&path).unwrap();
        let backup = cfg.backup.unwrap();
        assert_eq!(backup.bucket, "a-bucket");
        assert_eq!(backup.profile, "a-profile");
        assert_eq!(backup.interval_days, 3);
    }

    /// Only the bucket has no sensible default, so a one-line section is a
    /// complete configuration.
    #[test]
    fn a_backup_section_naming_only_a_bucket_takes_every_default() {
        let path = fixture("minimal", "[backup]\nbucket = \"a-bucket\"\n");
        let backup = load(&path).unwrap().backup.unwrap();
        assert_eq!(backup.profile, "mistermanager");
        assert_eq!(backup.interval_days, 7);
    }

    /// The prefix is fixed in `backup::PREFIX` because it has to match an IAM
    /// policy only an AWS apply can change. A file asking for another one is
    /// a line that does nothing, and the rest of the file still loads.
    #[test]
    fn a_prefix_key_is_ignored_rather_than_refusing_the_file() {
        let path = fixture(
            "prefix",
            "[backup]\nbucket = \"a-bucket\"\nprefix = \"a-prefix\"\n",
        );
        assert_eq!(load(&path).unwrap().backup.unwrap().bucket, "a-bucket");
    }

    /// A section a later build might add, or an earlier one has dropped.
    #[test]
    fn an_unknown_section_does_not_stop_the_rest_of_the_file_loading() {
        let path = fixture(
            "section",
            "[report]\nstyle = \"wide\"\n\n[backup]\nbucket = \"a-bucket\"\n",
        );
        assert_eq!(load(&path).unwrap().backup.unwrap().bucket, "a-bucket");
    }

    /// An unset feature is an off feature -- the rule the `setting` keys
    /// already follow. A machine the owner has not configured does nothing.
    #[test]
    fn a_config_file_with_no_backup_section_leaves_backups_off() {
        let path = fixture("empty", "");
        assert_eq!(load(&path).unwrap(), Config::default());
    }

    #[test]
    fn a_missing_config_file_leaves_backups_off() {
        let path = std::env::temp_dir().join(format!(
            "mistermanager_config_absent_never_written_{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        assert_eq!(load(&path).unwrap(), Config::default());
    }

    /// The one failure mode "unset means off" cannot absorb, and the reason
    /// `bucket` has no default: a typo in *it* must not leave the section
    /// parsing and the feature silently switched off, because a backup that
    /// stops running is a backup nothing downstream notices. An unknown key
    /// is ignored; a required one missing is still an error.
    #[test]
    fn a_misspelled_bucket_is_an_error_rather_than_a_silently_disabled_backup() {
        let path = fixture("typo", "[backup]\nbucketname = \"a-bucket\"\n");
        // `{:#}` rather than `to_string()`: anyhow's plain Display prints
        // only the outermost context, which is "parsing <path>". The field
        // name is in the `toml` error it wraps.
        let err = format!("{:#}", load(&path).unwrap_err());
        assert!(err.contains("bucket"), "unhelpful error: {err}");
    }

    #[test]
    fn a_wrongly_typed_value_is_an_error_naming_the_file() {
        let path = fixture("type", "[backup]\nbucket = 7\n");
        let err = format!("{:#}", load(&path).unwrap_err());
        assert!(
            err.contains("mistermanager_config_type"),
            "unhelpful error: {err}"
        );
    }
}
