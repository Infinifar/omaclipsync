//! Linux adapters for the shared engine. No synchronization policy lives here.
use crate::{
    hash,
    history::History,
    keyring,
    v1_state::{self, FileStateStore},
    wayland, Filter,
};
pub use rustclipsync_core::{
    adapters::{Clipboard, ClipboardError},
    session::SyncError as V1Error,
};
use rustclipsync_core::{
    client::Client,
    session::{Session as CoreSession, SessionConfig},
};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    sync::{atomic::AtomicU64, Arc},
    time::Instant,
};

pub struct WaylandClipboard;
impl Clipboard for WaylandClipboard {
    fn snapshot(&self, allow_sensitive: bool) -> Result<Option<(String, bool)>, ClipboardError> {
        match wayland::read_current_with_sensitivity(allow_sensitive) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(wayland::ClipboardError::Unavailable) => Ok(None),
            Err(error) => Err(clipboard_error(error)),
        }
    }
    fn write(&self, text: &str, message_id: &str, sensitive: bool) -> Result<(), ClipboardError> {
        wayland::write_text_marked(text, message_id, sensitive).map_err(clipboard_error)
    }
}
fn clipboard_error(error: wayland::ClipboardError) -> ClipboardError {
    match error {
        wayland::ClipboardError::Io(e) => ClipboardError::Io(e),
        wayland::ClipboardError::TooLarge => ClipboardError::TooLarge,
        wayland::ClipboardError::InvalidUtf8 => ClipboardError::InvalidUtf8,
        wayland::ClipboardError::Timeout => ClipboardError::Timeout,
        wayland::ClipboardError::Sensitive => ClipboardError::Sensitive,
        wayland::ClipboardError::Unavailable => ClipboardError::Unavailable,
        wayland::ClipboardError::Wayland(_) | wayland::ClipboardError::Copy(_) => {
            ClipboardError::Platform
        }
    }
}

pub struct Session<C>(CoreSession<C, FileStateStore, History>);
impl<C> Deref for Session<C> {
    type Target = CoreSession<C, FileStateStore, History>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<C> DerefMut for Session<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<C: Clipboard> Session<C> {
    /// Explicit inbox copies use our durable device UUID as their local origin.
    /// Recognize it separately from remote message echoes, including after a
    /// policy reload or restart. A normal user copy has no such origin.
    pub fn record_local(
        &mut self,
        text: &str,
        marker: Option<&str>,
        sensitive: bool,
        now: Instant,
    ) -> Result<rustclipsync_core::Outcome, V1Error> {
        if marker == Some(self.persisted.state.device_id.as_str()) {
            return self.0.record_clear();
        }
        self.0.record_local(text, marker, sensitive, now)
    }

    pub fn copy_inbox(&mut self, id: &str) -> Result<bool, V1Error> {
        let Some(item) = self.persisted.inbox.as_ref().filter(|item| item.id == id) else {
            return Ok(false);
        };
        self.clipboard
            .write(&item.text, &self.persisted.state.device_id, true)
            .map_err(V1Error::Clipboard)?;
        // This selection supersedes any queued local copy. Clear it now rather
        // than waiting for the Wayland event, so an old outbox cannot flush first.
        self.0.record_clear()?;
        Ok(true)
    }

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
        let remote = Arc::new(Client::new(server, user.to_owned(), password, insecure)?);
        let identity = hash(&format!("{server}\0{user}"));
        let path = state_dir.join(format!("v1-{identity}.state"));
        let account = format!("v1-state-key-{identity}");
        let key = match keyring::lookup(&account) {
            Ok(value) => decode_key(&value).ok_or(V1Error::Credentials)?,
            Err(_) if !path.try_exists()? => {
                let key = v1_state::random_key()?;
                let encoded: String = key.iter().map(|b| format!("{b:02x}")).collect();
                keyring::store(&account, &encoded).map_err(|_| V1Error::Credentials)?;
                key
            }
            Err(_) => return Err(V1Error::Credentials),
        };
        let config = SessionConfig {
            server: server.to_owned(),
            encryption_key: key,
            filter,
            allow_sensitive,
        };
        CoreSession::open(
            config,
            remote,
            FileStateStore(path),
            clipboard,
            history,
            generation,
        )
        .map(Self)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustclipsync_core::{state::InboxItem, FilterMode, Outcome};
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct MemoryClipboard(Rc<RefCell<Vec<(String, String, bool)>>>);
    impl Clipboard for MemoryClipboard {
        fn snapshot(&self, _: bool) -> Result<Option<(String, bool)>, ClipboardError> {
            Ok(None)
        }
        fn write(&self, text: &str, origin: &str, sensitive: bool) -> Result<(), ClipboardError> {
            self.0
                .borrow_mut()
                .push((text.into(), origin.into(), sensitive));
            Ok(())
        }
    }

    #[test]
    fn explicit_copy_is_id_checked_marked_and_not_reuploaded_even_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.state");
        let clipboard = MemoryClipboard::default();
        let open = || {
            Session(
                CoreSession::open(
                    SessionConfig {
                        server: "https://example.test".into(),
                        encryption_key: [7; 32],
                        filter: Filter::new(FilterMode::Off, &[], false).unwrap(),
                        allow_sensitive: true,
                    },
                    Arc::new(
                        Client::new(
                            "https://example.test",
                            "fixture".into(),
                            String::new(),
                            false,
                        )
                        .unwrap(),
                    ),
                    FileStateStore(path.clone()),
                    clipboard.clone(),
                    History::new(dir.path().join("history.json"), false, 50).unwrap(),
                    Arc::new(AtomicU64::new(0)),
                )
                .unwrap(),
            )
        };
        let mut session = open();
        let id = v1_state::random_uuid().unwrap();
        session.persisted.inbox = Some(InboxItem {
            id: id.clone(),
            text: "marked fixture".into(),
            received_at: 42,
        });
        let cursor = session.persisted.state.cursor;
        assert!(!session.copy_inbox("old item").unwrap());
        assert!(clipboard.0.borrow().is_empty());
        session
            .record_local("old queued fixture", None, false, Instant::now())
            .unwrap();
        assert!(session.persisted.state.pending.is_some());
        assert!(session.copy_inbox(&id).unwrap());
        assert!(session.persisted.state.pending.is_none());
        let (text, marker, sensitive) = clipboard.0.borrow()[0].clone();
        assert!(sensitive);
        assert_eq!(marker, session.persisted.state.device_id);
        assert_eq!(
            session
                .record_local(&text, Some(&marker), true, Instant::now())
                .unwrap(),
            Outcome::Skipped
        );
        assert!(session.persisted.state.pending.is_none());
        assert_eq!(session.persisted.state.cursor, cursor);
        v1_state::save(&path, &[7; 32], &session.persisted).unwrap();
        drop(session);
        let mut reopened = open();
        assert_eq!(
            reopened
                .record_local(&text, Some(&marker), true, Instant::now())
                .unwrap(),
            Outcome::Skipped
        );
        assert!(reopened.persisted.state.pending.is_none());
        // The same literal copied by a user is a new intentional copy.
        assert_eq!(
            reopened
                .record_local(&text, None, true, Instant::now())
                .unwrap(),
            Outcome::Current
        );
        assert!(reopened.persisted.state.pending.is_some());
    }
}
