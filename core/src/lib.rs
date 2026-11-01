//! RustClipSync synchronization core and Linux platform adapters.
pub use rustclipsync_core::{hash, Error, Filter, FilterMode, Outcome, MAX_TEXT_BYTES};
pub mod client;
pub mod config;
pub mod history;
pub mod inbox;
pub mod keyring;
pub mod logging;
pub mod protocol;
pub mod session;
pub mod state;
mod storage;
pub mod wayland;
pub mod worker;
