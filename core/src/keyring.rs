//! Secret Service access through `secret-tool`; credentials never enter argv or logs.
use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_SECRET: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug)]
pub enum KeyringError {
    Io(io::Error),
    Timeout,
    Unavailable,
    Invalid,
}
impl From<io::Error> for KeyringError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn run(
    program: &str,
    mode: &str,
    account: &str,
    secret: Option<&str>,
    timeout: Duration,
) -> Result<String, KeyringError> {
    if account.is_empty() || account.len() > 512 || account.contains('\0') {
        return Err(KeyringError::Invalid);
    }
    if let Some(s) = secret {
        if s.is_empty()
            || s.len() > MAX_SECRET
            || s.contains('\0')
            || s.contains('\n')
            || s.contains('\r')
        {
            return Err(KeyringError::Invalid);
        }
    }
    let mut cmd = Command::new(program);
    cmd.arg(mode);
    if mode == "store" {
        cmd.arg("--label=omaclipsync");
    }
    cmd.args(["service", "omaclipsync", "account", account])
        .stdin(if secret.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    if let Some(s) = secret {
        let mut input = child.stdin.take().ok_or(KeyringError::Unavailable)?;
        if let Err(err) = input.write_all(s.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err.into());
        }
    }
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => {
                if !status.success() {
                    return Err(KeyringError::Unavailable);
                }
                let mut data = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or(KeyringError::Unavailable)?
                    .take(MAX_SECRET as u64 + 2)
                    .read_to_end(&mut data)?;
                if data.ends_with(b"\n") {
                    data.pop();
                }
                if data.len() > MAX_SECRET {
                    return Err(KeyringError::Invalid);
                }
                return String::from_utf8(data).map_err(|_| KeyringError::Invalid);
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(KeyringError::Timeout);
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    }
}
pub fn lookup(account: &str) -> Result<String, KeyringError> {
    let secret = run("secret-tool", "lookup", account, None, TIMEOUT)?;
    if secret.is_empty() {
        return Err(KeyringError::Unavailable);
    }
    Ok(secret)
}
pub fn store(account: &str, password: &str) -> Result<(), KeyringError> {
    run("secret-tool", "store", account, Some(password), TIMEOUT).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn secret_is_passed_on_stdin_not_in_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("secret-tool");
        let output = dir.path().join("arguments");
        let stdin = dir.path().join("stdin");
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\n",
                output.display(),
                stdin.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        run(
            program.to_str().unwrap(),
            "store",
            "test",
            Some("a $; <secret>"),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(stdin).unwrap(), "a $; <secret>");
        assert!(!fs::read_to_string(output).unwrap().contains("<secret>"));
    }
    #[test]
    fn bad_input_is_rejected() {
        assert!(matches!(
            run("unused", "store", "", Some("x"), TIMEOUT),
            Err(KeyringError::Invalid)
        ));
        assert!(matches!(
            run("unused", "store", "user", Some("x\ny"), TIMEOUT),
            Err(KeyringError::Invalid)
        ));
    }
}
