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
