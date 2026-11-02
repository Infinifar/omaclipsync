//! Device direction control shared by WebSocket and HTTP, separate from clipboard data.
use serde::{Deserialize, Serialize};

/// Wire protocol required by this core; `/api/` itself is not a version selector.
pub const PROTOCOL_VERSION: u32 = 3;
/// Minimum peer protocol supported by this coordinated breaking release.
pub const MIN_PROTOCOL_VERSION: u32 = 3;
/// Largest exactly representable JSON integer across supported clients.
pub const MAX_DIRECTION_VERSION: u64 = 9_007_199_254_740_991;

/// The selected synchronization direction. Pause is a separate work state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SyncDirection {
    PushOnly,
    ReceiveOnly,
    Bidirectional,
}
impl SyncDirection {
    /// Whether the selected direction permits publishing local text.
    pub fn allows_push(self) -> bool {
        self != Self::ReceiveOnly
    }
    /// Whether the selected direction permits receiving remote text.
    pub fn allows_receive(self) -> bool {
        self != Self::PushOnly
    }
}

/// An authenticated snapshot; reading it never activates direction or refreshes TTL.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncDirectionStatus {
    pub device_id: String,
    pub session_id: Option<String>,
    pub version: u64,
    pub direction: Option<SyncDirection>,
    pub paused: bool,
    pub updated_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

/// An update uses CAS version=current+1; a new session is claimed with connect=true.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDirectionUpdate {
    pub session_id: String,
    pub version: u64,
    pub direction: SyncDirection,
    pub paused: bool,
    pub connect: bool,
}
/// Explicit logout. Ordinary process or socket close must not send this message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDirectionRevoke {
    pub session_id: String,
    pub version: u64,
}
/// Confirmation binds the accepted device, session and version, not just HTTP success.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncDirectionAck {
    pub device_id: String,
    pub session_id: String,
    pub version: u64,
    pub server_time: i64,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}
/// Machine-readable direction failures. Hosts must not retry stale settings blindly.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DirectionErrorCode {
    Unauthenticated,
    DeviceMismatch,
    VersionStale,
    SessionRevoked,
    SessionOffline,
    RateLimited,
    ProtocolUnsupported,
    InvalidDirection,
    DirectionForbidden,
}
/// The same failure body is used by HTTP and WebSocket control replies.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncDirectionError {
    pub code: DirectionErrorCode,
    pub version: Option<u64>,
    pub minimum_protocol_version: Option<u32>,
}
/// Bounded text WebSocket control messages; data remains on the authenticated HTTP API.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DirectionFrame {
    SyncDirectionUpdate(SyncDirectionUpdate),
    SyncDirectionRevoke(SyncDirectionRevoke),
    SyncDirectionAck(SyncDirectionAck),
    SyncDirectionError(SyncDirectionError),
}

impl SyncDirectionUpdate {
    /// Validate session identity and the monotonic mutation version.
    ///
    /// # Errors
    /// Rejects noncanonical UUIDs, zero and versions outside the interoperable JSON range.
    pub fn validate(&self) -> Result<(), crate::Error> {
        validate_mutation(&self.session_id, self.version)
    }
}
impl SyncDirectionRevoke {
    /// Validate the session and version retained by a revocation tombstone.
    ///
    /// # Errors
    /// Returns invalid-profile for malformed sessions or out-of-range versions.
    pub fn validate(&self) -> Result<(), crate::Error> {
        validate_mutation(&self.session_id, self.version)
    }
}
fn validate_mutation(session: &str, version: u64) -> Result<(), crate::Error> {
    if crate::client::valid_uuid(session) && (1..=MAX_DIRECTION_VERSION).contains(&version) {
        Ok(())
    } else {
        Err(crate::Error::InvalidProfile)
    }
}
