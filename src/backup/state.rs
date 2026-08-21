//! When the last backup succeeded, kept beside the config file rather than
//! inside it: one is edited by a person and would be restored from a dotfile
//! backup, the other is written by the program and means nothing on another
//! machine.
//!
//! Written only after a successful upload, so a failed run leaves the
//! schedule due rather than recording an attempt that moved no bytes.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// `toml` is named here as well as in `src/config.rs`. That is one module's worth of leakage and it is
// deliberate: the alternative is a serialization helper in `config` that knows about backup state,
// which couples the two files the design just separated. If a third module ever needs `toml`, move all
// three behind `config`.

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct State {
    pub last_backup_at: DateTime<Utc>,
    /// Not read by anything; it is what `mm backup --status` prints, and what
    /// makes the file legible to a human wondering what the last upload was.
    pub last_key: String,
}

/// `$XDG_STATE_HOME/mistermanager/backup.toml`, or `~/.local/state` when it
/// is unset or empty.
pub fn default_path() -> Result<PathBuf> {
    let dir = match std::env::var("XDG_STATE_HOME") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".local").join("state")
        }
    };
    Ok(dir.join("mistermanager").join("backup.toml"))
}

/// `Ok(None)` means no backup has ever been recorded. An unreadable file is
/// an `Err` rather than a second flavour of `None`: the caller is the one
/// that decides an unreadable state file is worth a warning and a redundant
/// upload, and it cannot decide that if this function has already shrugged.
pub fn read(path: &Path) -> Result<Option<State>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let state = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(state))
}

pub fn write(path: &Path, state: &State) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string(state).context("serializing backup state")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn temp_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mistermanager_state_{label}_{}.toml",
            std::process::id()
        ))
    }

    fn a_state() -> State {
        State {
            last_backup_at: Utc.with_ymd_and_hms(2026, 8, 20, 14, 3, 5).unwrap(),
            last_key: "a-prefix/money-20260820T140305Z.db".to_string(),
        }
    }

    #[test]
    fn a_written_state_reads_back_identical() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        write(&path, &a_state()).unwrap();
        assert_eq!(read(&path).unwrap(), Some(a_state()));
    }

    /// Creating the directory is the point: nothing else in the application
    /// ever writes to `~/.local/state`, so the first backup on a machine finds
    /// it missing.
    #[test]
    fn writing_creates_the_directory_it_needs() {
        let dir =
            std::env::temp_dir().join(format!("mistermanager_state_mkdir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("backup.toml");
        write(&path, &a_state()).unwrap();
        assert_eq!(read(&path).unwrap(), Some(a_state()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_state_file_reads_as_never_backed_up() {
        let path = temp_path("absent_never_written");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read(&path).unwrap(), None);
    }

    /// Distinct from the case above, and the caller is what tells them apart:
    /// `run_if_due` warns and carries on, where a dangling `setting` key would
    /// refuse. The difference is the consequence -- one redundant upload
    /// rather than money moved to the wrong place.
    #[test]
    fn a_state_file_that_does_not_parse_is_an_error() {
        let path = temp_path("garbage");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        assert!(read(&path).is_err());
    }
}
