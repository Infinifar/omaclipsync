//! Durable clipboard session. Network activity starts only on `check`,
//! `flush`, or `pull`, never while constructing a session.
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    adapters::{Clipboard, ClipboardError, Direction, HistorySink, StateStore},
    client::Client,
    state::{self, Ack, LocalCopy, Persisted, State},
    transport::Transport,
    Error, Filter, Outcome, MAX_TEXT_BYTES,
};

const MAX_RETRY_SECS: u64 = 7 * 24 * 60 * 60;
const MAX_CLOCK_SKEW_SECS: u64 = 5 * 60;

#[derive(Debug)]
pub enum SyncError {
    Remote(Error),
    Clipboard(ClipboardError),
    Storage(io::Error),
    Credentials,
    Conflict,
    Expired,
    Epoch,
}
impl From<Error> for SyncError {
    fn from(value: Error) -> Self {
        match value {
            Error::Status(401 | 403) => Self::Epoch,
            other => Self::Remote(other),
        }
    }
}
impl From<io::Error> for SyncError {
    fn from(value: io::Error) -> Self {
        Self::Storage(value)
    }
}

/// The host obtains the key from Secret Service, DPAPI, or its equivalent.
/// This type deliberately does not implement Debug.
pub struct SessionConfig {
    pub server: String,
    pub encryption_key: [u8; 32],
    pub filter: Filter,
    pub allow_sensitive: bool,
}

pub struct Session<C, S, H, R = Client> {
    pub remote: Arc<R>,
    pub persisted: Persisted,
    pub history: H,
    pub history_error: bool,
    pub clipboard: C,
    filter: Filter,
    allow_sensitive: bool,
    key: [u8; 32],
    storage: S,
    generation: Arc<AtomicU64>,
    echo: Option<(String, Instant)>,
    needs_pending_check: bool,
}

impl<C: Clipboard, S: StateStore, H: HistorySink, R: Transport> Session<C, S, H, R> {
    pub fn open(
        config: SessionConfig,
        remote: Arc<R>,
        storage: S,
        clipboard: C,
        history: H,
        generation: Arc<AtomicU64>,
    ) -> Result<Self, SyncError> {
        let SessionConfig {
            server,
            encryption_key: key,
            filter,
            allow_sensitive,
        } = config;
        let mut persisted = match storage.load()? {
            Some(bytes) => state::open(&key, bytes)?,
            None => {
                let value = Persisted {
                    server: server.clone(),
                    token: state::random_token()?,
                    paired: false,
                    state: State::new(String::new(), state::random_uuid()?, 0),
                };
                storage.save(&state::seal(&key, &value)?)?;
                value
            }
        };
        if persisted.server != server {
            return Err(SyncError::Epoch);
        }
        if persisted.state.applying.is_some() {
            persisted.state.recover_ambiguous_apply();
            storage.save(&state::seal(&key, &persisted)?)?;
        }
        let needs_pending_check = persisted.state.pending.is_some();
        Ok(Self {
            remote,
            persisted,
            history,
            history_error: false,
            clipboard,
            filter,
            allow_sensitive,
            key,
            storage,
            generation,
            echo: None,
            needs_pending_check,
        })
    }

    fn save(&self) -> Result<(), SyncError> {
        self.storage
            .save(&state::seal(&self.key, &self.persisted)?)
            .map_err(Into::into)
    }

    /// First registration records the then-current server version as baseline.
    pub fn check(&mut self) -> Result<(), SyncError> {
        if self.needs_pending_check {
            let expected = self
                .persisted
                .state
                .queued
                .as_ref()
                .or_else(|| self.persisted.state.pending.as_ref().map(|p| &p.copy));
            let actual = match self.clipboard.snapshot(self.allow_sensitive) {
                Ok(value) => value,
                Err(ClipboardError::Sensitive) => None,
                Err(error) => return Err(SyncError::Clipboard(error)),
            };
            if expected.is_some_and(|copy| {
                actual.as_ref().is_none_or(|(text, sensitive)| {
                    text != &copy.text || *sensitive != copy.sensitive
                })
            }) {
                self.persisted.state.local_clear();
                self.save()?;
            }
            self.needs_pending_check = false;
        }
        if !self.persisted.paired {
            let registered = self
                .remote
                .register(&self.persisted)
                .map_err(SyncError::Remote)?;
            if self
                .persisted
                .state
                .pending
                .as_ref()
                .is_some_and(|p| p.attempted)
            {
                return Err(SyncError::Epoch);
            }
            self.persisted.state.epoch = registered.epoch;
            self.persisted.state.cursor = registered.ack_revision;
            if let Some(pending) = self.persisted.state.pending.as_mut() {
                pending.base_revision = registered.ack_revision;
            }
            self.persisted.paired = true;
            self.save()?;
        }
        let remote = self.remote.state(&self.persisted)?;
        if remote.epoch != self.persisted.state.epoch
            || remote.revision < self.persisted.state.cursor
        {
            return Err(SyncError::Epoch);
        }
        if remote.ack_revision > self.persisted.state.cursor {
            self.persisted.state.cursor = remote.ack_revision;
            self.save()?;
        }
        Ok(())
    }

    pub fn record_local(
        &mut self,
        text: &str,
        marker: Option<&str>,
        sensitive: bool,
        now: Instant,
    ) -> Result<Outcome, SyncError> {
        if self
            .echo
            .as_ref()
            .is_some_and(|(expected, until)| marker == Some(expected.as_str()) && now <= *until)
        {
            return Ok(Outcome::Skipped);
        }
        self.echo = None;
        if text.is_empty() {
            return self.record_clear();
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(SyncError::Remote(Error::TooLarge));
        }
        if (sensitive && !self.allow_sensitive) || !self.filter.allows(text, false) {
            return self.record_clear().map(|_| Outcome::Filtered);
        }
        self.persisted.state.local_copy(LocalCopy {
            id: state::random_uuid()?,
            text: text.to_owned(),
            created_at: unix_time(),
            sensitive,
        });
        self.save()?;
        Ok(Outcome::Current)
    }

    pub fn record_clear(&mut self) -> Result<Outcome, SyncError> {
        self.echo = None;
        self.persisted.state.local_clear();
        self.save()?;
        Ok(Outcome::Skipped)
    }

    pub fn has_pending(&self) -> bool {
        self.persisted.state.pending.is_some() || self.persisted.state.ack.is_some()
    }

    /// Only called after explicit confirmation, since pending data is discarded.
    pub fn re_pair(&mut self) -> Result<(), SyncError> {
        self.persisted.token = state::random_token()?;
        self.persisted.state = State::new(String::new(), state::random_uuid()?, 0);
        self.persisted.paired = false;
        self.needs_pending_check = false;
        self.echo = None;
        self.save()?;
        self.check()
    }

    /// An explicit user action chooses the current local clipboard over the
    /// latest server value. Never silently rebase an automatic upload.
    pub fn push_now(&mut self, observed: u64) -> Result<Outcome, SyncError> {
        if !self.persisted.paired {
            self.check()?;
        }
        let Some((text, sensitive)) = self
            .clipboard
            .snapshot(self.allow_sensitive)
            .map_err(SyncError::Clipboard)?
        else {
            return Ok(Outcome::Skipped);
        };
        if text.is_empty() {
            return Ok(Outcome::Skipped);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(SyncError::Remote(Error::TooLarge));
        }
        if !self.filter.allows(&text, false) {
            return Ok(Outcome::Filtered);
        }
        if self.persisted.state.applying.is_some() {
            return Err(SyncError::Conflict);
        }
        if let Some(pending) = self.persisted.state.pending.clone().filter(|p| p.attempted) {
            match self.remote.cancel(&self.persisted, &pending) {
                Ok(_) => {}
                Err(Error::Status(409)) => return Err(SyncError::Epoch),
                Err(err) => return Err(err.into()),
            }
        }
        let remote = self.remote.state(&self.persisted)?;
        if remote.epoch != self.persisted.state.epoch {
            return Err(SyncError::Epoch);
        }
        if self.generation.load(Ordering::SeqCst) != observed
            || self
                .clipboard
                .snapshot(self.allow_sensitive)
                .map_err(SyncError::Clipboard)?
                .as_ref()
                .is_none_or(|(current, current_sensitive)| {
                    current != &text || *current_sensitive != sensitive
                })
        {
            return Ok(Outcome::Skipped);
        }
        let ack = remote.message().map(|message| Ack {
            id: message.id,
            revision: message.revision,
            result: if message.source_device_id == self.persisted.state.device_id {
                "self"
            } else {
                "skipped"
            }
            .into(),
        });
        let mut next = State::new(
            self.persisted.state.epoch.clone(),
            self.persisted.state.device_id.clone(),
            remote.revision,
        );
        next.ack = ack;
        next.local_copy(LocalCopy {
            id: state::random_uuid()?,
            text,
            created_at: unix_time(),
            sensitive,
        });
        self.persisted.state = next;
        self.save()?;
        self.flush(observed)
    }

    /// The caller supplies the watcher's change counter before entering this
    /// function. A new local event aborts an upload before it is sent.
    pub fn flush(&mut self, observed: u64) -> Result<Outcome, SyncError> {
        if !self.persisted.paired {
            self.check()?;
        }
        if !self.allow_sensitive
            && (self
                .persisted
                .state
                .queued
                .as_ref()
                .is_some_and(|c| c.sensitive)
                || (self
                    .persisted
                    .state
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.copy.sensitive)
                    && self.persisted.state.queued.is_none()))
        {
            self.persisted.state.local_clear();
            self.save()?;
        }
        if let Some(ack) = self.persisted.state.ack.clone() {
            match self.remote.acknowledge(&self.persisted, &ack) {
                Ok(()) => {}
                Err(Error::Status(410)) => {
                    let current = self.remote.state(&self.persisted)?;
                    if current.epoch != self.persisted.state.epoch
                        || current.revision <= ack.revision
                    {
                        return Err(SyncError::Epoch);
                    }
                }
                Err(error) => return Err(error.into()),
            }
            if self.persisted.state.ack.as_ref() == Some(&ack) {
                self.persisted.state.ack = None;
                self.save()?;
            }
        }
        let Some(pending) = self.persisted.state.pending.clone() else {
            return Ok(Outcome::Current);
        };
        let now = unix_time();
        if now.saturating_sub(pending.copy.created_at) > MAX_RETRY_SECS
            || now.saturating_add(MAX_CLOCK_SKEW_SECS) < pending.copy.created_at
        {
            return Err(SyncError::Expired);
        }
        if self.persisted.state.queued.is_some() || self.persisted.state.discard_pending {
            match self.remote.cancel(&self.persisted, &pending) {
                Ok(Some(revision)) => self
                    .persisted
                    .state
                    .committed(&pending.copy.id, revision)
                    .map_err(invalid_state)?,
                Ok(None) => self
                    .persisted
                    .state
                    .cancelled(&pending.copy.id)
                    .map_err(invalid_state)?,
                Err(Error::Status(409)) => return Err(SyncError::Conflict),
                Err(err) => return Err(err.into()),
            }
            self.save()?;
            return Ok(Outcome::Current);
        }
        let request = self.persisted.state.attempt().map_err(invalid_state)?;
        self.save()?;
        if self.generation.load(Ordering::SeqCst) != observed {
            return Ok(Outcome::Skipped);
        }
        let revision = match self.remote.send(&self.persisted, &request) {
            Ok(revision) => revision,
            Err(Error::Status(409)) => return Err(SyncError::Conflict),
            Err(err) => return Err(err.into()),
        };
        self.persisted
            .state
            .committed(&request.copy.id, revision)
            .map_err(invalid_state)?;
        self.save()?;
        self.record_history(&request.copy.text, Direction::Upload);
        Ok(Outcome::Uploaded)
    }

    pub fn pull(&mut self, observed: u64, now: Instant) -> Result<Outcome, SyncError> {
        if !self.persisted.paired {
            self.check()?;
        }
        if self.persisted.state.pending.is_some() || self.persisted.state.applying.is_some() {
            return Ok(Outcome::Skipped);
        }
        let before = self
            .clipboard
            .snapshot(self.allow_sensitive)
            .map_err(SyncError::Clipboard)?;
        let remote = self.remote.state(&self.persisted)?;
        if remote.epoch != self.persisted.state.epoch
            || remote.revision < self.persisted.state.cursor
        {
            return Err(SyncError::Epoch);
        }
        if remote.ack_revision > self.persisted.state.cursor {
            self.persisted.state.cursor = remote.ack_revision;
            self.save()?;
        }
        let Some(message) = remote.message() else {
            return Ok(Outcome::Current);
        };
        if message.revision <= self.persisted.state.cursor {
            return Ok(Outcome::Current);
        }
        if message.source_device_id == self.persisted.state.device_id {
            self.persisted
                .state
                .acknowledge_self(message)
                .map_err(invalid_state)?;
            self.save()?;
            return Ok(Outcome::Skipped);
        }
        if message.sensitive && !self.allow_sensitive {
            self.persisted.state.cursor = message.revision;
            self.persisted.state.ack = Some(Ack {
                id: message.id,
                revision: message.revision,
                result: "skipped".into(),
            });
            self.save()?;
            return Ok(Outcome::Filtered);
        }
        let text = match self.remote.content(&self.persisted, &message) {
            Ok(text) => text,
            Err(Error::Status(410)) => return Ok(Outcome::Skipped),
            Err(err) => return Err(err.into()),
        };
        if Some(text.len() as u64) != remote.size {
            return Err(SyncError::Remote(Error::InvalidProfile));
        }
        if !self.filter.allows(&text, true) {
            self.persisted.state.cursor = message.revision;
            self.persisted.state.ack = Some(Ack {
                id: message.id,
                revision: message.revision,
                result: "skipped".into(),
            });
            self.save()?;
            return Ok(Outcome::Filtered);
        }
        let confirm = self.remote.state(&self.persisted)?;
        if confirm.epoch != remote.epoch || confirm.message() != Some(message.clone()) {
            return Ok(Outcome::Skipped);
        }
        if self.generation.load(Ordering::SeqCst) != observed
            || self
                .clipboard
                .snapshot(self.allow_sensitive)
                .map_err(SyncError::Clipboard)?
                != before
        {
            return Ok(Outcome::Skipped);
        }
        self.persisted
            .state
            .prepare_apply(message.clone())
            .map_err(invalid_state)?;
        self.save()?;
        if self.generation.load(Ordering::SeqCst) != observed
            || self
                .clipboard
                .snapshot(self.allow_sensitive)
                .map_err(SyncError::Clipboard)?
                != before
        {
            self.persisted
                .state
                .finish_apply(&message.id, false)
                .map_err(invalid_state)?;
            self.save()?;
            return Ok(Outcome::Skipped);
        }
        // If the write or subsequent save has an ambiguous outcome, leave the
        // durable journal intact; recovery skips it instead of replaying it.
        self.clipboard
            .write(&text, &message.id, message.sensitive)
            .map_err(SyncError::Clipboard)?;
        self.echo = Some((message.id.clone(), now + Duration::from_secs(5)));
        self.persisted
            .state
            .finish_apply(&message.id, true)
            .map_err(invalid_state)?;
        self.save()?;
        self.record_history(&text, Direction::Download);
        Ok(Outcome::Downloaded)
    }

    fn record_history(&mut self, text: &str, direction: Direction) {
        self.history_error = self.history.record(text, direction, unix_time()).is_err();
    }
}

fn invalid_state(_: &'static str) -> SyncError {
    SyncError::Storage(io::Error::other("invalid sync state"))
}
fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
