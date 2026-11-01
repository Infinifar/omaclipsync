//! Transport errors. Error values must not be logged with credentials or bodies.
#[derive(Debug)]
pub enum Error {
    InvalidUrl,
    Http(reqwest::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Status(u16),
    TooLarge,
    InvalidProfile,
    HashMismatch,
    Filtered,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("invalid server URL"),
            Self::Http(_) => f.write_str("HTTP transport failed"),
            Self::Io(_) => f.write_str("transport I/O failed"),
            Self::Json(_) => f.write_str("invalid JSON response"),
            Self::Status(status) => write!(f, "HTTP status {status}"),
            Self::TooLarge => f.write_str("content exceeds the size limit"),
            Self::InvalidProfile => f.write_str("invalid protocol data"),
            Self::HashMismatch => f.write_str("content hash mismatch"),
            Self::Filtered => f.write_str("content rejected by the filter"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
