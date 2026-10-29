//! State transitions for loop suppression and global download deduplication.
//! Side effects are deliberately separate: acknowledge only after they succeed.
use crate::{hash, Dedupe, DedupeCheck, Filter, Profile};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum UploadDecision {
    Upload(String),
    Suppressed,
    Filtered,
    Paused,
}
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadDecision {
    Download,
    Unchanged,
    Duplicate,
    Unsupported,
    Paused,
    Invalid,
    Capacity,
}

pub struct Engine {
    filter: Filter,
    dedupe: Dedupe,
    enabled: bool,
    last_server_hash: Option<String>,
    ignored_server_hash: Option<String>,
    suppress_clipboard_hash: Option<String>,
}
impl Engine {
    pub fn new(filter: Filter, window: Duration, enabled: bool) -> Self {
        Self {
            filter,
            dedupe: Dedupe::new(window),
            enabled,
            last_server_hash: None,
            ignored_server_hash: None,
            suppress_clipboard_hash: None,
        }
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn reconfigure(&mut self, filter: Filter, window: Duration, enabled: bool) {
        self.filter = filter;
        // A changed filter must reconsider previously filtered server data,
        // while successful uploads and downloads must remain loop-suppressed.
        self.ignored_server_hash = None;
        self.dedupe.set_window(window);
        self.enabled = enabled;
    }
    pub fn filter(&self) -> &Filter {
        &self.filter
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn clipboard_changed(&mut self, text: &str) -> UploadDecision {
        let digest = hash(text);
        // One expected event for a successful remote write; any later, different
        // selection must not be swallowed by a stale marker.
        if let Some(suppressed) = self.suppress_clipboard_hash.take() {
            if suppressed.eq_ignore_ascii_case(&digest) {
                return UploadDecision::Suppressed;
            }
        }
        if !self.enabled {
            return UploadDecision::Paused;
        }
        if !self.filter.allows(text, false) {
            return UploadDecision::Filtered;
        }
        UploadDecision::Upload(digest)
    }
    pub fn manual_upload(&self, text: &str) -> UploadDecision {
        if self.filter.allows(text, false) {
            UploadDecision::Upload(hash(text))
        } else {
            UploadDecision::Filtered
        }
    }
    pub fn ignored_remote(&mut self, digest: &str) {
        self.ignored_server_hash = Some(digest.to_ascii_uppercase());
    }
    pub fn clear_clipboard_suppression(&mut self) {
        self.suppress_clipboard_hash = None;
    }
    pub fn uploaded(&mut self, digest: &str) {
        self.last_server_hash = Some(digest.to_ascii_uppercase());
        self.ignored_server_hash = None;
    }
    pub fn remote_changed(
        &mut self,
        profile: &Profile,
        force: bool,
        now: Instant,
    ) -> DownloadDecision {
        if !self.enabled && !force {
            return DownloadDecision::Paused;
        }
        if profile.kind != "Text" {
            return DownloadDecision::Unsupported;
        }
        if profile.hash.len() != 64 || !profile.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return DownloadDecision::Invalid;
        }
        if !force
            && (self
                .last_server_hash
                .as_ref()
                .is_some_and(|h| h.eq_ignore_ascii_case(&profile.hash))
                || self
                    .ignored_server_hash
                    .as_ref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&profile.hash)))
        {
            return DownloadDecision::Unchanged;
        }
        match self.dedupe.check(&profile.hash, force, now) {
            DedupeCheck::Allowed => DownloadDecision::Download,
            DedupeCheck::Duplicate => DownloadDecision::Duplicate,
            DedupeCheck::Full => DownloadDecision::Capacity,
        }
    }
    /// Call only after validating content and successfully writing the clipboard.
    pub fn downloaded(&mut self, digest: &str, now: Instant) {
        let normalized = digest.to_ascii_uppercase();
        self.suppress_clipboard_hash = Some(normalized.clone());
        self.last_server_hash = Some(normalized.clone());
        self.ignored_server_hash = None;
        self.dedupe.record(&normalized, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{profile, FilterMode};
    fn engine() -> Engine {
        Engine::new(
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
        )
    }
    #[test]
    fn successful_remote_write_suppresses_only_its_own_event() {
        let mut engine = engine();
        let now = Instant::now();
        let digest = hash("remote");
        engine.downloaded(&digest, now);
        assert_eq!(
            engine.clipboard_changed("remote"),
            UploadDecision::Suppressed
        );
        assert_eq!(
            engine.clipboard_changed("local"),
            UploadDecision::Upload(hash("local"))
        );
    }
    #[test]
    fn dedupe_is_global_across_intervening_changes_and_force_bypasses() {
        let mut engine = engine();
        let now = Instant::now();
        let a = profile("a", 32768).unwrap();
        let b = profile("b", 32768).unwrap();
        assert_eq!(
            engine.remote_changed(&a, false, now),
            DownloadDecision::Download
        );
        engine.downloaded(&a.hash, now);
        assert_eq!(
            engine.remote_changed(&b, false, now),
            DownloadDecision::Download
        );
        engine.downloaded(&b.hash, now);
        assert_eq!(
            engine.remote_changed(&a, false, now),
            DownloadDecision::Duplicate
        );
        assert_eq!(
            engine.remote_changed(&a, true, now),
            DownloadDecision::Download
        );
        assert_eq!(
            engine.remote_changed(&a, false, now + Duration::from_secs(300)),
            DownloadDecision::Download
        );
    }
    #[test]
    fn failed_side_effects_do_not_commit_state_and_own_upload_is_not_pulled() {
        let mut engine = engine();
        let p = profile("text", 32768).unwrap();
        let now = Instant::now();
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Download
        );
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Download
        );
        engine.uploaded(&p.hash.to_lowercase());
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Unchanged
        );
        engine.set_enabled(false);
        assert_eq!(engine.clipboard_changed("x"), UploadDecision::Paused);
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Paused
        );
    }
    #[test]
    fn reconfiguration_preserves_download_dedupe() {
        let mut engine = engine();
        let now = Instant::now();
        let a = profile("a", 32768).unwrap();
        let b = profile("b", 32768).unwrap();
        engine.downloaded(&a.hash, now);
        engine.downloaded(&b.hash, now);
        engine.reconfigure(
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
        );
        assert_eq!(
            engine.remote_changed(&a, false, now),
            DownloadDecision::Duplicate
        );
    }
    #[test]
    fn reconfiguration_keeps_own_upload_suppressed() {
        let mut engine = engine();
        let profile = profile("local", 32768).unwrap();
        engine.uploaded(&profile.hash);
        engine.reconfigure(
            Filter::new(FilterMode::Off, &[], false).unwrap(),
            Duration::from_secs(300),
            true,
        );
        assert_eq!(
            engine.remote_changed(&profile, false, Instant::now()),
            DownloadDecision::Unchanged
        );
    }
    #[test]
    fn unsupported_types_do_not_change_state() {
        let mut engine = engine();
        let now = Instant::now();
        let mut p = profile("x", 0).unwrap();
        p.kind = "Image".into();
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Unsupported
        );
        p.kind = "Text".into();
        assert_eq!(
            engine.remote_changed(&p, false, now),
            DownloadDecision::Download
        );
    }
}
