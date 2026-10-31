//! Text filtering shared by client adapters.
use regex::RegexSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Off,
    Blacklist,
    Whitelist,
}
pub struct Filter {
    mode: FilterMode,
    patterns: RegexSet,
    downloads: bool,
}
impl Filter {
    pub fn new(
        mode: FilterMode,
        patterns: &[String],
        downloads: bool,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            mode,
            patterns: RegexSet::new(patterns)?,
            downloads,
        })
    }
    pub fn matches(&self, text: &str) -> bool {
        self.patterns.is_match(text)
    }
    pub fn allows(&self, text: &str, download: bool) -> bool {
        if download && !self.downloads {
            return true;
        }
        let matched = self.patterns.is_match(text);
        match self.mode {
            FilterMode::Off => true,
            FilterMode::Blacklist => !matched,
            FilterMode::Whitelist => matched,
        }
    }
}
