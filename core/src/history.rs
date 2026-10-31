//! Optional local history. Disabled history does not access the filesystem.
use crate::{hash, storage::write_private, MAX_TEXT_BYTES};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_FAVORITES: usize = 5;

fn is_false(value: &bool) -> bool {
    !value
}
pub use rustclipsync_core::adapters::Direction;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub text: String,
    pub hash: String,
    pub time: u64,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorite: bool,
}
#[derive(Debug)]
pub enum HistoryError {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid,
    TooLarge,
    Disabled,
    NotFound,
    FavoriteLimit,
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
        if entries.len() > 1000 + MAX_FAVORITES
            || entries.iter().filter(|e| e.favorite).count() > MAX_FAVORITES
            || entries.iter().any(|e| {
                e.text.len() > MAX_TEXT_BYTES || !hash(&e.text).eq_ignore_ascii_case(&e.hash)
            })
        {
            return Err(HistoryError::Invalid);
        }
        self.trim(&mut entries);
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
        let digest = hash(text);
        // A new occurrence of a favorite updates it instead of creating a second copy.
        let favorite = entries
            .iter()
            .position(|e| e.favorite && e.hash.eq_ignore_ascii_case(&digest));
        if let Some(index) = favorite {
            entries.remove(index);
        }
        entries.push(Entry {
            text: text.to_owned(),
            hash: digest,
            time: timestamp,
            direction,
            favorite: favorite.is_some(),
        });
        self.save(entries)
    }

    fn trim(&self, entries: &mut Vec<Entry>) {
        let mut ordinary = entries.iter().filter(|e| !e.favorite).count();
        entries.retain(|entry| {
            if !entry.favorite && ordinary > self.limit {
                ordinary -= 1;
                false
            } else {
                true
            }
        });
    }

    fn save(&self, mut entries: Vec<Entry>) -> Result<(), HistoryError> {
        self.trim(&mut entries);
        // Favorites survive automatic count and size eviction.
        loop {
            let data = serde_json::to_vec(&entries)?;
            if data.len() <= MAX_HISTORY_BYTES {
                return write_private(&self.path, &data).map_err(Into::into);
            }
            match entries.iter().position(|e| !e.favorite) {
                Some(index) if entries.len() > 1 => {
                    entries.remove(index);
                }
                _ => return Err(HistoryError::TooLarge),
            }
        }
    }

    pub fn set_favorite(&self, digest: &str, favorite: bool) -> Result<(), HistoryError> {
        if !self.enabled {
            return Err(HistoryError::Disabled);
        }
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HistoryError::NotFound);
        }
        let mut entries = self.load()?;
        let index = entries
            .iter()
            .rposition(|e| e.hash.eq_ignore_ascii_case(digest))
            .ok_or(HistoryError::NotFound)?;
        if favorite
            && !entries[index].favorite
            && entries.iter().filter(|e| e.favorite).count() >= MAX_FAVORITES
        {
            return Err(HistoryError::FavoriteLimit);
        }
        let mut selected = entries.remove(index);
        selected.favorite = favorite;
        // Identical selections represent one favorite and remain unambiguous by hash.
        entries.retain(|e| !e.hash.eq_ignore_ascii_case(digest));
        entries.push(selected);
        self.save(entries)
    }
    pub fn clear(&self) -> Result<(), HistoryError> {
        if !self.enabled {
            return Ok(());
        }
        let favorites = self.load()?.into_iter().filter(|e| e.favorite).collect();
        self.save(favorites)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl rustclipsync_core::adapters::HistorySink for History {
    type Error = HistoryError;
    fn record(&self, text: &str, direction: Direction, timestamp: u64) -> Result<(), Self::Error> {
        History::record(self, text, direction, timestamp)
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
        assert!(matches!(
            history.set_favorite(&hash("secret"), true),
            Err(HistoryError::Disabled)
        ));
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

    #[test]
    fn favorites_survive_eviction_clear_and_restart_with_a_five_item_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let history = History::new(&path, true, 2).unwrap();
        // Existing pre-favorites JSON remains readable.
        fs::write(
            &path,
            serde_json::json!([{
                "text": "old", "hash": hash("old"), "time": 0, "direction": "upload"
            }])
            .to_string(),
        )
        .unwrap();
        assert!(!history.load().unwrap()[0].favorite);
        history.set_favorite(&hash("old"), true).unwrap();
        for i in 1..5 {
            let text = format!("favorite-{i}");
            history.record(&text, Direction::Download, i).unwrap();
            history.set_favorite(&hash(&text), true).unwrap();
        }
        history.record("sixth", Direction::Upload, 10).unwrap();
        assert!(matches!(
            history.set_favorite(&hash("sixth"), true),
            Err(HistoryError::FavoriteLimit)
        ));
        history.record("newer", Direction::Upload, 11).unwrap();
        history.record("newest", Direction::Upload, 12).unwrap();
        let entries = history.load().unwrap();
        assert_eq!(entries.iter().filter(|e| e.favorite).count(), 5);
        assert_eq!(
            entries
                .iter()
                .filter(|e| !e.favorite)
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            ["newer", "newest"]
        );
        // Recording a favorite again does not consume a new slot or lose the star.
        history.record("old", Direction::Download, 13).unwrap();
        assert_eq!(
            history
                .load()
                .unwrap()
                .iter()
                .filter(|e| e.favorite)
                .count(),
            5
        );
        assert_eq!(
            history
                .load()
                .unwrap()
                .iter()
                .filter(|e| e.text == "old")
                .count(),
            1
        );
        history.clear().unwrap();
        let restored = History::new(&path, true, 1).unwrap().load().unwrap();
        assert_eq!(restored.len(), 5);
        assert!(restored.iter().all(|e| e.favorite));
        history.set_favorite(&hash("old"), false).unwrap();
        history.set_favorite(&hash("absent"), true).unwrap_err();
        history.clear().unwrap();
        assert_eq!(history.load().unwrap().len(), 4);
    }
}
