//! Optional local history. Disabled history does not access the filesystem.
use crate::{hash, storage::write_private, MAX_TEXT_BYTES};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Upload,
    Download,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub text: String,
    pub hash: String,
    pub time: u64,
    pub direction: Direction,
}
#[derive(Debug)]
pub enum HistoryError {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid,
    TooLarge,
}
impl From<io::Error> for HistoryError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for HistoryError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub struct History {
    path: PathBuf,
    enabled: bool,
    limit: usize,
}
impl History {
    pub fn new(
        path: impl Into<PathBuf>,
        enabled: bool,
        limit: usize,
    ) -> Result<Self, HistoryError> {
        if !(1..=1000).contains(&limit) {
            return Err(HistoryError::Invalid);
        }
        Ok(Self {
            path: path.into(),
            enabled,
            limit,
        })
    }
    pub fn load(&self) -> Result<Vec<Entry>, HistoryError> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut data = Vec::new();
        file.take(MAX_HISTORY_BYTES as u64 + 1)
            .read_to_end(&mut data)?;
        if data.len() > MAX_HISTORY_BYTES {
            return Err(HistoryError::TooLarge);
        }
        let mut entries: Vec<Entry> = serde_json::from_slice(&data)?;
        if entries.len() > 1000
            || entries.iter().any(|e| {
                e.text.len() > MAX_TEXT_BYTES || !hash(&e.text).eq_ignore_ascii_case(&e.hash)
            })
        {
            return Err(HistoryError::Invalid);
        }
        if entries.len() > self.limit {
            entries.drain(..entries.len() - self.limit);
        }
        Ok(entries)
    }
    pub fn record(
        &self,
        text: &str,
        direction: Direction,
        timestamp: u64,
    ) -> Result<(), HistoryError> {
        if !self.enabled {
            return Ok(());
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(HistoryError::TooLarge);
        }
        let mut entries = self.load()?;
        entries.push(Entry {
            text: text.to_owned(),
            hash: hash(text),
            time: timestamp,
            direction,
        });
        if entries.len() > self.limit {
            entries.drain(..entries.len() - self.limit);
        }
        // Limit total disk and memory use as well as count. Keep the newest entries.
        loop {
            let data = serde_json::to_vec(&entries)?;
            if data.len() <= MAX_HISTORY_BYTES {
                return write_private(&self.path, &data).map_err(Into::into);
            }
            if entries.len() <= 1 {
                return Err(HistoryError::TooLarge);
            }
            entries.remove(0);
        }
    }
    pub fn clear(&self) -> Result<(), HistoryError> {
        if !self.enabled {
            return Ok(());
        }
        write_private(&self.path, b"[]").map_err(Into::into)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn disabled_never_reads_or_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let history = History::new(&path, false, 2).unwrap();
        history.record("secret", Direction::Upload, 1).unwrap();
        history.clear().unwrap();
        assert!(!path.exists());
        assert!(history.load().unwrap().is_empty());
    }
    #[test]
    fn private_atomic_history_limit_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let history = History::new(&path, true, 2).unwrap();
        for (i, text) in ["a", "b", "c"].iter().enumerate() {
            history.record(text, Direction::Download, i as u64).unwrap();
        }
        let entries = history.load().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        history.clear().unwrap();
        assert!(history.load().unwrap().is_empty());
    }
}
