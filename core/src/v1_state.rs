//! Linux private-file adapter. The shared crate owns state and encryption.
use rustclipsync_core::adapters::StateStore;
pub use rustclipsync_core::state::*;
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub struct FileStateStore(pub PathBuf);
impl StateStore for FileStateStore {
    fn load(&self) -> io::Result<Option<Vec<u8>>> {
        let file = match File::open(&self.0) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(MAX_STATE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_STATE {
            return Err(io::Error::other("state too large"));
        }
        Ok(Some(bytes))
    }
    fn save(&self, bytes: &[u8]) -> io::Result<()> {
        crate::storage::write_private_bytes(&self.0, bytes)?;
        // The shared StateStore contract requires the rename itself to survive
        // a power loss, as well as the file contents synced before the rename.
        if let Some(parent) = self.0.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

pub fn save(path: &Path, key: &[u8; 32], state: &Persisted) -> io::Result<()> {
    FileStateStore(path.to_owned()).save(&seal(key, state)?)
}
pub fn load(path: &Path, key: &[u8; 32]) -> io::Result<Persisted> {
    let bytes = FileStateStore(path.to_owned())
        .load()?
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    open(key, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn state_replacement_preserves_private_permissions_and_exact_codec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let key = random_key().unwrap();
        let mut value = Persisted {
            server: "https://example.test".into(),
            token: random_token().unwrap(),
            paired: false,
            state: State::new(String::new(), random_uuid().unwrap(), 0),
        };
        save(&path, &key, &value).unwrap();
        value.paired = true;
        save(&path, &key, &value).unwrap();
        assert_eq!(load(&path, &key).unwrap(), value);
        assert_eq!(open(&key, fs::read(&path).unwrap()).unwrap(), value);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
