//! Structured logs accept fixed event codes only; no clipboard text or secrets.
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Copy, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Started,
    Connected,
    Offline,
    AuthFailed,
    Unsupported,
    Uploaded,
    Downloaded,
    Filtered,
    HistoryError,
    ClipboardError,
    ConfigError,
    WorkerCrashed,
    Shutdown,
    Polled,
}
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}
impl Level {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }
}
impl Event {
    fn level(self) -> Level {
        match self {
            Self::ConfigError | Self::WorkerCrashed => Level::Error,
            Self::Offline | Self::AuthFailed | Self::HistoryError | Self::ClipboardError => {
                Level::Warn
            }
            Self::Polled => Level::Debug,
            _ => Level::Info,
        }
    }
}
#[derive(Serialize)]
struct Record {
    timestamp: String,
    event: Event,
}
pub struct Logger {
    dir: PathBuf,
    level: Level,
}
impl Logger {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            level: Level::Info,
        }
    }
    pub fn set_level(&mut self, level: Level) {
        self.level = level;
    }
    pub fn write(&self, event: Event, now: DateTime<Utc>) -> io::Result<()> {
        if event.level() > self.level {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        let path = self
            .dir
            .join(format!("omaclipsync-{}.jsonl", now.format("%Y-%m-%d")));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let record = Record {
            timestamp: now.to_rfc3339(),
            event,
        };
        serde_json::to_writer(&mut file, &record).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        self.rotate(now.date_naive())
    }
    fn rotate(&self, today: NaiveDate) -> io::Result<()> {
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(date) = parse_log_date(name) else {
                continue;
            };
            if today.signed_duration_since(date).num_days() >= 7 {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.dir
    }
}
fn parse_log_date(name: &str) -> Option<NaiveDate> {
    let date = name.strip_prefix("omaclipsync-")?.strip_suffix(".jsonl")?;
    if date.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[test]
    fn log_levels_skip_low_priority_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = Logger::new(dir.path());
        logger.set_level(Level::Error);
        let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
        logger.write(Event::Connected, now).unwrap();
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
        logger.write(Event::ConfigError, now).unwrap();
        assert!(fs::read_dir(dir.path()).unwrap().next().is_some());
    }
    #[test]
    fn rotates_only_matching_old_files_and_writes_private_json() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "omaclipsync-2026-10-01.jsonl",
            "omaclipsync-2026-10-04.jsonl",
            "other-2026-10-01.jsonl",
        ] {
            fs::write(dir.path().join(name), b"test").unwrap();
        }
        let logger = Logger::new(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
        logger.write(Event::Connected, now).unwrap();
        assert!(!dir.path().join("omaclipsync-2026-10-01.jsonl").exists());
        assert!(dir.path().join("omaclipsync-2026-10-04.jsonl").exists());
        assert!(dir.path().join("other-2026-10-01.jsonl").exists());
        let path = dir.path().join("omaclipsync-2026-10-10.jsonl");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["event"], "connected");
        assert_eq!(json.as_object().unwrap().len(), 2);
    }
}
