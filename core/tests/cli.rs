use std::{fs, os::unix::fs::PermissionsExt, process::Command};
#[test]
fn pull_force_routes_to_existing_shell_without_shell_interpolation() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("omarchy-shell");
    fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_syncclipctl"))
        .args(["pull", "--force"])
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"io.github.infinifar.omaclipsync\npullForce\n");
}

#[test]
fn unavailable_shell_service_is_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("omarchy-shell");
    fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' unavailable\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_syncclipctl"))
        .arg("push")
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(result.stdout, b"unavailable\n");
}

#[test]
fn config_reads_existing_legacy_data_directory_after_plugin_id_migration() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("omaclipsync");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("config.json"), r#"{"sync":{"intervalSec":23}}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_syncclipctl"))
        .arg("config")
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(config["sync"]["intervalSec"], 23);
}
