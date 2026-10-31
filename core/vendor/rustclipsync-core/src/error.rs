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
