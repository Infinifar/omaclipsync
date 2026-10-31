//! SyncClipboard text protocol primitives. No clipboard or network activity is
//! started merely by loading this library.
use regex::RegexSet;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Read,
    time::{Duration, Instant},
};
use url::Url;

pub mod signalr;
pub mod v1_client;
pub mod v1_session;
pub mod v1_state;
pub mod v1_worker;

pub const MAX_TEXT_BYTES: usize = 10 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(rename = "type")]
    pub kind: String,
    pub hash: String,
    pub text: String,
    pub has_data: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer_data_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
}

pub fn hash(text: &str) -> String {
    format!("{:X}", Sha256::digest(text.as_bytes()))
}

/// Build a compatible text profile, preserving UTF-8 boundaries in the preview.
pub fn profile(text: &str, max_inline: usize) -> Result<Profile, &'static str> {
    if text.len() > MAX_TEXT_BYTES {
        return Err("text too large");
    }
    let digest = hash(text);
    let has_data = text.len() > max_inline;
    let mut end = text.len().min(1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(Profile {
        kind: "Text".into(),
        hash: digest.clone(),
        text: if has_data {
            text[..end].into()
        } else {
            text.into()
        },
        has_data,
        data_name: has_data.then(|| format!("Text_{digest}.txt")),
        transfer_data_hash: has_data.then_some(digest),
        // SyncClipboard's TextProfile.Size counts UTF-16 code units.
        size: Some(text.encode_utf16().count()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Off,
    Blacklist,
    Whitelist,
}
pub struct Filter {
    mode: FilterMode,
    patterns: RegexSet,
    downloads: bool,
}
impl Filter {
    pub fn new(
        mode: FilterMode,
        patterns: &[String],
        downloads: bool,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            mode,
            patterns: RegexSet::new(patterns)?,
            downloads,
        })
    }
    pub fn matches(&self, text: &str) -> bool {
        self.patterns.is_match(text)
    }
    pub fn allows(&self, text: &str, download: bool) -> bool {
        if download && !self.downloads {
            return true;
        }
        let matched = self.patterns.is_match(text);
        match self.mode {
            FilterMode::Off => true,
            FilterMode::Blacklist => !matched,
            FilterMode::Whitelist => matched,
        }
    }
}

/// Deduplication remembers only successful clipboard writes.
const MAX_DEDUPE_ENTRIES: usize = 10_000;
#[derive(Debug, PartialEq, Eq)]
pub enum DedupeCheck {
    Allowed,
    Duplicate,
    Full,
}
pub struct Dedupe {
    window: Duration,
    seen: HashMap<String, Instant>,
}
impl Dedupe {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            seen: HashMap::new(),
        }
    }
    pub fn check(&mut self, digest: &str, force: bool, now: Instant) -> DedupeCheck {
        self.seen
            .retain(|_, at| now.saturating_duration_since(*at) < self.window);
        if self.window.is_zero() {
            return DedupeCheck::Allowed;
        }
        let digest = digest.to_ascii_uppercase();
        if self.seen.contains_key(&digest) {
            return if force {
                DedupeCheck::Allowed
            } else {
                DedupeCheck::Duplicate
            };
        }
        if self.seen.len() >= MAX_DEDUPE_ENTRIES {
            return DedupeCheck::Full;
        }
        DedupeCheck::Allowed
    }
    pub fn allows(&mut self, digest: &str, force: bool, now: Instant) -> bool {
        self.check(digest, force, now) == DedupeCheck::Allowed
    }
    pub fn set_window(&mut self, window: Duration) {
        self.window = window;
    }
    pub fn record(&mut self, digest: &str, now: Instant) {
        if !self.window.is_zero() {
            let digest = digest.to_ascii_uppercase();
            if self.seen.len() < MAX_DEDUPE_ENTRIES || self.seen.contains_key(&digest) {
                self.seen.insert(digest, now);
            }
        }
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidUrl,
    Http(reqwest::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Status(u16),
    TooLarge,
    InvalidProfile,
    HashMismatch,
    Filtered,
}
impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub struct Server {
    base: Url,
    user: String,
    password: String,
    client: Client,
}
impl Server {
    /// Basic auth is permitted only over HTTPS; redirects are disabled so the
    /// configured server cannot forward credentials or clipboard contents.
    pub fn new(base: &str, user: String, password: String) -> Result<Self, Error> {
        Self::with_tls(base, user, password, false)
    }
    /// Certificate verification can only be disabled by an explicit user setting.
    pub fn with_tls(
        base: &str,
        user: String,
        password: String,
        insecure: bool,
    ) -> Result<Self, Error> {
        let mut base = Url::parse(base).map_err(|_| Error::InvalidUrl)?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let client = Client::builder()
            .user_agent(concat!("omaclipsync/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            base,
            user,
            password,
            client,
        })
    }
    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base.join(path).map_err(|_| Error::InvalidUrl)
    }
    fn send_retry(request: reqwest::blocking::RequestBuilder) -> Result<Response, Error> {
        for attempt in 0..=2 {
            let response = request.try_clone().ok_or(Error::InvalidProfile)?.send()?;
            if response.status().is_server_error() && attempt < 2 {
                std::thread::sleep(Duration::from_millis(200 * (1 << attempt)));
                continue;
            }
            if !response.status().is_success() {
                return Err(Error::Status(response.status().as_u16()));
            }
            return Ok(response);
        }
        unreachable!()
    }
    fn get(&self, path: &str) -> Result<Response, Error> {
        Self::send_retry(
            self.client
                .get(self.url(path)?)
                .basic_auth(&self.user, Some(&self.password)),
        )
    }
    fn put(&self, path: &str, body: Vec<u8>, content_type: &'static str) -> Result<(), Error> {
        Self::send_retry(
            self.client
                .put(self.url(path)?)
                .basic_auth(&self.user, Some(&self.password))
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body),
        )?;
        Ok(())
    }
    fn read_bounded(response: Response, limit: u64) -> Result<Vec<u8>, Error> {
        if response.content_length().is_some_and(|n| n > limit) {
            return Err(Error::TooLarge);
        }
        let mut buf = Vec::new();
        response.take(limit + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > limit {
            return Err(Error::TooLarge);
        }
        Ok(buf)
    }
    pub fn check(&self) -> Result<(), Error> {
        self.get("")?;
        Ok(())
    }
    pub fn current(&self) -> Result<Profile, Error> {
        Ok(serde_json::from_slice(&Self::read_bounded(
            self.get("SyncClipboard.json")?,
            MAX_METADATA_BYTES,
        )?)?)
    }
    pub fn push(&self, text: &str, max_inline: usize, filter: &Filter) -> Result<bool, Error> {
        if !filter.allows(text, false) {
            return Err(Error::Filtered);
        }
        let p = profile(text, max_inline).map_err(|_| Error::TooLarge)?;
        // Never treat a failed comparison as permission to overwrite a server.
        let current = self.current()?;
        if current.kind == "Text" && current.hash.eq_ignore_ascii_case(&p.hash) {
            return Ok(false);
        }
        if let Some(name) = &p.data_name {
            // The official server reuses an existing history profile and ignores
            // a freshly uploaded file; avoid leaving an orphan in that case.
            let history_path = format!("api/history/Text-{}", p.hash);
            let exists = match self.get(&history_path) {
                Ok(_) => true,
                Err(Error::Status(404)) => false,
                Err(error) => return Err(error),
            };
            if !exists {
                self.put(
                    &format!("file/{name}"),
                    text.as_bytes().to_vec(),
                    "application/octet-stream",
                )?;
            }
        }
        self.put(
            "SyncClipboard.json",
            serde_json::to_vec(&p)?,
            "application/json",
        )?;
        Ok(true)
    }
    pub fn pull(&self, p: &Profile, filter: &Filter) -> Result<String, Error> {
        if p.kind != "Text" || p.hash.len() != 64 || !p.hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::InvalidProfile);
        }
        let text = if p.has_data {
            let path = format!("api/history/Text-{}/data", p.hash);
            let bytes = Self::read_bounded(self.get(&path)?, MAX_TEXT_BYTES as u64)?;
            String::from_utf8(bytes).map_err(|_| Error::InvalidProfile)?
        } else {
            if p.text.len() > MAX_TEXT_BYTES {
                return Err(Error::TooLarge);
            }
            p.text.clone()
        };
        if !hash(&text).eq_ignore_ascii_case(&p.hash)
            || p.transfer_data_hash
                .as_ref()
                .is_some_and(|h| !hash(&text).eq_ignore_ascii_case(h))
        {
            return Err(Error::HashMismatch);
        }
        if !filter.allows(&text, true) {
            return Err(Error::Filtered);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_vectors() {
        assert_eq!(
            hash("abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
    #[test]
    fn dedupe_capacity_fails_closed_until_entries_expire() {
        let now = Instant::now();
        let mut d = Dedupe::new(Duration::from_secs(300));
        for i in 0..MAX_DEDUPE_ENTRIES {
            d.record(&format!("{i:064X}"), now);
        }
        assert_eq!(d.check("new", false, now), DedupeCheck::Full);
        assert_eq!(d.check("new", true, now), DedupeCheck::Full);
        assert_eq!(
            d.check(&format!("{:064X}", 0), true, now),
            DedupeCheck::Allowed
        );
        assert_eq!(
            d.check("new", false, now + Duration::from_secs(300)),
            DedupeCheck::Allowed
        );
    }
    #[test]
    fn unicode_long_preview_is_valid() {
        let text = "😀".repeat(25000);
        let p = profile(&text, 32768).unwrap();
        assert!(p.has_data);
        assert!(p.text.len() <= 1024);
        assert_eq!(p.size, Some(50000));
        assert_eq!(p.transfer_data_hash, Some(hash(&text)));
    }
    #[test]
    fn filter_modes_and_invalid_regex() {
        let rules = vec!["secret".to_owned()];
        let black = Filter::new(FilterMode::Blacklist, &rules, false).unwrap();
        assert!(!black.allows("secret", false));
        assert!(black.allows("secret", true));
        let white = Filter::new(FilterMode::Whitelist, &rules, true).unwrap();
        assert!(white.allows("secret", false));
        assert!(!white.allows("other", true));
        assert!(Filter::new(FilterMode::Off, &["[".into()], false).is_err());
    }
    #[test]
    fn dedupe_force_expiry_and_case() {
        let now = Instant::now();
        let mut d = Dedupe::new(Duration::from_secs(300));
        d.record("abcd", now);
        assert!(!d.allows("ABCD", false, now));
        assert!(d.allows("abcd", true, now));
        assert!(d.allows("abcd", false, now + Duration::from_secs(300)));
    }
    #[test]
    fn long_upload_sends_file_before_metadata_and_checks_existing_hash() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let text = "长😀".repeat(12000);
        let expected_hash = hash(&text);
        let expected_body = text.clone();
        let server_thread = thread::spawn(move || {
            let mut seen = Vec::new();
            for index in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let request = header.lines().next().unwrap().to_owned();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                seen.push((request, body));
                let response = if index == 0 {
                    format!(
                        r#"{{"type":"Text","hash":"{}","text":"","hasData":false}}"#,
                        hash("")
                    )
                } else {
                    String::new()
                };
                let status = if index == 1 {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                )
                .unwrap();
            }
            assert_eq!(seen[0].0, "GET /SyncClipboard.json HTTP/1.1");
            assert_eq!(
                seen[1].0,
                format!("GET /api/history/Text-{expected_hash} HTTP/1.1")
            );
            assert_eq!(
                seen[2].0,
                format!("PUT /file/Text_{expected_hash}.txt HTTP/1.1")
            );
            assert_eq!(seen[2].1, expected_body.as_bytes());
            assert_eq!(seen[3].0, "PUT /SyncClipboard.json HTTP/1.1");
            let metadata: Profile = serde_json::from_slice(&seen[3].1).unwrap();
            assert!(metadata.has_data);
            assert_eq!(metadata.hash, expected_hash);
        });
        // Plain HTTP is injected only in this loopback test. The public constructor requires HTTPS.
        let server = Server {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            user: "test".into(),
            password: "test".into(),
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        let filter = Filter::new(FilterMode::Off, &[], false).unwrap();
        assert!(server.push(&text, 32768, &filter).unwrap());
        server_thread.join().unwrap();
    }
    #[test]
    fn known_long_history_is_reused_without_uploading_an_orphan() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let mut paths = Vec::new();
            for index in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                paths.push(header.lines().next().unwrap().to_owned());
                let len = header
                    .lines()
                    .find_map(|line| {
                        let (n, v) = line.split_once(':')?;
                        n.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; len];
                socket.read_exact(&mut body).unwrap();
                let body = if index == 0 {
                    format!(
                        r#"{{"type":"Text","hash":"{}","text":"","hasData":false}}"#,
                        hash("")
                    )
                } else {
                    "{}".into()
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
            assert!(paths[0].starts_with("GET /SyncClipboard.json "));
            assert!(paths[1].starts_with("GET /api/history/Text-"));
            assert!(paths[2].starts_with("PUT /SyncClipboard.json "));
        });
        let server = Server {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            user: "test".into(),
            password: "test".into(),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        let filter = Filter::new(FilterMode::Off, &[], false).unwrap();
        assert!(server.push(&"x".repeat(40000), 32768, &filter).unwrap());
        worker.join().unwrap();
    }
    #[test]
    fn download_rejects_tampered_content_and_non_text() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            assert!(String::from_utf8(headers)
                .unwrap()
                .starts_with("GET /api/history/Text-"));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\ntampered",
                )
                .unwrap();
        });
        let server = Server {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            user: "test".into(),
            password: "test".into(),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        let filter = Filter::new(FilterMode::Off, &[], false).unwrap();
        let p = profile(&"x".repeat(40000), 32768).unwrap();
        assert!(matches!(server.pull(&p, &filter), Err(Error::HashMismatch)));
        worker.join().unwrap();
        let mut other = p;
        other.kind = "Image".into();
        assert!(matches!(
            server.pull(&other, &filter),
            Err(Error::InvalidProfile)
        ));
    }

    #[test]
    fn retries_server_errors_twice_before_success() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            for i in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    socket.read_exact(&mut b).unwrap();
                    header.push(b[0]);
                }
                let status = if i < 2 {
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let server = Server {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            user: "test".into(),
            password: "test".into(),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        assert!(server.check().is_ok());
        worker.join().unwrap();
    }
    #[test]
    fn retries_replay_the_same_upload_body() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            for attempt in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                assert_eq!(body, b"example-body");
                let status = if attempt < 2 { "500 Error" } else { "200 OK" };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let server = Server {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            user: "test".into(),
            password: "test".into(),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        };
        server
            .put(
                "file/example",
                b"example-body".to_vec(),
                "application/octet-stream",
            )
            .unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn server_paths_preserve_configured_prefix() {
        let server = Server::new("https://example.com/sjtume", "u".into(), "p".into()).unwrap();
        assert_eq!(
            server.url("SyncClipboard.json").unwrap().as_str(),
            "https://example.com/sjtume/SyncClipboard.json"
        );
        assert_eq!(
            server.url("").unwrap().as_str(),
            "https://example.com/sjtume/"
        );
    }
    #[test]
    fn rejects_unsafe_urls() {
        assert!(Server::new("http://example.com", "u".into(), "p".into()).is_err());
        assert!(Server::new("https://user:pass@example.com", "u".into(), "p".into()).is_err());
    }
}

pub mod config;
pub mod engine;
pub mod history;
pub mod keyring;
pub mod logging;
pub mod protocol;
pub mod session;
mod storage;
pub mod wayland;
