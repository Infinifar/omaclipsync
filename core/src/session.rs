//! Coordinates protocol, clipboard and loop suppression; network calls are serial.
use crate::{
    engine::{DownloadDecision, Engine, UploadDecision},
    history::{Direction, History},
    wayland, Error, Filter, Profile, Server,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub trait Remote {
    fn current(&self) -> Result<Profile, Error>;
    fn push(&self, text: &str, max_inline: usize, filter: &Filter) -> Result<bool, Error>;
    fn pull(&self, profile: &Profile, filter: &Filter) -> Result<String, Error>;
}
impl Remote for Server {
    fn current(&self) -> Result<Profile, Error> {
        Server::current(self)
    }
    fn push(&self, text: &str, max_inline: usize, filter: &Filter) -> Result<bool, Error> {
        Server::push(self, text, max_inline, filter)
    }
    fn pull(&self, profile: &Profile, filter: &Filter) -> Result<String, Error> {
        Server::pull(self, profile, filter)
    }
}
pub trait Clipboard {
    fn read(&self, allow_sensitive: bool) -> Result<String, wayland::ClipboardError>;
    fn write(&self, text: &str) -> Result<(), wayland::ClipboardError>;
}
pub struct WaylandClipboard;
impl Clipboard for WaylandClipboard {
    fn read(&self, allow_sensitive: bool) -> Result<String, wayland::ClipboardError> {
        wayland::read_current(allow_sensitive)
    }
    fn write(&self, text: &str) -> Result<(), wayland::ClipboardError> {
        wayland::write_text(text)
    }
}
#[derive(Debug)]
pub enum SyncError {
    Remote(Error),
    Clipboard(wayland::ClipboardError),
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

pub struct Session<R, C> {
    pub engine: Engine,
    pub remote: R,
    pub clipboard: C,
    pub max_inline: usize,
    pub history: History,
    pub history_error: bool,
}
impl<R: Remote, C: Clipboard> Session<R, C> {
    pub fn new(
        remote: R,
        clipboard: C,
        filter: Filter,
        window: Duration,
        enabled: bool,
        max_inline: usize,
        history: History,
    ) -> Self {
        Self {
            engine: Engine::new(filter, window, enabled),
            remote,
            clipboard,
            max_inline,
            history,
            history_error: false,
        }
    }
    pub fn local_changed(&mut self, text: &str) -> Result<Outcome, SyncError> {
        let decision = self.engine.clipboard_changed(text);
        self.upload_decision(text, decision)
    }
    pub fn push_now(&mut self, allow_sensitive: bool) -> Result<Outcome, SyncError> {
        let text = self
            .clipboard
            .read(allow_sensitive)
            .map_err(SyncError::Clipboard)?;
        let decision = self.engine.manual_upload(&text);
        self.upload_decision(&text, decision)
    }
    fn upload_decision(
        &mut self,
        text: &str,
        decision: UploadDecision,
    ) -> Result<Outcome, SyncError> {
        match decision {
            UploadDecision::Upload(digest) => {
                let changed = self
                    .remote
                    .push(text, self.max_inline, self.engine.filter())
                    .map_err(SyncError::Remote)?;
                self.engine.uploaded(&digest);
                if changed {
                    self.record_history(text, Direction::Upload);
                    Ok(Outcome::Uploaded)
                } else {
                    Ok(Outcome::Current)
                }
            }
            UploadDecision::Suppressed => Ok(Outcome::Skipped),
            UploadDecision::Filtered => Ok(Outcome::Filtered),
            UploadDecision::Paused => Ok(Outcome::Paused),
        }
    }
    pub fn pull(&mut self, force: bool, now: Instant) -> Result<Outcome, SyncError> {
        let profile = self.remote.current().map_err(SyncError::Remote)?;
        match self.engine.remote_changed(&profile, force, now) {
            DownloadDecision::Download => {
                let text = match self.remote.pull(&profile, self.engine.filter()) {
                    Ok(text) => text,
                    Err(Error::Filtered) => {
                        self.engine.ignored_remote(&profile.hash);
                        return Ok(Outcome::Filtered);
                    }
                    Err(err) => return Err(SyncError::Remote(err)),
                };
                self.clipboard.write(&text).map_err(SyncError::Clipboard)?;
                self.engine.downloaded(&profile.hash, now);
                self.record_history(&text, Direction::Download);
                Ok(Outcome::Downloaded)
            }
            DownloadDecision::Unchanged => Ok(Outcome::Current),
            DownloadDecision::Duplicate => Ok(Outcome::Skipped),
            DownloadDecision::Unsupported => Ok(Outcome::Unsupported),
            DownloadDecision::Paused => Ok(Outcome::Paused),
            DownloadDecision::Invalid => Ok(Outcome::Invalid),
            DownloadDecision::Capacity => Ok(Outcome::Capacity),
        }
    }
    fn record_history(&mut self, text: &str, direction: Direction) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // A history write failure must not roll back a completed sync operation.
        self.history_error = self.history.record(text, direction, now).is_err();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hash, profile, FilterMode};
    use std::{cell::RefCell, rc::Rc};
    #[derive(Default)]
    struct MockState {
        text: String,
        writes: usize,
        pushes: usize,
        fail_write: bool,
        fail_push: bool,
        clipboard_sensitive: bool,
    }
    struct MockRemote(Rc<RefCell<MockState>>);
    impl Remote for MockRemote {
        fn current(&self) -> Result<Profile, Error> {
            Ok(profile(&self.0.borrow().text, 32768).unwrap())
        }
        fn push(&self, text: &str, _max: usize, filter: &Filter) -> Result<bool, Error> {
            if !filter.allows(text, false) {
                return Err(Error::Filtered);
            }
            let mut s = self.0.borrow_mut();
            if s.fail_push {
                return Err(Error::Status(503));
            }
            if s.text == text {
                return Ok(false);
            }
            s.text = text.into();
            s.pushes += 1;
            Ok(true)
        }
        fn pull(&self, _: &Profile, filter: &Filter) -> Result<String, Error> {
            let text = self.0.borrow().text.clone();
            if filter.allows(&text, true) {
                Ok(text)
            } else {
                Err(Error::Filtered)
            }
        }
    }
    struct MockClipboard(Rc<RefCell<MockState>>);
    impl Clipboard for MockClipboard {
        fn read(&self, allow_sensitive: bool) -> Result<String, wayland::ClipboardError> {
            if self.0.borrow().clipboard_sensitive && !allow_sensitive {
                return Err(wayland::ClipboardError::Sensitive);
            }
            Ok("local".into())
        }
        fn write(&self, _text: &str) -> Result<(), wayland::ClipboardError> {
            let mut s = self.0.borrow_mut();
            if s.fail_write {
                return Err(wayland::ClipboardError::Io(std::io::Error::other(
                    "failure",
                )));
            }
            s.writes += 1;
            Ok(())
        }
    }
    #[test]
    fn manual_push_obeys_sensitive_policy() {
        let dir = tempfile::tempdir().unwrap();
        let state = Rc::new(RefCell::new(MockState {
            clipboard_sensitive: true,
            ..Default::default()
        }));
        let mut session = Session::new(
            MockRemote(state.clone()),
            MockClipboard(state.clone()),
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            History::new(dir.path().join("history"), false, 50).unwrap(),
        );
        assert!(matches!(
            session.push_now(false),
            Err(SyncError::Clipboard(wayland::ClipboardError::Sensitive))
        ));
        assert_eq!(state.borrow().pushes, 0);
        assert_eq!(session.push_now(true).unwrap(), Outcome::Uploaded);
        assert_eq!(state.borrow().pushes, 1);
    }
    #[test]
    fn local_remote_loop_and_failed_write_retry() {
        let dir = tempfile::tempdir().unwrap();
        let state = Rc::new(RefCell::new(MockState::default()));
        let mut session = Session::new(
            MockRemote(state.clone()),
            MockClipboard(state.clone()),
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            History::new(dir.path().join("history"), false, 50).unwrap(),
        );
        assert_eq!(session.local_changed("local").unwrap(), Outcome::Uploaded);
        assert_eq!(state.borrow().pushes, 1);
        let now = Instant::now();
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Current);
        state.borrow_mut().text = "remote".into();
        state.borrow_mut().fail_write = true;
        assert!(session.pull(false, now).is_err());
        state.borrow_mut().fail_write = false;
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Downloaded);
        assert_eq!(state.borrow().writes, 1);
        assert_eq!(session.local_changed("remote").unwrap(), Outcome::Skipped);
        assert_eq!(state.borrow().pushes, 1);
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Current);
        assert_eq!(hash(&state.borrow().text), hash("remote"));
    }
    #[test]
    fn filtered_remote_is_not_downloaded_again_until_filter_changes() {
        let dir = tempfile::tempdir().unwrap();
        let state = Rc::new(RefCell::new(MockState {
            text: "secret".into(),
            ..Default::default()
        }));
        let filter = Filter::new(FilterMode::Blacklist, &["secret".into()], true).unwrap();
        let mut session = Session::new(
            MockRemote(state.clone()),
            MockClipboard(state.clone()),
            filter,
            Duration::from_secs(300),
            true,
            32768,
            History::new(dir.path().join("history"), false, 50).unwrap(),
        );
        let now = Instant::now();
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Filtered);
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Current);
        session.engine.reconfigure(
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
        );
        assert_eq!(session.pull(false, now).unwrap(), Outcome::Downloaded);
    }
    #[test]
    fn failed_upload_not_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let state = Rc::new(RefCell::new(MockState {
            fail_push: true,
            ..Default::default()
        }));
        let mut session = Session::new(
            MockRemote(state.clone()),
            MockClipboard(state.clone()),
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
            32768,
            History::new(dir.path().join("history"), false, 50).unwrap(),
        );
        assert!(session.local_changed("local").is_err());
        state.borrow_mut().fail_push = false;
        assert_eq!(session.local_changed("local").unwrap(), Outcome::Uploaded);
    }
}
