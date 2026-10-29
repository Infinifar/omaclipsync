//! Explicit, potentially visible live test. Restores the original server profile
//! only if the current hash still matches our synthetic text. Concurrent writes
//! can race the check because the SyncClipboard server has no conditional PUT.
use omaclipsync_core::{hash, keyring, Filter, FilterMode, Profile, Server};
use reqwest::blocking::Client;
use std::{
    env,
    process::ExitCode,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 || args[3] != "--write-test-data" {
        eprintln!("usage: real_roundtrip HTTPS_URL USER --write-test-data");
        return ExitCode::FAILURE;
    }
    match run(&args[1], &args[2]) {
        Ok(()) => {
            println!("live long-text round trip and restoration: PASS");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
fn run(url: &str, user: &str) -> Result<(), &'static str> {
    let password = keyring::lookup(user).map_err(|_| "keyring lookup failed")?;
    let server = Server::new(url, user.to_owned(), password.clone())
        .map_err(|_| "invalid server configuration")?;
    let mut base = Url::parse(url).map_err(|_| "invalid URL")?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let client = Client::builder()
        .user_agent("omaclipsync-live-test/0.1")
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "client failed")?;
    let original = server
        .current()
        .map_err(|_| "cannot read current profile")?;
    if original.kind != "Text"
        || original.hash.len() != 64
        || !original.hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("current server profile is not a restorable text profile; no changes made");
    }
    let off = Filter::new(FilterMode::Off, &[], false).map_err(|_| "filter failed")?;
    // Verify the full original content before touching the current profile.
    server
        .pull(&original, &off)
        .map_err(|_| "original text cannot be verified; no changes made")?;
    let history = base
        .join(&format!("api/history/Text-{}", original.hash))
        .map_err(|_| "invalid history URL")?;
    let exists = client
        .get(history)
        .basic_auth(user, Some(&password))
        .send()
        .map_err(|_| "history check failed; no changes made")?;
    if !exists.status().is_success() {
        return Err("original profile is not present in server history; no changes made");
    }
    drop(exists);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let fixture = format!(
        "omaclipsync-live-fixture-{nonce}-{}-{}",
        std::process::id(),
        "x".repeat(100_000)
    );
    let fixture_hash = hash(&fixture);
    let before = server
        .current()
        .map_err(|_| "cannot recheck current profile; no changes made")?;
    if before.kind != original.kind || !before.hash.eq_ignore_ascii_case(&original.hash) {
        return Err("another server update was observed before the test; no changes made");
    }
    let sent = server.push(&fixture, 32768, &off);
    let verified = sent.as_ref().is_ok_and(|changed| *changed)
        && server
            .current()
            .ok()
            .filter(|p| p.hash.eq_ignore_ascii_case(&fixture_hash))
            .and_then(|p| server.pull(&p, &off).ok())
            .is_some_and(|value| value == fixture);
    // Always inspect after the attempted PUT, including a timeout or error: the
    // server may have committed before the response was lost.
    let mut current = None;
    for _ in 0..3 {
        if let Ok(profile) = server.current() {
            current = Some(profile);
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    let current = current.ok_or("cannot inspect server after test; restoration is unconfirmed")?;
    if current.hash.eq_ignore_ascii_case(&fixture_hash) {
        restore(&client, &base, user, &password, &original)?;
        let restored = server
            .current()
            .map_err(|_| "restore sent, but result cannot be verified")?;
        if !restored.hash.eq_ignore_ascii_case(&original.hash) {
            return Err("restore was not confirmed");
        }
    } else if !current.hash.eq_ignore_ascii_case(&original.hash) {
        return Err("another server update was observed; restoration was skipped");
    }
    if verified {
        Ok(())
    } else {
        Err("live round trip failed; original profile was retained or restored")
    }
}
fn restore(
    client: &Client,
    base: &Url,
    user: &str,
    password: &str,
    original: &Profile,
) -> Result<(), &'static str> {
    let url = base
        .join("SyncClipboard.json")
        .map_err(|_| "invalid metadata URL")?;
    let response = client
        .put(url)
        .basic_auth(user, Some(password))
        .json(original)
        .send()
        .map_err(|_| "restoration request failed")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err("restoration was rejected")
    }
}
