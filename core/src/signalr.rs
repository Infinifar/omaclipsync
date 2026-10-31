//! A bounded, cancellable SignalR JSON-over-WebSocket notification worker.
//!
//! Only the configured HTTPS origin receives credentials. Notifications are
//! hints: clipboard data is fetched and validated by the existing HTTP path.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::blocking::Client;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde_json::Value;
use std::{
    io::{self, Read},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{SyncSender, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{
    client::IntoClientRequest,
    client_tls_with_config,
    http::header::{AUTHORIZATION, USER_AGENT},
    protocol::{Message, WebSocketConfig},
    stream::MaybeTlsStream,
    Connector, WebSocket,
};
use url::Url;

const LIMIT: usize = 256 * 1024;
const RETRY_CAP: u64 = 60;
const HEARTBEAT: Duration = Duration::from_secs(15);
const SERVER_TIMEOUT: Duration = Duration::from_secs(45);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT_VALUE: &str = concat!("omaclipsync/", env!("CARGO_PKG_VERSION"));

// DNS resolution can block indefinitely inside system libraries. A cancelled
// generation must never make unbounded replacement network threads possible.
// If both slots are stuck, the newest generation waits without blocking the
// daemon, which continues its independent HTTP fallback polling.
static NETWORK_SLOTS: Slots = Slots::new(2);

struct Slots {
    in_use: AtomicUsize,
    limit: usize,
}
impl Slots {
    const fn new(limit: usize) -> Self {
        Self {
            in_use: AtomicUsize::new(0),
            limit,
        }
    }

    fn acquire(&self, cancel: &AtomicBool) -> Option<Permit<'_>> {
        while !stopped(cancel) {
            let used = self.in_use.load(Ordering::Acquire);
            if used < self.limit
                && self
                    .in_use
                    .compare_exchange_weak(used, used + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let permit = Permit(self);
                // Cancellation may have raced with acquisition.
                return (!stopped(cancel)).then_some(permit);
            }
            wait(cancel, Duration::from_millis(100));
        }
        None
    }
}
struct Permit<'a>(&'a Slots);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.in_use.fetch_sub(1, Ordering::AcqRel);
    }
}

pub use rustclipsync_core::worker::Event;

pub struct Worker {
    stop: Arc<AtomicBool>,
}
impl Worker {
    pub fn start(
        url: String,
        user: String,
        password: String,
        insecure: bool,
        generation: u64,
        tx: SyncSender<(u64, Event)>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::spawn(move || {
            if let Some(_permit) = NETWORK_SLOTS.acquire(&stopped) {
                run(url, user, password, insecure, generation, tx, stopped);
            }
        });
        Self { stop }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn stopped(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Acquire)
}
fn wait(flag: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while !stopped(flag) && Instant::now() < end {
        thread::sleep(
            Duration::from_millis(100).min(end.saturating_duration_since(Instant::now())),
        );
    }
}
fn emit(tx: &SyncSender<(u64, Event)>, generation: u64, event: Event, stop: &AtomicBool) {
    let mut message = (generation, event);
    loop {
        if stopped(stop) {
            return;
        }
        match tx.try_send(message) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(_)) if event == Event::Changed => return,
            Err(TrySendError::Full(m)) => {
                message = m;
                wait(stop, Duration::from_millis(50));
            }
        }
    }
}
fn run(
    url: String,
    user: String,
    password: String,
    insecure: bool,
    generation: u64,
    tx: SyncSender<(u64, Event)>,
    stop: Arc<AtomicBool>,
) {
    let mut retry = 1;
    while !stopped(&stop) {
        let mut connected_since = None;
        let result = connect(&url, &user, &password, insecure, &stop).and_then(|mut socket| {
            let mut buffer = String::new();
            let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
            socket
                .send(Message::Text(
                    "{\"protocol\":\"json\",\"version\":1}\u{1e}".into(),
                ))
                .map_err(|_| ())?;
            let mut handshaken = false;
            let mut last_rx = Instant::now();
            let mut last_ping = Instant::now();
            while !stopped(&stop) {
                if (!handshaken && Instant::now() > deadline) || last_rx.elapsed() > SERVER_TIMEOUT
                {
                    return Err(());
                }
                if handshaken && last_ping.elapsed() >= HEARTBEAT {
                    socket
                        .send(Message::Text("{\"type\":6}\u{1e}".into()))
                        .map_err(|_| ())?;
                    last_ping = Instant::now();
                }
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        last_rx = Instant::now();
                        if buffer.len().saturating_add(text.len()) > LIMIT {
                            return Err(());
                        }
                        buffer.push_str(&text);
                        while let Some(end) = buffer.find('\u{1e}') {
                            let record = buffer[..end].to_owned();
                            buffer.drain(..=end);
                            let value: Value = serde_json::from_str(&record).map_err(|_| ())?;
                            if !handshaken {
                                if !value.is_object()
                                    || value.get("error").is_some()
                                    || value.get("type").is_some()
                                {
                                    return Err(());
                                }
                                handshaken = true;
                                connected_since = Some(Instant::now());
                                emit(&tx, generation, Event::Connected, &stop);
                            } else if value.get("type").and_then(Value::as_u64) == Some(1)
                                && value.get("target").and_then(Value::as_str)
                                    == Some("RemoteProfileChanged")
                            {
                                emit(&tx, generation, Event::Changed, &stop);
                            } else if value.get("type").and_then(Value::as_u64) == Some(7) {
                                return Err(());
                            }
                        }
                    }
                    Ok(Message::Ping(_) | Message::Pong(_)) => {
                        last_rx = Instant::now();
                    }
                    Ok(Message::Close(_) | Message::Binary(_)) => return Err(()),
                    Ok(Message::Frame(_)) => return Err(()),
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => return Err(()),
                }
            }
            let _ = socket.close(None);
            Ok(())
        });
        if stopped(&stop) {
            break;
        }
        emit(&tx, generation, Event::Disconnected, &stop);
        // Rapid connect/close cycles must not reset backoff and flood the server.
        if connected_since.is_some_and(|since| since.elapsed() >= Duration::from_secs(30)) {
            retry = 1;
        }
        let _ = result; // No network payload, URLs, tokens, or credentials enter logs.
        wait(&stop, Duration::from_secs(retry));
        retry = (retry * 2).min(RETRY_CAP);
    }
}

fn base_url(url: &str) -> Result<Url, ()> {
    let mut base = Url::parse(url).map_err(|_| ())?;
    if base.scheme() != "https"
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(());
    }
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join("SyncClipboardHub").map_err(|_| ())
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;
fn connect(
    base: &str,
    user: &str,
    password: &str,
    insecure: bool,
    stop: &AtomicBool,
) -> Result<Socket, ()> {
    let hub = base_url(base)?;
    let mut negotiate = hub.clone();
    negotiate.set_path(&format!("{}/negotiate", hub.path()));
    negotiate.set_query(Some("negotiateVersion=1"));
    let client = Client::builder()
        .user_agent(USER_AGENT_VALUE)
        .timeout(HANDSHAKE_TIMEOUT)
        .danger_accept_invalid_certs(insecure)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ())?;
    let response = client
        .post(negotiate)
        .basic_auth(user, Some(password))
        .send()
        .map_err(|_| ())?;
    if !response.status().is_success() || stopped(stop) {
        return Err(());
    }
    let mut body = Vec::new();
    response
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| ())?;
    if body.len() > LIMIT {
        return Err(());
    }
    let data: Value = serde_json::from_slice(&body).map_err(|_| ())?;
    // Refuse SignalR redirects: they would forward credentials or a token.
    if data.get("url").is_some() || data.get("accessToken").is_some() || data.get("error").is_some()
    {
        return Err(());
    }
    let token = data
        .get("connectionToken")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 512
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
        })
        .ok_or(())?;
    let transports = data
        .get("availableTransports")
        .and_then(Value::as_array)
        .ok_or(())?;
    if !transports
        .iter()
        .any(|t| t.get("transport").and_then(Value::as_str) == Some("WebSockets"))
    {
        return Err(());
    }
    let mut ws = hub.clone();
    ws.set_scheme("wss").map_err(|_| ())?;
    ws.query_pairs_mut().append_pair("id", token);
    let host = ws.host_str().ok_or(())?;
    let port = ws.port_or_known_default().ok_or(())?;
    let mut stream = None;
    for addr in (host, port).to_socket_addrs().map_err(|_| ())? {
        if stopped(stop) {
            return Err(());
        }
        if let Ok(s) = TcpStream::connect_timeout(&addr, HANDSHAKE_TIMEOUT) {
            stream = Some(s);
            break;
        }
    }
    let stream = stream.ok_or(())?;
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|_| ())?;
    let mut request = ws.as_str().into_client_request().map_err(|_| ())?;
    let auth = format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
    request
        .headers_mut()
        .insert(AUTHORIZATION, auth.parse().map_err(|_| ())?);
    request
        .headers_mut()
        .insert(USER_AGENT, USER_AGENT_VALUE.parse().map_err(|_| ())?);
    let config = WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(4096)
        .max_write_buffer_size(16 * 1024)
        .max_message_size(Some(LIMIT))
        .max_frame_size(Some(LIMIT));
    let connector = Connector::Rustls(Arc::new(tls_config(insecure)?));
    let (mut socket, _) =
        client_tls_with_config(request, stream, Some(config), Some(connector)).map_err(|_| ())?;
    if let MaybeTlsStream::Rustls(s) = socket.get_mut() {
        s.sock
            .set_read_timeout(Some(Duration::from_secs(1)))
            .map_err(|_| ())?;
        s.sock
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| ())?;
    } else {
        return Err(());
    }
    Ok(socket)
}

#[derive(Debug)]
struct InsecureVerifier {
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}
impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, sig, &self.algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, sig, &self.algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
fn tls_config(insecure: bool) -> Result<rustls::ClientConfig, ()> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|_| ())?;
    if insecure {
        Ok(builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier {
                algorithms: provider.signature_verification_algorithms,
            }))
            .with_no_client_auth())
    } else {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        Ok(builder.with_root_certificates(roots).with_no_client_auth())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_url_and_path_rules() {
        assert_eq!(
            base_url("https://example.com/path").unwrap().as_str(),
            "https://example.com/path/SyncClipboardHub"
        );
        for url in [
            "http://example.com",
            "https://user@example.com",
            "https://example.com/?token=secret",
            "https://example.com/#x",
        ] {
            assert!(base_url(url).is_err());
        }
    }
    #[test]
    fn cancellation_interrupts_retry() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(4);
        let worker = Worker::start("invalid".into(), "u".into(), "p".into(), false, 1, tx);
        drop(worker);
    }

    #[test]
    fn stuck_workers_limit_network_concurrency_and_waiters_can_cancel() {
        use std::sync::mpsc;

        // Hold both slots to model two old generations stalled in DNS.
        let slots = Arc::new(Slots::new(2));
        let running = AtomicBool::new(false);
        let first = slots.acquire(&running).unwrap();
        let second = slots.acquire(&running).unwrap();
        assert_eq!(slots.in_use.load(Ordering::Acquire), 2);

        // A superseded replacement must leave its wait promptly, without
        // opening a network connection or requiring the stalled DNS to return.
        let cancel = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter_slots = slots.clone();
        let waiter_cancel = cancel.clone();
        let waiter = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let acquired = waiter_slots.acquire(&waiter_cancel).is_some();
            done_tx.send(acquired).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.store(true, Ordering::Release);
        assert!(!done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        waiter.join().unwrap();
        assert_eq!(slots.in_use.load(Ordering::Acquire), 2);

        // A current generation waits rather than exceeding the bound and can
        // connect when an old generation finally returns.
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let replacement_slots = slots.clone();
        let replacement = thread::spawn(move || {
            let flag = AtomicBool::new(false);
            let _permit = replacement_slots.acquire(&flag).unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        assert!(started_rx.recv_timeout(Duration::from_millis(150)).is_err());
        assert_eq!(slots.in_use.load(Ordering::Acquire), 2);
        drop(first);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(slots.in_use.load(Ordering::Acquire), 2);
        release_tx.send(()).unwrap();
        replacement.join().unwrap();
        drop(second);
        assert_eq!(slots.in_use.load(Ordering::Acquire), 0);
    }
}
