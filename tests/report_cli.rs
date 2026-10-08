//! `mm report` end to end, against scratch paths.

use std::path::PathBuf;
use std::process::Command;

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mistermanager_report_cli_{label}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `--dir` says where the page goes, which is all the config would have
/// supplied, so a config that does not parse does not stop the run.
#[test]
fn a_report_given_its_directory_is_written_despite_a_broken_config() {
    let dir = scratch("broken_config");
    let config = dir.join("config.toml");
    std::fs::write(&config, "[report\n").unwrap();
    let out = dir.join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_mm"))
        .args(["--db", dir.join("money.db").to_str().unwrap()])
        .args(["--config", config.to_str().unwrap()])
        .args(["report", "--dir", out.to_str().unwrap()])
        .env("XDG_STATE_HOME", dir.join("state"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.join("Money.html").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
