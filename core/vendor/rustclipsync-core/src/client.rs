//! HTTPS transport for RustClipSync. It never follows redirects.
use std::{io::Read, time::Duration};

use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use url::Url;

use crate::{
    state::{Ack, Message, Outbox, Persisted},
    Error, MAX_TEXT_BYTES,
};

const MAX_META: u64 = 16 * 1024;

pub struct Client {
    base: Url,
    user: String,
    password: String,
    http: Http,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub epoch: String,
    pub ack_revision: u64,
    pub current_revision: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
    pub epoch: String,
    pub revision: u64,
    pub ack_revision: u64,
    pub message_id: Option<String>,
    pub source_device_id: Option<String>,
    pub size: Option<u64>,
    pub sensitive: Option<bool>,
}

impl RemoteState {
    pub fn message(&self) -> Option<Message> {
        Some(Message {
            id: self.message_id.clone()?,
            revision: self.revision,
            source_device_id: self.source_device_id.clone()?,
            sensitive: self.sensitive?,
        })
    }
    fn validate(self) -> Result<Self, Error> {
        if !valid_uuid(&self.epoch)
            || self.ack_revision > self.revision
            || (self.revision == 0
                && (self.message_id.is_some()
                    || self.source_device_id.is_some()
                    || self.size.is_some()
                    || self.sensitive.is_some()))
            || (self.revision != 0
                && (!self.message_id.as_deref().is_some_and(valid_uuid)
                    || !self.source_device_id.as_deref().is_some_and(valid_uuid)
                    || !self.size.is_some_and(|n| n <= MAX_TEXT_BYTES as u64)
                    || self.sensitive.is_none()))
        {
            return Err(Error::InvalidProfile);
        }
        Ok(self)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    epoch: String,
    revision: u64,
    message_id: String,
}

#[derive(Deserialize)]
struct Cancellation {
    state: String,
    revision: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Acknowledged {
    ack_revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Register<'a> {
    device_id: &'a str,
    device_token: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Acknowledge<'a> {
    epoch: &'a str,
    revision: u64,
    message_id: &'a str,
    result: &'a str,
}

impl Client {
    pub fn new(base: &str, user: String, password: String, insecure: bool) -> Result<Self, Error> {
        let mut base = Url::parse(base).map_err(|_| Error::InvalidUrl)?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = Http::builder()
            .user_agent(concat!("rustclipsync-core/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            base,
            user,
            password,
            http,
        })
    }

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base.join(path).map_err(|_| Error::InvalidUrl)
    }
    fn bearer(&self, request: RequestBuilder, session: &Persisted) -> RequestBuilder {
        request.bearer_auth(format!("{}:{}", session.state.device_id, session.token))
    }
    fn perform(request: RequestBuilder) -> Result<Response, Error> {
        let response = request.send()?;
        if !response.status().is_success() {
            return Err(Error::Status(response.status().as_u16()));
        }
        Ok(response)
    }
    fn bytes(response: Response, limit: u64) -> Result<Vec<u8>, Error> {
        if response.content_length().is_some_and(|n| n > limit) {
            return Err(Error::TooLarge);
        }
        let mut bytes = Vec::new();
        response.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(Error::TooLarge);
        }
        Ok(bytes)
    }
    fn json<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
        Ok(serde_json::from_slice(&Self::bytes(response, MAX_META)?)?)
    }

    pub fn register(&self, session: &Persisted) -> Result<Registration, Error> {
        if !valid_uuid(&session.state.device_id) || !valid_token(&session.token) {
            return Err(Error::InvalidProfile);
        }
        let request = self
            .http
            .post(self.url("api/devices")?)
            .basic_auth(&self.user, Some(&self.password))
            .json(&Register {
                device_id: &session.state.device_id,
                device_token: &session.token,
            });
        let response: Registration = Self::json(Self::perform(request)?)?;
        if !valid_uuid(&response.epoch) || response.ack_revision > response.current_revision {
            return Err(Error::InvalidProfile);
        }
        Ok(response)
    }

    pub fn state(&self, session: &Persisted) -> Result<RemoteState, Error> {
        let request = self.bearer(self.http.get(self.url("api/state")?), session);
        Self::json::<RemoteState>(Self::perform(request)?)?.validate()
    }

    pub fn changes(&self, session: &Persisted) -> Result<Option<RemoteState>, Error> {
        if !valid_uuid(&session.state.epoch) {
            return Err(Error::InvalidProfile);
        }
        let request = self
            .bearer(self.http.get(self.url("api/changes")?), session)
            .query(&[
                ("epoch", session.state.epoch.as_str()),
                ("after", &session.state.cursor.to_string()),
            ])
            .timeout(Duration::from_secs(30));
        let response = Self::perform(request)?;
        if response.status().as_u16() == 204 {
            return Ok(None);
        }
        Ok(Some(Self::json::<RemoteState>(response)?.validate()?))
    }

    pub fn content(&self, session: &Persisted, message: &Message) -> Result<String, Error> {
        if !valid_uuid(&message.id) {
            return Err(Error::InvalidProfile);
        }
        let request = self.bearer(
            self.http
                .get(self.url(&format!("api/messages/{}/content", message.id))?),
            session,
        );
        String::from_utf8(Self::bytes(Self::perform(request)?, MAX_TEXT_BYTES as u64)?)
            .map_err(|_| Error::InvalidProfile)
    }

    pub fn send(&self, session: &Persisted, outbox: &Outbox) -> Result<u64, Error> {
        if !valid_uuid(&outbox.copy.id) || !valid_uuid(&session.state.epoch) {
            return Err(Error::InvalidProfile);
        }
        if outbox.copy.text.len() > MAX_TEXT_BYTES {
            return Err(Error::TooLarge);
        }
        let request = self
            .bearer(
                self.http
                    .put(self.url(&format!("api/messages/{}", outbox.copy.id))?),
                session,
            )
            .header("x-rcs-epoch", &session.state.epoch)
            .header("x-rcs-base-revision", outbox.base_revision)
            .header(
                "x-rcs-sensitive",
                if outbox.copy.sensitive { "1" } else { "0" },
            )
            .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(outbox.copy.text.clone());
        let response: Commit = Self::json(Self::perform(request)?)?;
        if response.epoch != session.state.epoch
            || response.message_id != outbox.copy.id
            || response.revision <= outbox.base_revision
        {
            return Err(Error::InvalidProfile);
        }
        Ok(response.revision)
    }

    /// None means cancelled; Some is the revision already committed.
    pub fn cancel(&self, session: &Persisted, outbox: &Outbox) -> Result<Option<u64>, Error> {
        if !valid_uuid(&outbox.copy.id) || !valid_uuid(&session.state.epoch) {
            return Err(Error::InvalidProfile);
        }
        let request = self
            .bearer(
                self.http
                    .delete(self.url(&format!("api/messages/{}", outbox.copy.id))?),
                session,
            )
            .header("x-rcs-epoch", &session.state.epoch);
        let response: Cancellation = Self::json(Self::perform(request)?)?;
        match (response.state.as_str(), response.revision) {
            ("cancelled", None) => Ok(None),
            ("committed", Some(revision)) if revision > outbox.base_revision => Ok(Some(revision)),
            _ => Err(Error::InvalidProfile),
        }
    }

    pub fn acknowledge(&self, session: &Persisted, ack: &Ack) -> Result<(), Error> {
        if !valid_uuid(&ack.id)
            || !valid_uuid(&session.state.epoch)
            || !matches!(ack.result.as_str(), "self" | "skipped" | "applied")
        {
            return Err(Error::InvalidProfile);
        }
        let request = self
            .bearer(self.http.post(self.url("api/ack")?), session)
            .json(&Acknowledge {
                epoch: &session.state.epoch,
                revision: ack.revision,
                message_id: &ack.id,
                result: &ack.result,
            });
        let response: Acknowledged = Self::json(Self::perform(request)?)?;
        if response.ack_revision < ack.revision {
            return Err(Error::InvalidProfile);
        }
        Ok(())
    }
}

fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            14 => c == b'4',
            19 => matches!(c, b'8' | b'9' | b'a' | b'b'),
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
}
fn valid_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_canonical_v4_ids_and_safe_urls() {
        assert!(valid_uuid(&crate::state::random_uuid().unwrap()));
        assert!(!valid_uuid("00000000-0000-1000-8000-000000000000"));
        assert!(Client::new("https://example.test/base", "a".into(), "b".into(), false).is_ok());
        for url in [
            "http://example.test",
            "https://user@example.test",
            "https://example.test/?x=1",
        ] {
            assert!(Client::new(url, "a".into(), "b".into(), false).is_err());
        }
    }
}
