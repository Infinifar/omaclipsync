//! Transport boundary, implemented by the HTTPS client and deterministic tests.
use crate::{
    client::{Client, Registration, RemoteState},
    state::{Ack, Message, Outbox, Persisted},
    Error,
};

pub trait Transport: Send + Sync {
    fn register(&self, session: &Persisted) -> Result<Registration, Error>;
    fn state(&self, session: &Persisted) -> Result<RemoteState, Error>;
    fn changes(&self, session: &Persisted) -> Result<Option<RemoteState>, Error>;
    fn content(&self, session: &Persisted, message: &Message) -> Result<String, Error>;
    fn send(&self, session: &Persisted, outbox: &Outbox) -> Result<u64, Error>;
    fn cancel(&self, session: &Persisted, outbox: &Outbox) -> Result<Option<u64>, Error>;
    fn acknowledge(&self, session: &Persisted, ack: &Ack) -> Result<(), Error>;
}

impl Transport for Client {
    fn register(&self, s: &Persisted) -> Result<Registration, Error> {
        Client::register(self, s)
    }
    fn state(&self, s: &Persisted) -> Result<RemoteState, Error> {
        Client::state(self, s)
    }
    fn changes(&self, s: &Persisted) -> Result<Option<RemoteState>, Error> {
        Client::changes(self, s)
    }
    fn content(&self, s: &Persisted, m: &Message) -> Result<String, Error> {
        Client::content(self, s, m)
    }
    fn send(&self, s: &Persisted, o: &Outbox) -> Result<u64, Error> {
        Client::send(self, s, o)
    }
    fn cancel(&self, s: &Persisted, o: &Outbox) -> Result<Option<u64>, Error> {
        Client::cancel(self, s, o)
    }
    fn acknowledge(&self, s: &Persisted, a: &Ack) -> Result<(), Error> {
        Client::acknowledge(self, s, a)
    }
}
