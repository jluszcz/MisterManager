//! Backing the database up to S3.

pub mod s3;
pub mod state;

use crate::config::Config;
use crate::db;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use std::path::Path;

/// Whether a backup is owed, against the real clock rather than `--today`.
/// `--today` simulates a financial date; whether a file reached S3 is a fact
/// about wall time.
pub fn is_due(last: Option<DateTime<Utc>>, now: DateTime<Utc>, interval_days: u32) -> bool {
    match last {
        None => true,
        Some(last) => now.signed_duration_since(last) >= interval(interval_days),
    }
}

pub fn next_due(last: DateTime<Utc>, interval_days: u32) -> DateTime<Utc> {
    last + interval(interval_days)
}

/// Ten years. `interval_days` is read straight out of a hand-edited config
/// file, and `DateTime`'s addition panics rather than erroring once the sum
/// leaves chrono's calendar. Past a decade the setting is a typo rather than
/// a schedule, and a clamp is what keeps one from taking the program down.
const MAX_INTERVAL_DAYS: u32 = 3653;

fn interval(days: u32) -> TimeDelta {
    TimeDelta::days(i64::from(days.min(MAX_INTERVAL_DAYS)))
}

// The snapshot is a full, unencrypted copy of the owner's database sitting in a
// directory every local account can traverse -- `std::env::temp_dir()` is `/tmp` on
// Linux, and `create_dir_all`'s default `0777 & ~umask` leaves it world-readable for
// as long as the upload takes, or indefinitely if the process dies mid-upload.
//
// The leaf is created *non*-recursively, and that is the whole guard rather than a
// detail. A recursive create returns `Ok` for a path that already exists without
// applying the mode at all, and its existence check follows symlinks -- so the
// predictable `/tmp/mistermanager-backup-<pid>`, pre-created world-readable or
// pointed somewhere else entirely, would have taken the snapshot with it. Creating
// the leaf ourselves means the only way past this function is a directory this
// process just made with mode 0700.
#[cfg(unix)]
fn create_snapshot_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Our own leftover, from a run killed between creating this and removing it.
    // Anything that survives the removal is someone else's and must not be written
    // into, so the `create` below is left to fail on it rather than being coaxed
    // past it.
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_snapshot_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// A backup is an object at the root of the bucket, under no prefix. The
/// bucket is this application's own and holds nothing else, so a prefix would
/// name the only thing in there -- and every rule written about the bucket,
/// the lifecycle configuration and the IAM policy alike, then covers the
/// backups by covering everything.
pub fn key_for(now: DateTime<Utc>) -> String {
    format!("money-{}.db", now.format("%Y%m%dT%H%M%SZ"))
}

/// Integer arithmetic on purpose: the crate has no floats, and a backup size
/// to one decimal place is not worth being the first.
pub fn human_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// No `[backup]` section: the feature is off.
    Disabled,
    NotDue {
        next: DateTime<Utc>,
    },
    BackedUp {
        key: String,
        bytes: u64,
    },
}

/// Snapshot, upload, and record -- when the schedule says so, or when `force`
/// overrides it.
pub fn run_if_due(
    db_path: &Path,
    cfg: &Config,
    state_path: &Path,
    now: DateTime<Utc>,
    force: bool,
) -> Result<Outcome> {
    let Some(backup) = cfg.backup.as_ref() else {
        return Ok(Outcome::Disabled);
    };

    let last = match state::read(state_path) {
        Ok(state) => state.map(|s| s.last_backup_at),
        // Warns rather than refusing, unlike a dangling `setting` key. The
        // difference is what it costs to be wrong: one redundant upload,
        // after which the file is rewritten and correct again.
        Err(e) => {
            eprintln!("ignoring unreadable backup state: {e:#}");
            None
        }
    };

    if !force
        && let Some(last) = last
        && !is_due(Some(last), now, backup.interval_days)
    {
        return Ok(Outcome::NotDue {
            next: next_due(last, backup.interval_days),
        });
    }

    // Its own directory, named for the process, so two `mm` runs cannot pick
    // the same snapshot path.
    let dir = std::env::temp_dir().join(format!("mistermanager-backup-{}", std::process::id()));
    create_snapshot_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let snapshot = dir.join("money.db");
    // `VACUUM INTO` refuses a destination that already exists, which a
    // previous run killed mid-upload would have left behind.
    let _ = std::fs::remove_file(&snapshot);

    let key = key_for(now);
    let result = (|| {
        db::snapshot(db_path, &snapshot)?;
        let bytes = std::fs::metadata(&snapshot)
            .with_context(|| format!("measuring {}", snapshot.display()))?
            .len();
        s3::upload(&backup.profile, &backup.bucket, &key, &snapshot)?;
        Ok::<u64, anyhow::Error>(bytes)
    })();

    // On both paths: a copy of the whole database must not be left in the
    // temp directory because an upload failed.
    let _ = std::fs::remove_dir_all(&dir);
    let bytes = result?;

    state::write(
        state_path,
        &state::State {
            last_backup_at: now,
            last_key: key.clone(),
        },
    )?;

    Ok(Outcome::BackedUp { key, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn a_database_that_has_never_been_backed_up_is_due() {
        assert!(is_due(None, at(20, 0), 7));
    }

    #[test]
    fn a_backup_one_day_short_of_the_interval_is_not_due() {
        assert!(!is_due(Some(at(14, 0)), at(20, 0), 7));
    }

    #[test]
    fn a_backup_exactly_the_interval_old_is_due() {
        assert!(is_due(Some(at(13, 0)), at(20, 0), 7));
    }

    #[test]
    fn a_backup_well_past_the_interval_is_due() {
        assert!(is_due(Some(at(1, 0)), at(20, 0), 7));
    }

    /// A clock that went backwards leaves `last` in the future. Not due, and
    /// it heals itself once the clock passes the recorded time -- which beats
    /// uploading on every run until someone notices.
    #[test]
    fn a_last_backup_in_the_future_is_not_due() {
        assert!(!is_due(Some(at(25, 0)), at(20, 0), 7));
    }

    /// Legitimate while setting the feature up, so it is allowed rather than
    /// rejected.
    #[test]
    fn an_interval_of_zero_days_is_always_due() {
        assert!(is_due(Some(at(20, 0)), at(20, 0), 0));
    }

    #[test]
    fn the_next_backup_is_due_one_interval_after_the_last() {
        assert_eq!(next_due(at(13, 0), 7), at(20, 0));
    }

    /// A typo in the config file must not take the program down. `u32::MAX`
    /// days overflows chrono's calendar when added to any realistic `last`.
    #[test]
    fn an_absurd_interval_is_clamped_rather_than_panicking() {
        assert_eq!(
            next_due(at(20, 0), u32::MAX),
            at(20, 0) + TimeDelta::days(3653)
        );
    }

    /// At the root of the bucket, and sortable, so `aws s3 ls` returns the
    /// history in order and the newest object is the last line.
    #[test]
    fn a_key_is_a_sortable_utc_timestamp_under_no_prefix() {
        let now = Utc.with_ymd_and_hms(2026, 8, 20, 14, 3, 5).unwrap();
        assert_eq!(key_for(now), "money-20260820T140305Z.db");
    }

    #[test]
    fn a_byte_count_reads_in_kibibytes_below_a_mebibyte() {
        assert_eq!(human_bytes(2048), "2 KiB");
    }

    #[test]
    fn a_byte_count_reads_in_mebibytes_at_and_above_one() {
        assert_eq!(human_bytes(4 * 1024 * 1024), "4 MiB");
    }

    /// A partial kibibyte rounds up rather than down. This test fails if
    /// `div_ceil` is replaced with a truncating divide.
    #[test]
    fn a_partial_kibibyte_rounds_up() {
        assert_eq!(human_bytes(1025), "2 KiB");
    }

    /// The snapshot is the whole database in cleartext, and on Linux it lands
    /// under a `/tmp` every local account can traverse.
    #[cfg(unix)]
    #[test]
    fn a_snapshot_directory_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "mistermanager_snapshot_mode_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        create_snapshot_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The path is `/tmp/mistermanager-backup-<pid>` and nothing about it is
    /// secret, so a local attacker can sit on it before `mm` ever runs. A
    /// recursive create would accept the squatted directory and never apply the
    /// mode, handing over the snapshot; this is the test that fails if anyone
    /// reaches for `create_dir_all` again.
    #[cfg(unix)]
    #[test]
    fn a_snapshot_directory_that_already_exists_does_not_keep_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "mistermanager_snapshot_squatted_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        // A file inside it, so the directory cannot be replaced by a bare `rmdir`.
        std::fs::write(dir.join("planted"), b"planted").unwrap();

        create_snapshot_dir(&dir).unwrap();

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);
        assert!(!dir.join("planted").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_config_with_no_backup_section_does_nothing() {
        let cfg = crate::config::Config::default();
        let outcome = run_if_due(
            std::path::Path::new("/nonexistent/money.db"),
            &cfg,
            std::path::Path::new("/nonexistent/backup.toml"),
            at(20, 0),
            false,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Disabled);
    }

    /// The database path is deliberately nonexistent: a run that is not due
    /// must return before it touches the database at all.
    #[test]
    fn a_recent_backup_returns_when_the_next_one_is_due_without_reading_the_database() {
        let state_path = std::env::temp_dir().join(format!(
            "mistermanager_run_notdue_{}.toml",
            std::process::id()
        ));
        state::write(
            &state_path,
            &state::State {
                last_backup_at: at(19, 0),
                last_key: "money-20260819T000000Z.db".to_string(),
            },
        )
        .unwrap();

        let cfg = crate::config::Config {
            backup: Some(crate::config::Backup {
                bucket: "a-bucket".to_string(),
                profile: "a-profile".to_string(),
                interval_days: 7,
            }),
            report: None,
        };

        let outcome = run_if_due(
            std::path::Path::new("/nonexistent/money.db"),
            &cfg,
            &state_path,
            at(20, 0),
            false,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::NotDue { next: at(26, 0) });

        let _ = std::fs::remove_file(&state_path);
    }
}
