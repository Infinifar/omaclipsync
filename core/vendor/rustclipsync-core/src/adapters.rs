//! Host boundaries. This crate has no OS clipboard, keyring or filesystem code.
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Debug)]
pub enum ClipboardError {
    Io(io::Error),
    TooLarge,
    InvalidUtf8,
    Timeout,
    Sensitive,
    Unavailable,
    Platform,
}

pub trait Clipboard {
    /// None denotes an empty or unsupported selection. Disallowed marked text
    /// must produce Sensitive (or None), never a usable text snapshot.
    fn snapshot(&self, allow_sensitive: bool) -> Result<Option<(String, bool)>, ClipboardError>;
    /// Success means the OS clipboard accepted the text and origin marker.
    /// A private inbox alone must not return success for this operation.
    fn write(&self, text: &str, message_id: &str, sensitive: bool) -> Result<(), ClipboardError>;
}

pub trait StateStore {
    /// Return None only when no state exists. Bound reads to state::MAX_STATE;
    /// permissions, corruption and key failures must not reset the identity.
    fn load(&self) -> io::Result<Option<Vec<u8>>>;
    /// Atomically replace with these exact encrypted bytes, durably, with
    /// current-user access only. Do not append data or log the ciphertext/key.
    /// On failure the host must stop this session and reopen from disk.
    fn save(&self, ciphertext: &[u8]) -> io::Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Upload,
    Download,
}

pub trait HistorySink {
    type Error;
    /// Optional history failure does not roll back a completed synchronization.
    fn record(&self, text: &str, direction: Direction, timestamp: u64) -> Result<(), Self::Error>;
    /// Hosts may withhold marked content from ordinary history previews.
    fn record_marked(
        &self,
        text: &str,
        direction: Direction,
        timestamp: u64,
        _sensitive: bool,
    ) -> Result<(), Self::Error> {
        self.record(text, direction, timestamp)
    }
}

/// Default for clients without history: no retained text and no filesystem I/O.
pub struct NoHistory;
impl HistorySink for NoHistory {
    type Error = std::convert::Infallible;
    fn record(&self, _: &str, _: Direction, _: u64) -> Result<(), Self::Error> {
        Ok(())
    }
}
