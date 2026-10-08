//! `mm backup` end to end, against scratch paths and an AWS environment that
//! can reach nothing.

use std::path::PathBuf;
use std::process::Command;

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mistermanager_backup_cli_{label}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A mistyped `--db` must not become a freshly created, seeded database that
/// is uploaded as a restore point and stamped as the latest backup.
#[test]
fn backing_up_a_database_path_that_does_not_exist_creates_nothing() {
    let dir = scratch("missing_db");
    let config = dir.join("config.toml");
    std::fs::write(&config, "[backup]\nbucket = \"a-bucket\"\n").unwrap();
    let db = dir.join("typo.db");

    let output = Command::new(env!("CARGO_BIN_EXE_mm"))
        .args([
            "--db",
            db.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ])
        .args(["backup", "--force"])
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("AWS_CONFIG_FILE", "/dev/null")
        .env("AWS_SHARED_CREDENTIALS_FILE", "/dev/null")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!db.exists(), "mm backup created {}", db.display());
    assert!(!dir.join("state").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_help_names_the_default_database_and_config_paths() {
    let output = Command::new(env!("CARGO_BIN_EXE_mm"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("~/.local/share/mistermanager/money.db"),
        "{help}"
    );
    assert!(
        help.contains("~/.config/mistermanager/config.toml"),
        "{help}"
    );
}

/// With `--db` given, the default data directory is never computed, so it is
/// never created.
#[test]
fn a_db_flag_does_not_create_the_default_data_directory() {
    let dir = scratch("db_flag_home");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let db = dir.join("given.db");
    let output = Command::new(env!("CARGO_BIN_EXE_mm"))
        .args(["--db", db.to_str().unwrap(), "backup", "--status"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.join(".local/share/mistermanager").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
