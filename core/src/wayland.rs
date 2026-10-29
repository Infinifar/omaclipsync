//! Event-driven regular Wayland clipboard access. Never reads the primary selection.
use crate::MAX_TEXT_BYTES;
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};
use wl_clipboard_rs::{
    copy, paste,
    watch::{ClipboardEvent, ClipboardType, Watcher},
};

pub const MIME: &str = "text/plain;charset=utf-8";
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum ClipboardError {
    Wayland(paste::Error),
    Copy(copy::Error),
    Io(io::Error),
    TooLarge,
    InvalidUtf8,
    Timeout,
}
impl From<paste::Error> for ClipboardError {
    fn from(e: paste::Error) -> Self {
        Self::Wayland(e)
    }
}
impl From<copy::Error> for ClipboardError {
    fn from(e: copy::Error) -> Self {
        Self::Copy(e)
    }
}
impl From<io::Error> for ClipboardError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub fn create_watcher() -> Result<Watcher, ClipboardError> {
    Ok(Watcher::new(
        ClipboardType::Regular,
        paste::Seat::Unspecified,
    )?)
}

/// Drive on a dedicated thread; the caller holds `watcher.cancel_handle()` for shutdown.
/// The initial selection is not an observed copy and is skipped, so starting the
/// daemon cannot overwrite a newer server selection with an old local clipboard.
/// Unavailable MIME types and cleared selections are ignored. An oversized, invalid,
/// or stalled offer is reported without exposing its contents and watching continues.
pub fn watch_loop(
    mut watcher: Watcher,
    mut on_text: impl FnMut(Result<String, ClipboardError>),
) -> Result<(), ClipboardError> {
    let mut initial = true;
    while let Some(event) = watcher.next_event()? {
        if initial {
            initial = false;
            continue;
        }
        if let ClipboardEvent::Changed {
            mime_types,
            mut offer,
            ..
        } = event
        {
            if safe_text_offer(&mime_types) {
                let result = offer
                    .receive(MIME)
                    .map_err(ClipboardError::from)
                    .and_then(|reader| read_bounded(reader, MAX_TEXT_BYTES, READ_TIMEOUT));
                on_text(result);
            }
        }
    }
    Ok(())
}

fn safe_text_offer(mime_types: &[String]) -> bool {
    mime_types.iter().any(|m| m == MIME)
        && !mime_types
            .iter()
            .any(|m| m == wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE)
}

/// Used for manual upload; does not print or persist clipboard content.
pub fn read_current() -> Result<String, ClipboardError> {
    let (reader, _) = paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Specific(MIME),
    )?;
    read_bounded(reader, MAX_TEXT_BYTES, READ_TIMEOUT)
}

/// The library starts a serving thread for the selection; no child process is spawned.
pub fn write_text(text: &str) -> Result<(), ClipboardError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(ClipboardError::TooLarge);
    }
    copy::Options::new().copy(
        copy::Source::Bytes(text.as_bytes().into()),
        copy::MimeType::Text,
    )?;
    Ok(())
}

pub fn read_bounded(
    mut reader: impl Read + AsRawFd,
    max: usize,
    timeout: Duration,
) -> Result<String, ClipboardError> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ClipboardError::Timeout);
        }
        let millis = left.as_millis().max(1).min(i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor points to a valid single pollfd throughout this call.
        let result = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if result == 0 {
            return Err(ClipboardError::Timeout);
        }
        if result < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF).into());
        }
        let count = match reader.read(&mut buffer) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > max {
            return Err(ClipboardError::TooLarge);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).map_err(|_| ClipboardError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::net::UnixStream};
    #[test]
    fn ignores_password_manager_hints() {
        assert!(safe_text_offer(&[MIME.into()]));
        assert!(!safe_text_offer(&[
            MIME.into(),
            wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE.into()
        ]));
        assert!(!safe_text_offer(&["image/png".into()]));
    }
    #[test]
    fn bounded_utf8_and_oversize() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all("你好".as_bytes()).unwrap();
        drop(writer);
        assert_eq!(
            read_bounded(reader, 6, Duration::from_secs(1)).unwrap(),
            "你好"
        );
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"12345").unwrap();
        drop(writer);
        assert!(matches!(
            read_bounded(reader, 4, Duration::from_secs(1)),
            Err(ClipboardError::TooLarge)
        ));
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&[0xff]).unwrap();
        drop(writer);
        assert!(matches!(
            read_bounded(reader, 10, Duration::from_secs(1)),
            Err(ClipboardError::InvalidUtf8)
        ));
    }
    #[test]
    fn stalled_offer_times_out() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        assert!(matches!(
            read_bounded(reader, 10, Duration::from_millis(20)),
            Err(ClipboardError::Timeout)
        ));
    }
}
