//! Serialized direction control. A failed WS operation is retried unchanged over HTTP.
use crate::{
    client::{valid_uuid, Client},
    direction::*,
    state::{self, Persisted},
    Error,
};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message, WebSocket};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const WS_RETRY: Duration = Duration::from_secs(30);
const REFRESH: Duration = Duration::from_secs(10);
// A quiet interval longer than the HTTP online lease is a reconnect, including
// suspended processes whose control socket has not yet exposed its close.
const INACTIVE_RECONNECT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    pub direction: SyncDirection,
    pub paused: bool,
}
struct Active {
    id: String,
    version: u64,
    reported: Instant,
}
#[derive(Clone)]
enum Mutation {
    Update(SyncDirectionUpdate),
    Revoke(SyncDirectionRevoke),
}
impl Mutation {
    fn id(&self) -> &str {
        match self {
            Self::Update(v) => &v.session_id,
            Self::Revoke(v) => &v.session_id,
        }
    }
    fn version(&self) -> u64 {
        match self {
            Self::Update(v) => v.version,
            Self::Revoke(v) => v.version,
        }
    }
    fn frame(&self) -> DirectionFrame {
        match self {
            Self::Update(v) => DirectionFrame::SyncDirectionUpdate(v.clone()),
            Self::Revoke(v) => DirectionFrame::SyncDirectionRevoke(v.clone()),
        }
    }
}
#[derive(Default)]
pub(crate) struct Control {
    pub desired: Option<Policy>,
    active: Option<Active>,
    pending: Option<Mutation>,
    socket: Option<Socket>,
    retry_ws: Option<Instant>,
    blocked: bool,
}
impl Control {
    pub fn configure(
        &mut self,
        client: &Client,
        s: &Persisted,
        direction: SyncDirection,
        paused: bool,
    ) -> Result<(), Error> {
        let next = Policy { direction, paused };
        if self.desired != Some(next) || self.blocked {
            self.pending = None;
            if self.blocked {
                self.active = None;
                self.socket = None;
            }
        }
        self.desired = Some(next);
        self.blocked = false;
        if s.paired {
            self.ensure(client, s, true).map(|_| ())
        } else {
            Ok(())
        }
    }
    pub fn ensure(&mut self, client: &Client, s: &Persisted, force: bool) -> Result<String, Error> {
        let policy = self.desired.ok_or(Error::DirectionRequired)?;
        if self.blocked {
            return Err(revoked());
        }
        if self.pending.is_none()
            && self
                .active
                .as_ref()
                .is_some_and(|v| v.reported.elapsed() >= INACTIVE_RECONNECT)
        {
            self.active = None;
            self.socket = None;
        }
        if self.pending.is_none() {
            let reconnect_ws =
                self.socket.is_none() && self.retry_ws.is_none_or(|t| Instant::now() >= t);
            if reconnect_ws && self.active.is_some() {
                self.active = None;
            }
            if let Some(active) = &self.active {
                if !force && active.reported.elapsed() < REFRESH {
                    return Ok(active.id.clone());
                }
            }
            let (id, version, connect) = if let Some(active) = &self.active {
                (active.id.clone(), next_version(active.version)?, false)
            } else {
                let status = client.direction_status(s)?;
                (state::random_uuid()?, next_version(status.version)?, true)
            };
            self.pending = Some(Mutation::Update(SyncDirectionUpdate {
                session_id: id,
                version,
                direction: policy.direction,
                paused: policy.paused,
                connect,
            }));
        }
        let operation = self.pending.clone().ok_or(Error::InvalidProfile)?;
        let ack = self.transmit(client, s, &operation)?;
        self.pending = None;
        self.active = Some(Active {
            id: ack.session_id.clone(),
            version: ack.version,
            reported: Instant::now(),
        });
        Ok(ack.session_id)
    }
    pub fn revoke(&mut self, client: &Client, s: &Persisted) -> Result<(), Error> {
        // Change the local policy before network work; failures never re-enable data.
        self.desired = None;
        if !s.paired {
            self.pending = None;
            self.active = None;
            self.socket = None;
            return Ok(());
        }
        let operation = if let Some(Mutation::Revoke(v)) = &self.pending {
            Mutation::Revoke(v.clone())
        } else {
            let (id, version) = if let Some(active) = &self.active {
                (active.id.clone(), active.version)
            } else {
                let status = client.direction_status(s)?;
                if status.revoked || status.session_id.is_none() {
                    self.pending = None;
                    self.socket = None;
                    return Ok(());
                }
                (
                    status.session_id.ok_or(Error::InvalidProfile)?,
                    status.version,
                )
            };
            Mutation::Revoke(SyncDirectionRevoke {
                session_id: id,
                version: next_version(version)?,
            })
        };
        self.pending = Some(operation.clone());
        self.transmit(client, s, &operation)?;
        self.pending = None;
        self.active = None;
        self.socket = None;
        self.blocked = false;
        Ok(())
    }
    fn transmit(
        &mut self,
        client: &Client,
        s: &Persisted,
        op: &Mutation,
    ) -> Result<SyncDirectionAck, Error> {
        let ws_allowed = self.socket.is_some()
            || (matches!(op,Mutation::Update(v) if v.connect)
                && self.retry_ws.is_none_or(|t| Instant::now() >= t));
        let ack = if ws_allowed {
            match self.ws(client, s, op) {
                Ok(ack) => Ok(ack),
                Err(Error::Protocol(e)) => Err(Error::Protocol(e)),
                Err(_) => {
                    self.socket = None;
                    self.retry_ws = Some(Instant::now() + WS_RETRY);
                    self.http(client, s, op)
                }
            }
        } else {
            self.http(client, s, op)
        };
        let ack = match ack {
            Ok(v) => v,
            Err(e) => {
                if matches!(&e,Error::Protocol(v) if matches!(v.code,DirectionErrorCode::VersionStale|DirectionErrorCode::SessionRevoked|DirectionErrorCode::Unauthenticated|DirectionErrorCode::DeviceMismatch|DirectionErrorCode::ProtocolUnsupported))
                {
                    self.fence();
                }
                return Err(e);
            }
        };
        if ack.device_id != s.state.device_id
            || ack.session_id != op.id()
            || ack.version != op.version()
            || ack.revoked != matches!(op, Mutation::Revoke(_))
            || !valid_uuid(&ack.session_id)
            || ack.server_time < 0
        {
            return Err(Error::InvalidProfile);
        }
        Ok(ack)
    }
    fn http(
        &self,
        client: &Client,
        s: &Persisted,
        op: &Mutation,
    ) -> Result<SyncDirectionAck, Error> {
        let url = client.direction_url(s)?;
        let req = match op {
            Mutation::Update(v) => client.raw_bearer(client.http.put(url), s).json(v),
            Mutation::Revoke(v) => client.raw_bearer(client.http.delete(url), s).json(v),
        };
        Client::json(Client::perform(req)?)
    }
    fn ws(
        &mut self,
        client: &Client,
        s: &Persisted,
        op: &Mutation,
    ) -> Result<SyncDirectionAck, Error> {
        if self.socket.is_none() {
            self.socket = Some(Socket::connect(client, s)?);
        }
        let socket = self.socket.as_mut().ok_or(Error::InvalidProfile)?;
        socket.stream.get_mut().sock.deadline = Instant::now() + CONTROL_TIMEOUT;
        let text = serde_json::to_string(&op.frame())?;
        socket
            .stream
            .send(Message::Text(text.into()))
            .map_err(ws_error)?;
        // A bounded frame count and one stream-level deadline cover ping storms
        // and slow fragments; neither can extend the operation indefinitely.
        for _ in 0..16 {
            match socket.stream.read().map_err(ws_error)? {
                Message::Text(text) => match serde_json::from_str::<DirectionFrame>(&text)? {
                    DirectionFrame::SyncDirectionAck(v) => return Ok(v),
                    DirectionFrame::SyncDirectionError(v) => return Err(Error::Protocol(v)),
                    _ => return Err(Error::InvalidProfile),
                },
                Message::Ping(_) => socket.stream.flush().map_err(ws_error)?,
                Message::Pong(_) => {}
                _ => return Err(Error::InvalidProfile),
            }
        }
        Err(Error::InvalidProfile)
    }
    pub fn fence(&mut self) {
        self.blocked = true;
        self.pending = None;
        self.socket = None;
    }
    pub fn is_current(&self, id: &str) -> bool {
        self.active.as_ref().is_some_and(|v| v.id == id)
    }
    pub fn data_error(&mut self, id: &str, error: &Error) {
        if !self.is_current(id) {
            return;
        }
        match error {
            Error::Protocol(e)
                if matches!(
                    e.code,
                    DirectionErrorCode::SessionRevoked
                        | DirectionErrorCode::VersionStale
                        | DirectionErrorCode::DeviceMismatch
                        | DirectionErrorCode::Unauthenticated
                        | DirectionErrorCode::ProtocolUnsupported
                ) =>
            {
                self.fence()
            }
            Error::Protocol(e) if e.code == DirectionErrorCode::SessionOffline => {
                self.disconnected()
            }
            Error::Http(_) | Error::Io(_) => self.disconnected(),
            _ => {}
        }
    }
    pub fn disconnected(&mut self) {
        if !self.blocked {
            self.active = None;
            self.pending = None;
            self.socket = None;
        }
    }
}
fn next_version(version: u64) -> Result<u64, Error> {
    version
        .checked_add(1)
        .filter(|v| *v <= MAX_DIRECTION_VERSION)
        .ok_or(Error::InvalidProfile)
}
fn revoked() -> Error {
    Error::Protocol(SyncDirectionError {
        code: DirectionErrorCode::SessionRevoked,
        version: None,
        minimum_protocol_version: None,
    })
}
fn ws_error(error: tungstenite::Error) -> Error {
    Error::Io(io::Error::other(error))
}

struct DeadlineStream {
    tcp: TcpStream,
    deadline: Instant,
}
impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "control operation timed out"))
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.tcp.set_read_timeout(Some(self.remaining()?))?;
        self.tcp.read(b)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.tcp.set_write_timeout(Some(self.remaining()?))?;
        self.tcp.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.set_write_timeout(Some(self.remaining()?))?;
        self.tcp.flush()
    }
}
struct Socket {
    stream: WebSocket<StreamOwned<ClientConnection, DeadlineStream>>,
}
impl Socket {
    fn connect(client: &Client, s: &Persisted) -> Result<Self, Error> {
        let mut url = client.url("api/ws")?;
        url.set_scheme("wss").map_err(|_| Error::InvalidUrl)?;
        let host = url.host_str().ok_or(Error::InvalidUrl)?;
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        // Reuse the authenticated HTTPS discovery's address. Performing another
        // blocking system DNS lookup here would escape the control deadline.
        // Proxy-only connections may fail WSS and use the identical HTTP retry.
        let address = client
            .address
            .lock()
            .map_err(|_| Error::InvalidProfile)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "HTTPS peer address unavailable",
                )
            })?;
        let tcp = TcpStream::connect_timeout(&address, CONTROL_TIMEOUT)?;
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::InvalidProfile)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        if client.insecure {
            config
                .dangerous()
                .set_certificate_verifier(Arc::new(UntrustedCertificates { provider }));
        }
        let name = rustls::pki_types::ServerName::try_from(host.to_owned())
            .map_err(|_| Error::InvalidUrl)?;
        let tls = ClientConnection::new(Arc::new(config), name)
            .map_err(|e| Error::Io(io::Error::other(e)))?;
        let tls = StreamOwned::new(tls, DeadlineStream { tcp, deadline });
        let mut request = url.as_str().into_client_request().map_err(ws_error)?;
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}:{}", s.state.device_id, s.token)
                .parse()
                .map_err(|_| Error::InvalidProfile)?,
        );
        request.headers_mut().insert(
            "x-rcs-protocol",
            "3".parse().map_err(|_| Error::InvalidProfile)?,
        );
        let config = WebSocketConfig::default()
            .read_buffer_size(8192)
            .write_buffer_size(4096)
            .max_write_buffer_size(16384)
            .max_message_size(Some(4096))
            .max_frame_size(Some(4096));
        let (stream, response) =
            tungstenite::client::client_with_config(request, tls, Some(config))
                .map_err(|e| Error::Io(io::Error::other(e)))?;
        if response
            .headers()
            .get("x-rcs-protocol")
            .and_then(|v| v.to_str().ok())
            != Some("3")
        {
            return Err(Error::Protocol(SyncDirectionError {
                code: DirectionErrorCode::ProtocolUnsupported,
                version: None,
                minimum_protocol_version: Some(MIN_PROTOCOL_VERSION),
            }));
        }
        Ok(Self { stream })
    }
}
// The existing explicit insecure test/self-signed option applies equally to
// HTTPS and WSS. Even here, TLS handshake signatures are cryptographically checked.
#[derive(Debug)]
struct UntrustedCertificates {
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl rustls::client::danger::ServerCertVerifier for UntrustedCertificates {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn active(id: &str) -> Control {
        Control {
            desired: Some(Policy {
                direction: SyncDirection::Bidirectional,
                paused: false,
            }),
            active: Some(Active {
                id: id.into(),
                version: 7,
                reported: Instant::now(),
            }),
            ..Control::default()
        }
    }
    fn protocol(code: DirectionErrorCode) -> Error {
        Error::Protocol(SyncDirectionError {
            code,
            version: Some(7),
            minimum_protocol_version: None,
        })
    }
    #[test]
    fn stale_inflight_error_cannot_fence_a_newer_session() {
        let mut c = active("new-session");
        c.data_error("old-session", &protocol(DirectionErrorCode::SessionRevoked));
        c.data_error(
            "old-session",
            &Error::Io(io::Error::new(io::ErrorKind::TimedOut, "old request")),
        );
        assert!(c.is_current("new-session"));
        assert!(!c.blocked);
        c.data_error("new-session", &protocol(DirectionErrorCode::SessionRevoked));
        assert!(
            c.blocked,
            "a real current-session revocation must remain terminal"
        );
    }
    #[test]
    fn offline_lease_retains_policy_for_fresh_cas_report_without_retrying_old_data() {
        let mut c = active("session");
        c.data_error("session", &protocol(DirectionErrorCode::SessionOffline));
        assert!(c.active.is_none());
        assert!(!c.blocked);
        assert_eq!(c.desired.unwrap().direction, SyncDirection::Bidirectional);
        assert!(c.pending.is_none());
    }
}
