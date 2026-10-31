//! Durable V1 clipboard session. Network activity starts only on `check`,
//! `flush`, or `pull`, never while constructing a session.
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    hash,
    history::{Direction, History},
    keyring,
    session::Outcome,
    v1_client::Client,
    v1_state::{self, Ack, LocalCopy, Persisted, State},
    wayland::{self, ClipboardError},
    Error, Filter, MAX_TEXT_BYTES,
};

const MAX_RETRY_SECS: u64 = 7 * 24 * 60 * 60;
const MAX_CLOCK_SKEW_SECS: u64 = 5 * 60;

#[derive(Debug)]
pub enum V1Error {
    Remote(Error),
    Clipboard(ClipboardError),
    Storage(io::Error),
    Keyring,
    Conflict,
    Expired,
    Epoch,
}
impl From<Error> for V1Error {
    fn from(value: Error) -> Self {
        match value {
            Error::Status(401 | 403) => Self::Epoch,
            other => Self::Remote(other),
        }
    }
}
impl From<io::Error> for V1Error {
    fn from(value: io::Error) -> Self {
        Self::Storage(value)
    }
}

pub trait Clipboard {
    fn snapshot(&self, allow_sensitive: bool) -> Result<Option<(String, bool)>, ClipboardError>;
    fn write(&self, text: &str, message_id: &str, sensitive: bool) -> Result<(), ClipboardError>;
}

pub struct WaylandClipboard;
impl Clipboard for WaylandClipboard {
    fn snapshot(&self, allow_sensitive: bool) -> Result<Option<(String, bool)>, ClipboardError> {
        match wayland::read_current_with_sensitivity(allow_sensitive) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(ClipboardError::Unavailable) => Ok(None),
            Err(err) => Err(err),
        }
    }
    fn write(&self, text: &str, message_id: &str, sensitive: bool) -> Result<(), ClipboardError> {
        wayland::write_text_marked(text, message_id, sensitive)
    }
}

pub struct Session<C> {
    pub remote: Arc<Client>,
    pub persisted: Persisted,
    pub history: History,
    pub history_error: bool,
    pub clipboard: C,
    filter: Filter,
    allow_sensitive: bool,
    key: [u8; 32],
    path: PathBuf,
    generation: Arc<AtomicU64>,
    echo: Option<(String, Instant)>,
    needs_pending_check: bool,
}

impl<C: Clipboard> Session<C> {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        server: &str,
        user: &str,
        password: String,
        insecure: bool,
        state_dir: &Path,
        clipboard: C,
        filter: Filter,
        allow_sensitive: bool,
        history: History,
        generation: Arc<AtomicU64>,
    ) -> Result<Self, V1Error> {
        let client = Arc::new(Client::new(server, user.to_owned(), password, insecure)?);
        let identity = hash(&format!("{server}\0{user}"));
        let path = state_dir.join(format!("v1-{identity}.state"));
        let account = format!("v1-state-key-{identity}");
        let key = match keyring::lookup(&account) {
            Ok(value) => decode_key(&value).ok_or(V1Error::Keyring)?,
            Err(_) if !path.exists() => {
                let key = v1_state::random_key()?;
                let encoded: String = key.iter().map(|b| format!("{b:02x}")).collect();
                keyring::store(&account, &encoded).map_err(|_| V1Error::Keyring)?;
                key
            }
            Err(_) => return Err(V1Error::Keyring),
        };
        let mut persisted = if path.exists() {
            v1_state::load(&path, &key)?
        } else {
            let value = Persisted {
                server: server.to_owned(),
                token: v1_state::random_token()?,
                paired: false,
                state: State::new(String::new(), v1_state::random_uuid()?, 0),
            };
            v1_state::save(&path, &key, &value)?;
            value
        };
        if persisted.server != server {
            return Err(V1Error::Epoch);
        }
        if persisted.state.applying.is_some() {
            persisted.state.recover_ambiguous_apply();
            v1_state::save(&path, &key, &persisted)?;
        }
        let needs_pending_check = persisted.state.pending.is_some();
        Ok(Self {
            remote: client,
            persisted,
            history,
            history_error: false,
            clipboard,
            filter,
            allow_sensitive,
            key,
            path,
            generation,
            echo: None,
            needs_pending_check,
        })
    }

    fn save(&self) -> Result<(), V1Error> {
        v1_state::save(&self.path, &self.key, &self.persisted).map_err(Into::into)
    }

    /// First registration records the then-current server version as baseline.
    pub fn check(&mut self) -> Result<(), V1Error> {
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
                Err(error) => return Err(V1Error::Clipboard(error)),
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
                .map_err(V1Error::Remote)?;
            if self
                .persisted
                .state
                .pending
                .as_ref()
                .is_some_and(|p| p.attempted)
            {
                return Err(V1Error::Epoch);
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
            return Err(V1Error::Epoch);
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
    ) -> Result<Outcome, V1Error> {
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
            return Err(V1Error::Remote(Error::TooLarge));
        }
        if (sensitive && !self.allow_sensitive) || !self.filter.allows(text, false) {
            return self.record_clear().map(|_| Outcome::Filtered);
        }
        self.persisted.state.local_copy(LocalCopy {
            id: v1_state::random_uuid()?,
            text: text.to_owned(),
            created_at: unix_time(),
            sensitive,
        });
        self.save()?;
        Ok(Outcome::Current)
    }

    pub fn record_clear(&mut self) -> Result<Outcome, V1Error> {
        self.echo = None;
        self.persisted.state.local_clear();
        self.save()?;
        Ok(Outcome::Skipped)
    }

    pub fn has_pending(&self) -> bool {
        self.persisted.state.pending.is_some() || self.persisted.state.ack.is_some()
    }

    /// Only called after explicit confirmation, since pending data is discarded.
    pub fn re_pair(&mut self) -> Result<(), V1Error> {
        self.persisted.token = v1_state::random_token()?;
        self.persisted.state = State::new(String::new(), v1_state::random_uuid()?, 0);
        self.persisted.paired = false;
        self.needs_pending_check = false;
        self.echo = None;
        self.save()?;
        self.check()
    }

    /// An explicit user action chooses the current local clipboard over the
    /// latest server value. Never silently rebase an automatic upload.
    pub fn push_now(&mut self, observed: u64) -> Result<Outcome, V1Error> {
        if !self.persisted.paired {
            self.check()?;
        }
        let Some((text, sensitive)) = self
            .clipboard
            .snapshot(self.allow_sensitive)
            .map_err(V1Error::Clipboard)?
        else {
            return Ok(Outcome::Skipped);
        };
        if text.is_empty() {
            return Ok(Outcome::Skipped);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(V1Error::Remote(Error::TooLarge));
        }
        if !self.filter.allows(&text, false) {
            return Ok(Outcome::Filtered);
        }
        if self.persisted.state.applying.is_some() {
            return Err(V1Error::Conflict);
        }
        if let Some(pending) = self.persisted.state.pending.clone().filter(|p| p.attempted) {
            match self.remote.cancel(&self.persisted, &pending) {
                Ok(_) => {}
                Err(Error::Status(409)) => return Err(V1Error::Epoch),
                Err(err) => return Err(err.into()),
            }
        }
        let remote = self.remote.state(&self.persisted)?;
        if remote.epoch != self.persisted.state.epoch {
            return Err(V1Error::Epoch);
        }
        if self.generation.load(Ordering::SeqCst) != observed
            || self
                .clipboard
                .snapshot(self.allow_sensitive)
                .map_err(V1Error::Clipboard)?
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
            id: v1_state::random_uuid()?,
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
    pub fn flush(&mut self, observed: u64) -> Result<Outcome, V1Error> {
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
                        return Err(V1Error::Epoch);
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
            return Err(V1Error::Expired);
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
                Err(Error::Status(409)) => return Err(V1Error::Conflict),
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
            Err(Error::Status(409)) => return Err(V1Error::Conflict),
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

    pub fn pull(&mut self, observed: u64, now: Instant) -> Result<Outcome, V1Error> {
        if !self.persisted.paired {
            self.check()?;
        }
        if self.persisted.state.pending.is_some() || self.persisted.state.applying.is_some() {
            return Ok(Outcome::Skipped);
        }
        let before = self
            .clipboard
            .snapshot(self.allow_sensitive)
            .map_err(V1Error::Clipboard)?;
        let remote = self.remote.state(&self.persisted)?;
        if remote.epoch != self.persisted.state.epoch
            || remote.revision < self.persisted.state.cursor
        {
            return Err(V1Error::Epoch);
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
            return Err(V1Error::Remote(Error::InvalidProfile));
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
                .map_err(V1Error::Clipboard)?
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
                .map_err(V1Error::Clipboard)?
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
            .map_err(V1Error::Clipboard)?;
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

fn invalid_state(_: &'static str) -> V1Error {
    V1Error::Storage(io::Error::other("invalid sync state"))
}
fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn decode_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}
