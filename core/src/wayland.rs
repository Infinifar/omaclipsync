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
pub const V1_ORIGIN_PREFIX: &str = "application/x-rustclipsync-v1-";
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum ClipboardError {
    Wayland(paste::Error),
    Copy(copy::Error),
    Io(io::Error),
    TooLarge,
    InvalidUtf8,
    Timeout,
    Sensitive,
    Unavailable,
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
    allow_sensitive: bool,
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
            if safe_text_offer(&mime_types, allow_sensitive) {
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

#[derive(Debug, PartialEq, Eq)]
pub enum V1ClipboardChange {
    Text {
        text: String,
        origin: Option<String>,
        sensitive: bool,
    },
    Cleared,
}

/// V1 needs to observe clears and blocked sensitive offers as well as text.
/// `changed` runs before reading the offer, allowing concurrent downloads to
/// reject responses that started before a newer local selection.
pub fn watch_loop_v1(
    mut watcher: Watcher,
    allow_sensitive: bool,
    mut changed: impl FnMut(),
    mut on_change: impl FnMut(Result<V1ClipboardChange, ClipboardError>),
) -> Result<(), ClipboardError> {
    let mut initial = true;
    while let Some(event) = watcher.next_event()? {
        if initial {
            initial = false;
            continue;
        }
        changed();
        match event {
            ClipboardEvent::Cleared { .. } => on_change(Ok(V1ClipboardChange::Cleared)),
            ClipboardEvent::Changed {
                mime_types,
                mut offer,
                ..
            } => {
                if !safe_text_offer(&mime_types, allow_sensitive) {
                    on_change(Ok(V1ClipboardChange::Cleared));
                    continue;
                }
                let origin = mime_types
                    .iter()
                    .find_map(|mime| mime.strip_prefix(V1_ORIGIN_PREFIX))
                    .filter(|id| {
                        id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                    })
                    .map(str::to_owned);
                let sensitive = mime_types
                    .iter()
                    .any(|m| m == wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE);
                let text = offer
                    .receive(MIME)
                    .map_err(ClipboardError::from)
                    .and_then(|reader| read_bounded(reader, MAX_TEXT_BYTES, READ_TIMEOUT));
                on_change(text.map(|text| V1ClipboardChange::Text {
                    text,
                    origin,
                    sensitive,
                }));
            }
        }
    }
    Ok(())
}

fn safe_text_offer(mime_types: &[String], allow_sensitive: bool) -> bool {
    mime_types.iter().any(|m| m == MIME)
        && (allow_sensitive
            || !mime_types
                .iter()
                .any(|m| m == wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE))
}

/// Read the current selection and its MIME hints from the same Wayland offer.
/// Checking MIME types separately from reading would race with clipboard changes.
pub fn read_current(allow_sensitive: bool) -> Result<String, ClipboardError> {
    read_current_with_sensitivity(allow_sensitive).map(|(text, _)| text)
}

pub fn read_current_with_sensitivity(
    allow_sensitive: bool,
) -> Result<(String, bool), ClipboardError> {
    let mut watcher = create_watcher()?;
    match watcher.next_event()? {
        Some(ClipboardEvent::Changed {
            mime_types,
            mut offer,
            ..
        }) => {
            if !mime_types.iter().any(|mime| mime == MIME) {
                return Err(ClipboardError::Unavailable);
            }
            if !safe_text_offer(&mime_types, allow_sensitive) {
                return Err(ClipboardError::Sensitive);
            }
            let reader = offer.receive(MIME)?;
            let sensitive = mime_types
                .iter()
                .any(|m| m == wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE);
            read_bounded(reader, MAX_TEXT_BYTES, READ_TIMEOUT).map(|text| (text, sensitive))
        }
        _ => Err(ClipboardError::Unavailable),
    }
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

/// Attach an origin MIME marker to recognize our own Wayland selection event.
pub fn write_text_marked(
    text: &str,
    message_id: &str,
    sensitive: bool,
) -> Result<(), ClipboardError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(ClipboardError::TooLarge);
    }
    if message_id.len() != 36
        || !message_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(ClipboardError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid message id",
        )));
    }
    let mut sources = vec![
        copy::MimeSource {
            source: copy::Source::Bytes(text.as_bytes().into()),
            mime_type: copy::MimeType::Text,
        },
        copy::MimeSource {
            source: copy::Source::Bytes(Box::new([])),
            mime_type: copy::MimeType::Specific(format!("{V1_ORIGIN_PREFIX}{message_id}")),
        },
    ];
    if sensitive {
        sources.push(copy::MimeSource {
            source: copy::Source::Bytes(Box::new([])),
            mime_type: copy::MimeType::Specific(
                wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE.into(),
            ),
        });
    }
    copy::Options::new().copy_multi(sources)?;
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
        assert!(safe_text_offer(&[MIME.into()], false));
        assert!(!safe_text_offer(
            &[
                MIME.into(),
                wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE.into()
            ],
            false
        ));
        assert!(safe_text_offer(
            &[
                MIME.into(),
                wl_clipboard_rs::utils::PASSWORD_MANAGER_HINT_MIME_TYPE.into()
            ],
            true
        ));
        assert!(!safe_text_offer(&["image/png".into()], true));
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
