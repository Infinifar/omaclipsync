//! Portable RustClipSync client engine. Construction performs no network or
//! clipboard activity. Platform integration is supplied through adapters.
#![forbid(unsafe_code)]

pub mod adapters;
pub mod client;
mod error;
mod filter;
pub mod session;
pub mod state;
pub mod transport;
pub mod worker;

pub use error::Error;
pub use filter::{Filter, FilterMode};
use sha2::{Digest, Sha256};

pub const MAX_TEXT_BYTES: usize = 10 * 1024 * 1024;

/// Stable uppercase SHA-256; also preserves existing account/state filenames.
pub fn hash(text: &str) -> String {
    format!("{:X}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Uploaded,
    Current,
    Downloaded,
    Skipped,
    Filtered,
    Paused,
    Unsupported,
    Invalid,
    Capacity,
}
