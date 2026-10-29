//! Thin client for the already-running Omarchy shell; never starts a daemon.
use omaclipsync_core::config::Config;
use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

fn usage() {
    eprintln!("Usage: syncclipctl status | push | pull [--force] | toggle | watch | config");
}
fn ipc(method: &str) -> bool {
    match Command::new("omarchy-shell")
        .args(["omaclipsync", method])
        .output()
    {
        Ok(output) => {
            let _ = io::stdout().write_all(&output.stdout);
            let _ = io::stderr().write_all(&output.stderr);
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() != "unavailable"
        }
        Err(_) => {
            eprintln!("Unable to reach the Omarchy shell.");
            false
        }
    }
}
fn config_path() -> Option<PathBuf> {
    let root = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) if PathBuf::from(&value).is_absolute() => PathBuf::from(value),
        _ => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };
    Some(root.join("omaclipsync/config.json"))
}
fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let success = match args.as_slice() {
        [cmd] if cmd == "status" => ipc("status"),
        [cmd] if cmd == "push" => ipc("push"),
        [cmd] if cmd == "pull" => ipc("pull"),
        [cmd, flag] if cmd == "pull" && flag == "--force" => ipc("pullForce"),
        [cmd] if cmd == "toggle" => ipc("toggle"),
        [cmd] if cmd == "watch" => loop {
            if !ipc("status") {
                break false;
            }
            thread::sleep(Duration::from_secs(1));
        },
        [cmd] if cmd == "config" => {
            match config_path()
                .and_then(|p| Config::read(&p).ok())
                .and_then(|c| serde_json::to_string_pretty(&c).ok())
            {
                Some(text) => {
                    println!("{text}");
                    true
                }
                None => {
                    eprintln!("Configuration is missing or invalid.");
                    false
                }
            }
        }
        [flag] if flag == "--help" || flag == "-h" => {
            usage();
            true
        }
        _ => {
            usage();
            false
        }
    };
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
