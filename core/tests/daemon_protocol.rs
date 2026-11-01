use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn daemon_handshake_status_toggle_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_syncclipd"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let input = child.stdin.as_mut().unwrap();
        for command in [
            r#"{"id":1,"method":"hello"}"#,
            r#"{"id":2,"method":"status"}"#,
            r#"{"id":3,"method":"toggle"}"#,
            r#"{"id":4,"method":"get_config"}"#,
            r#"{"id":5,"method":"test_filter","params":{"filter":{"mode":"blacklist","patterns":["secret"],"alsoBlockDownloads":false},"text":"secret-private-test"}}"#,
            r#"{"id":6,"method":"shutdown"}"#,
        ] {
            writeln!(input, "{command}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let messages: Vec<serde_json::Value> = output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::from_slice(v).unwrap())
        .collect();
    let reply = |id| messages.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(reply(1)["result"]["protocol"], 1);
    assert_eq!(reply(2)["result"]["state"], "needs_config");
    assert_eq!(reply(3)["ok"], true);
    assert_eq!(reply(4)["result"]["sync"]["enabled"], false);
    assert_eq!(reply(5)["result"]["matched"], true);
    assert_eq!(reply(5)["result"]["allowed"], false);
    assert_eq!(reply(6)["ok"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-private-test"));
    use std::os::unix::fs::PermissionsExt;
    let config = root.path().join("config/omaclipsync/config.json");
    assert_eq!(
        std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
