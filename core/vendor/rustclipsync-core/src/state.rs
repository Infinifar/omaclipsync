//! Durable latest-value state for RustClipSync.
//!
//! Persist each returned transition before its associated external side effect.
//! Ciphertext contains clipboard text and device credentials; callers keep the
//! encryption key protected by the platform credential store and never print it.
use std::io;

use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};

use crate::MAX_TEXT_BYTES;

const MAGIC: &[u8; 4] = b"RCS1";
pub const MAX_STATE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalCopy {
    pub id: String,
    #[serde(with = "base64_text")]
    pub text: String,
    pub created_at: u64,
    #[serde(default)]
    pub sensitive: bool,
}

impl std::fmt::Debug for LocalCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalCopy")
            .field("id", &self.id)
            .field("text", &"<redacted>")
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Outbox {
    pub copy: LocalCopy,
    pub base_revision: u64,
    pub attempted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub revision: u64,
    pub source_device_id: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Ack {
    pub id: String,
    pub revision: u64,
    pub result: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub epoch: String,
    pub device_id: String,
    pub cursor: u64,
    pub pending: Option<Outbox>,
    pub queued: Option<LocalCopy>,
    pub discard_pending: bool,
    pub applying: Option<Message>,
    pub ack: Option<Ack>,
}

impl State {
    pub fn new(epoch: String, device_id: String, cursor: u64) -> Self {
        Self {
            epoch,
            device_id,
            cursor,
            pending: None,
            queued: None,
            discard_pending: false,
            applying: None,
            ack: None,
        }
    }

    pub fn local_copy(&mut self, value: LocalCopy) {
        match self.pending.as_mut() {
            None => {
                self.pending = Some(Outbox {
                    copy: value,
                    base_revision: self.cursor,
                    attempted: false,
                })
            }
            Some(p) if !p.attempted => p.copy = value,
            Some(_) => {
                self.queued = Some(value);
                self.discard_pending = false;
            }
        }
    }

    pub fn local_clear(&mut self) {
        match &self.pending {
            Some(p) if p.attempted => {
                self.queued = None;
                self.discard_pending = true;
            }
            Some(_) => {
                self.pending = None;
                self.queued = None;
                self.discard_pending = false;
            }
            None => {}
        }
    }

    pub fn attempt(&mut self) -> Result<Outbox, &'static str> {
        if self.queued.is_some() || self.discard_pending {
            return Err("cancel required");
        }
        let pending = self.pending.as_mut().ok_or("no pending copy")?;
        pending.attempted = true;
        Ok(pending.clone())
    }

    pub fn committed(&mut self, id: &str, revision: u64) -> Result<(), &'static str> {
        let old = self.pending.as_ref().ok_or("no pending copy")?;
        if old.copy.id != id || revision <= old.base_revision {
            return Err("invalid commit");
        }
        self.pending = self.queued.take().map(|copy| Outbox {
            copy,
            base_revision: revision,
            attempted: false,
        });
        self.discard_pending = false;
        self.cursor = self.cursor.max(revision);
        self.ack = Some(Ack {
            id: id.to_owned(),
            revision,
            result: "self".into(),
        });
        Ok(())
    }

    pub fn cancelled(&mut self, id: &str) -> Result<(), &'static str> {
        let old = self.pending.as_ref().ok_or("no pending copy")?;
        if old.copy.id != id || !old.attempted || (self.queued.is_none() && !self.discard_pending) {
            return Err("invalid cancellation");
        }
        let base = old.base_revision;
        self.pending = self.queued.take().map(|copy| Outbox {
            copy,
            base_revision: base,
            attempted: false,
        });
        self.discard_pending = false;
        Ok(())
    }

    pub fn prepare_apply(&mut self, message: Message) -> Result<(), &'static str> {
        if self.pending.is_some()
            || self.applying.is_some()
            || message.revision <= self.cursor
            || message.source_device_id == self.device_id
        {
            return Err("cannot apply");
        }
        self.applying = Some(message);
        Ok(())
    }

    pub fn acknowledge_self(&mut self, message: Message) -> Result<(), &'static str> {
        if self.pending.is_some()
            || self.applying.is_some()
            || message.revision <= self.cursor
            || message.source_device_id != self.device_id
        {
            return Err("cannot acknowledge own message");
        }
        self.cursor = message.revision;
        self.ack = Some(Ack {
            id: message.id,
            revision: message.revision,
            result: "self".into(),
        });
        Ok(())
    }

    pub fn finish_apply(&mut self, id: &str, success: bool) -> Result<(), &'static str> {
        let message = self.applying.as_ref().ok_or("no application pending")?;
        if message.id != id {
            return Err("wrong application");
        }
        if success {
            self.cursor = message.revision;
            self.ack = Some(Ack {
                id: message.id.clone(),
                revision: message.revision,
                result: "applied".into(),
            });
        }
        self.applying = None;
        Ok(())
    }

    pub fn recover_ambiguous_apply(&mut self) {
        if let Some(message) = self.applying.take() {
            self.cursor = self.cursor.max(message.revision);
            self.ack = Some(Ack {
                id: message.id,
                revision: message.revision,
                result: "skipped".into(),
            });
        }
    }
}

/// A deliberately un-applied incoming item, committed with its receive cursor.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,
    #[serde(with = "base64_text")]
    pub text: String,
    pub received_at: u64,
}
impl std::fmt::Debug for InboxItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxItem")
            .field("id", &self.id)
            .field("text", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Persisted {
    pub server: String,
    pub token: String,
    pub paired: bool,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<InboxItem>,
}

impl Persisted {
    /// Long polling only needs device identity and cursor, never clipboard text.
    pub fn notification_identity(&self) -> Self {
        Self {
            inbox: None,
            server: self.server.clone(),
            token: self.token.clone(),
            paired: self.paired,
            state: State::new(
                self.state.epoch.clone(),
                self.state.device_id.clone(),
                self.state.cursor,
            ),
        }
    }
}

impl std::fmt::Debug for Persisted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Persisted")
            .field("server", &self.server)
            .field("token", &"<redacted>")
            .field("paired", &self.paired)
            .field("state", &self.state)
            .finish()
    }
}

fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut out = [0; N];
    SystemRandom::new()
        .fill(&mut out)
        .map_err(|_| io::Error::other("secure random unavailable"))?;
    Ok(out)
}

pub fn random_key() -> io::Result<[u8; 32]> {
    random()
}

pub fn random_token() -> io::Result<String> {
    Ok(random::<32>()?.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn random_uuid() -> io::Result<String> {
    let mut b = random::<16>()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Encrypt in the existing RCS1 format (including its trailing newline).
pub fn seal(key: &[u8; 32], state: &Persisted) -> io::Result<Vec<u8>> {
    let mut clear = serde_json::to_vec(state).map_err(io::Error::other)?;
    if clear.len() as u64 > MAX_STATE - 64 {
        return Err(io::Error::other("state too large"));
    }
    let nonce = random::<12>()?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, key)
            .map_err(|_| io::Error::other("invalid key"))?,
    );
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(MAGIC),
        &mut clear,
    )
    .map_err(|_| io::Error::other("state encryption failed"))?;
    let mut file = Vec::with_capacity(MAGIC.len() + nonce.len() + clear.len());
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&nonce);
    file.extend_from_slice(&clear);
    file.push(b'\n');
    Ok(file)
}

/// Authenticate before decoding. Adapters must also bound reads to MAX_STATE.
pub fn open(key: &[u8; 32], mut bytes: Vec<u8>) -> io::Result<Persisted> {
    // write_private appends exactly one newline after the encrypted payload.
    if bytes.len() as u64 > MAX_STATE
        || bytes.len() < 4 + 12 + 16 + 1
        || bytes.pop() != Some(b'\n')
        || &bytes[..4] != MAGIC
    {
        return Err(io::Error::other("invalid encrypted state"));
    }
    let nonce: [u8; 12] = bytes[4..16].try_into().map_err(io::Error::other)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, key)
            .map_err(|_| io::Error::other("invalid key"))?,
    );
    let clear = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(MAGIC),
            &mut bytes[16..],
        )
        .map_err(|_| io::Error::other("state authentication failed"))?;
    serde_json::from_slice(clear).map_err(io::Error::other)
}

mod base64_text {
    use super::*;
    use serde::{Deserializer, Serializer};
    pub fn serialize<S: Serializer>(text: &str, serializer: S) -> Result<S::Ok, S::Error> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(serde::ser::Error::custom("text too large"));
        }
        serializer.serialize_str(&STANDARD.encode(text.as_bytes()))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw.len() > MAX_TEXT_BYTES.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("text too large"));
        }
        let bytes = STANDARD.decode(raw).map_err(serde::de::Error::custom)?;
        if bytes.len() > MAX_TEXT_BYTES {
            return Err(serde::de::Error::custom("text too large"));
        }
        String::from_utf8(bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(id: &str, text: &str) -> LocalCopy {
        LocalCopy {
            id: id.into(),
            text: text.into(),
            created_at: 1,
            sensitive: false,
        }
    }
    fn state() -> State {
        State::new("epoch".into(), "device".into(), 4)
    }

    #[test]
    fn latest_and_uncertain_uploads_are_distinct() {
        let mut s = state();
        s.local_copy(copy("a", "same"));
        s.local_copy(copy("b", "same"));
        assert_eq!(s.pending.as_ref().unwrap().copy.id, "b");
        s.attempt().unwrap();
        s.local_copy(copy("c", "middle"));
        s.local_copy(copy("d", "last"));
        assert!(s.attempt().is_err());
        let mut cancelled = s.clone();
        cancelled.cancelled("b").unwrap();
        assert_eq!(cancelled.pending.unwrap().base_revision, 4);
        s.committed("b", 5).unwrap();
        assert_eq!(s.pending.as_ref().unwrap().copy.id, "d");
        assert_eq!(s.pending.unwrap().base_revision, 5);
    }

    #[test]
    fn clear_and_ambiguous_apply_do_not_replay_old_values() {
        let mut s = state();
        s.local_copy(copy("a", "text"));
        s.attempt().unwrap();
        s.local_clear();
        s.cancelled("a").unwrap();
        assert!(s.pending.is_none());
        s.prepare_apply(Message {
            id: "b".into(),
            revision: 5,
            source_device_id: "other".into(),
            sensitive: false,
        })
        .unwrap();
        s.recover_ambiguous_apply();
        assert_eq!(s.cursor, 5);
        assert_eq!(s.ack.unwrap().result, "skipped");
    }

    #[test]
    fn encrypted_state_is_authenticated_and_compatible() {
        let key = random_key().unwrap();
        let mut s = state();
        s.local_copy(copy("a", "private clipboard text"));
        let saved = Persisted {
            server: "https://example.test".into(),
            token: random_token().unwrap(),
            paired: true,
            state: s,
            inbox: None,
        };
        let raw = seal(&key, &saved).unwrap();
        assert_eq!(&raw[..4], b"RCS1");
        assert_eq!(raw.last(), Some(&b'\n'));
        assert!(!raw
            .windows("private clipboard text".len())
            .any(|x| x == b"private clipboard text"));
        assert_eq!(open(&key, raw.clone()).unwrap(), saved);
        assert!(open(&random_key().unwrap(), raw.clone()).is_err());
        let mut corrupted = raw;
        corrupted[20] ^= 1;
        assert!(open(&key, corrupted).is_err());
    }
}
