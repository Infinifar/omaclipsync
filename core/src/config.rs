//! Configuration contains no credentials. Changes are validated before replacing the file.
use crate::{Filter, FilterMode, MAX_TEXT_BYTES};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self},
    io::{self, Read},
    path::Path,
};
use url::Url;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub sync: SyncConfig,
    pub filter: FilterConfig,
    pub history: HistoryConfig,
    pub dedupe: DedupeConfig,
    pub logging: LoggingConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub url: String,
    pub user: String,
    pub insecure: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SyncConfig {
    pub interval_sec: u64,
    pub enabled: bool,
    pub max_inline_bytes: usize,
    pub notify_on_success: bool,
}
impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            interval_sec: 60,
            enabled: true,
            max_inline_bytes: 32768,
            notify_on_success: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct FilterConfig {
    pub mode: FilterKind,
    pub patterns: Vec<String>,
    pub also_block_downloads: bool,
}
impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            mode: FilterKind::Off,
            patterns: vec![],
            also_block_downloads: false,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FilterKind {
    #[default]
    Off,
    Blacklist,
    Whitelist,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub limit: usize,
}
impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: 50,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct DedupeConfig {
    pub window_sec: u64,
}
impl Default for DedupeConfig {
    fn default() -> Self {
        Self { window_sec: 300 }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
}
impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl Config {
    pub fn validate(&self) -> Result<Filter, ConfigError> {
        // Old configurations used 1..29 for foreground polling. The daemon
        // clamps these values to 30 s when using them for fallback polling.
        if !(1..=300).contains(&self.sync.interval_sec) {
            return Err(ConfigError::Invalid("intervalSec must be 1..300"));
        }
        if self.sync.max_inline_bytes > MAX_TEXT_BYTES || self.sync.max_inline_bytes > 32768 {
            return Err(ConfigError::Invalid("maxInlineBytes must be at most 32768"));
        }
        if !(1..=1000).contains(&self.history.limit) {
            return Err(ConfigError::Invalid("history limit must be 1..1000"));
        }
        if self.dedupe.window_sec > 86400 {
            return Err(ConfigError::Invalid("windowSec must be at most 86400"));
        }
        if !["error", "warn", "info", "debug"].contains(&self.logging.level.as_str()) {
            return Err(ConfigError::Invalid("invalid log level"));
        }
        if self.filter.patterns.len() > 256 || self.filter.patterns.iter().any(|p| p.len() > 4096) {
            return Err(ConfigError::Invalid("filter patterns too large"));
        }
        if self.server.url.len() > 2048 || self.server.user.len() > 512 {
            return Err(ConfigError::Invalid("server configuration too long"));
        }
        if !self.server.url.is_empty() {
            let url = Url::parse(&self.server.url)
                .map_err(|_| ConfigError::Invalid("invalid server URL"))?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(ConfigError::Invalid(
                    "server URL must be HTTPS and contain no credentials, query, or fragment",
                ));
            }
        }
        let mode = match self.filter.mode {
            FilterKind::Off => FilterMode::Off,
            FilterKind::Blacklist => FilterMode::Blacklist,
            FilterKind::Whitelist => FilterMode::Whitelist,
        };
        Filter::new(
            mode,
            &self.filter.patterns,
            self.filter.also_block_downloads,
        )
        .map_err(|_| ConfigError::Invalid("invalid regular expression"))
    }
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let file = fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigError::Invalid("configuration too large"));
        }
        let config: Self = serde_json::from_slice(&bytes)?;
        config.validate()?;
        Ok(config)
    }
    pub fn write(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigError::Invalid("configuration too large"));
        }
        crate::storage::write_private(path, &bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn defaults_and_atomic_private_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        assert!(!cfg.history.enabled);
        assert_eq!(cfg.sync.interval_sec, 60);
        assert!(cfg.sync.notify_on_success);
        let old: Config = serde_json::from_str(r#"{"sync":{"enabled":true}}"#).unwrap();
        assert!(old.sync.notify_on_success);
        cfg.server.url = "https://example.org/base".into();
        cfg.write(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(Config::read(&path).unwrap(), cfg);
        cfg.filter.patterns.push("[".into());
        assert!(cfg.write(&path).is_err());
        assert_eq!(Config::read(&path).unwrap().filter.patterns.len(), 0);
    }
    #[test]
    fn reject_credentials_in_file_and_invalid_values() {
        assert!(serde_json::from_str::<Config>(r#"{"server":{"password":"secret"}}"#).is_err());
        let mut cfg = Config::default();
        cfg.sync.interval_sec = 0;
        assert!(cfg.validate().is_err());
        cfg.sync.interval_sec = 3;
        cfg.server.url = "http://example.org".into();
        assert!(cfg.validate().is_err());
    }
}
