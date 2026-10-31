//! Private, atomic file replacement shared by configuration and history.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_private_impl(path, bytes, true)
}

pub(crate) fn write_private_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_private_impl(path, bytes, false)
}

fn write_private_impl(path: &Path, bytes: &[u8], newline: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?;
    fs::create_dir_all(parent)?;
    let mut created = None;
    for n in 0..128 {
        let tmp = parent.join(format!(".omaclipsync.{}.{}.tmp", std::process::id(), n));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => {
                created = Some((tmp, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let (tmp, mut file) = created.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "unable to create temporary file",
        )
    })?;
    let result = (|| {
        file.write_all(bytes)?;
        if newline {
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
