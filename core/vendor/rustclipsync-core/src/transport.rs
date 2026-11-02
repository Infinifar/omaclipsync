//! Transport boundary, implemented by the HTTPS client and deterministic tests.
use crate::{
    client::{Client, Registration, RemoteState},
    direction::SyncDirection,
    state::{Ack, Message, Outbox, Persisted},
    Error,
};

/// Blocking protocol operations with finite, implementation-defined timeouts.
/// Implementations must bound network reads, writes and connection setup so the
/// host can stop synchronization and recover from unavailable peers.
pub trait Transport: Send + Sync {
    /// Configure local direction and report it when the identity is paired.
    fn set_sync_direction(
        &self,
        session: &Persisted,
        direction: SyncDirection,
        paused: bool,
    ) -> Result<(), Error>;
    /// Ensure configured direction is acknowledged before accessing metadata or content.
    fn ensure_sync_direction(&self, session: &Persisted) -> Result<(), Error>;
    /// Explicit logout; a transport failure means revocation is unconfirmed.
    fn revoke_sync_direction(&self, session: &Persisted) -> Result<(), Error>;
    fn register(&self, session: &Persisted) -> Result<Registration, Error>;
    fn state(&self, session: &Persisted) -> Result<RemoteState, Error>;
    /// Wait for metadata changes, returning `None` when the long poll expires.
    /// Enforce and document a finite timeout for the entire request, including
    /// reading its response. The built-in client uses 30 seconds for the HTTP
    /// poll, plus up to 40 seconds for discovery/control and one HTTP fallback.
    /// Worker shutdown cannot interrupt this call and join waits for it to return.
    ///
    /// # Errors
    /// Returns a transport or protocol error if the request cannot complete.
    fn changes(&self, session: &Persisted) -> Result<Option<RemoteState>, Error>;
    fn content(&self, session: &Persisted, message: &Message) -> Result<String, Error>;
    fn send(&self, session: &Persisted, outbox: &Outbox) -> Result<u64, Error>;
    fn cancel(&self, session: &Persisted, outbox: &Outbox) -> Result<Option<u64>, Error>;
    fn acknowledge(&self, session: &Persisted, ack: &Ack) -> Result<(), Error>;
}

impl Transport for Client {
    fn ensure_sync_direction(&self, s: &Persisted) -> Result<(), Error> {
        Client::ensure_sync_direction(self, s)
    }
    fn set_sync_direction(
        &self,
        s: &Persisted,
        d: SyncDirection,
        paused: bool,
    ) -> Result<(), Error> {
        Client::set_sync_direction(self, s, d, paused)
    }
    fn revoke_sync_direction(&self, s: &Persisted) -> Result<(), Error> {
        Client::revoke_sync_direction(self, s)
    }
    fn register(&self, s: &Persisted) -> Result<Registration, Error> {
        Client::register(self, s)
    }
    fn state(&self, s: &Persisted) -> Result<RemoteState, Error> {
        Client::state(self, s)
    }
    fn changes(&self, s: &Persisted) -> Result<Option<RemoteState>, Error> {
        Client::changes(self, s)
    }
    fn content(&self, s: &Persisted, m: &Message) -> Result<String, Error> {
        Client::content(self, s, m)
    }
    fn send(&self, s: &Persisted, o: &Outbox) -> Result<u64, Error> {
        Client::send(self, s, o)
    }
    fn cancel(&self, s: &Persisted, o: &Outbox) -> Result<Option<u64>, Error> {
        Client::cancel(self, s, o)
    }
    fn acknowledge(&self, s: &Persisted, a: &Ack) -> Result<(), Error> {
        Client::acknowledge(self, s, a)
    }
}
