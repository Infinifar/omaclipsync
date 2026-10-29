//! Versioned, bounded JSON-lines messages for the shell-hosted daemon.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

pub const VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 17 * 1024 * 1024;

// Intentionally no Debug implementation: params may contain a keyring password.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
impl Request {
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        let req: Request = serde_json::from_slice(bytes).map_err(|_| ProtocolError::Invalid)?;
        if req.method.is_empty()
            || req.method.len() > 64
            || !req
                .method
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(ProtocolError::Invalid);
        }
        if !req.params.is_null() && !req.params.is_object() {
            return Err(ProtocolError::Invalid);
        }
        Ok(req)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    TooLarge,
    Invalid,
    Io,
}

/// Reads at most one bounded request. Oversized input is a terminal protocol error.
pub fn read_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, ProtocolError> {
    let mut line = Vec::new();
    loop {
        let (len, ended) = {
            let data = reader.fill_buf().map_err(|_| ProtocolError::Io)?;
            if data.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(line))
                };
            }
            let end = data.iter().position(|&b| b == b'\n');
            let len = end.map_or(data.len(), |n| n + 1);
            if line.len().saturating_add(len) > MAX_REQUEST_BYTES {
                return Err(ProtocolError::TooLarge);
            }
            line.extend_from_slice(&data[..len]);
            (len, end.is_some())
        };
        reader.consume(len);
        if ended {
            return Ok(Some(line));
        }
    }
}
#[derive(Serialize)]
struct Reply<'a> {
    id: u64,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}
#[derive(Serialize)]
struct Event<'a> {
    event: &'static str,
    data: &'a Value,
}
fn write_json(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response too large",
        ));
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
pub fn reply(
    writer: &mut impl Write,
    id: u64,
    result: Result<&Value, &'static str>,
) -> io::Result<()> {
    let (ok, value, error) = match result {
        Ok(v) => (true, Some(v), None),
        Err(e) => (false, None, Some(e)),
    };
    write_json(
        writer,
        &Reply {
            id,
            ok,
            result: value,
            error,
        },
    )
}
pub fn event(writer: &mut impl Write, name: &'static str, data: &Value) -> io::Result<()> {
    write_json(writer, &Event { event: name, data })
}
pub fn hello() -> Value {
    json!({"protocol": VERSION})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn handshake_and_replies_are_json_lines() {
        let mut input = Cursor::new(b"{\"id\":1,\"method\":\"hello\"}\n".as_slice());
        let req = Request::parse(&read_line(&mut input).unwrap().unwrap()).unwrap();
        assert_eq!(req.id, 1);
        let mut out = Vec::new();
        reply(&mut out, req.id, Ok(&hello())).unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["result"]["protocol"], 1);
        assert_eq!(value["ok"], true);
        assert!(read_line(&mut input).unwrap().is_none());
    }
    #[test]
    fn malformed_and_unbounded_input_is_rejected() {
        assert!(matches!(
            Request::parse(br#"{"id":1,"method":"x","params":[]}"#),
            Err(ProtocolError::Invalid)
        ));
        assert!(matches!(
            Request::parse(br#"{"id":1,"method":"../../x"}"#),
            Err(ProtocolError::Invalid)
        ));
        let mut large = Cursor::new(vec![b'x'; MAX_REQUEST_BYTES + 1]);
        assert!(matches!(
            read_line(&mut large),
            Err(ProtocolError::TooLarge)
        ));
    }
}
