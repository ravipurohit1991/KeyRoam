//! The autofill protocol spoken between the browser extension, the
//! native-messaging host and the desktop app.
//!
//! **Framing** follows Chrome/Firefox native messaging: each message is UTF-8
//! JSON preceded by its length as a 32-bit unsigned integer in native byte
//! order. The same framing is used on the local channel between the host and
//! the app ([`crate::ipc`]).
//!
//! **Requests** are `{"v": 1, "id": "...", "type": "...", ...}`. **Responses**
//! echo `v` and `id` and carry either `"ok": {...}` or
//! `"error": {"code": "...", "message": "..."}`. Error codes are stable; see
//! [`codes`].

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Protocol version understood by this build.
pub const PROTOCOL_VERSION: u32 = 1;

/// Native-messaging host name registered with the browsers.
pub const HOST_NAME: &str = "io.github.ravipurohit1991.keyroam";

/// Largest message accepted in either direction. Requests are tiny, and
/// responses stay far below the browsers' 1 MiB host-to-extension limit.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Most logins returned for one page.
pub const MAX_LOGINS: usize = 50;

/// Stable error codes.
pub mod codes {
    pub const INVALID_REQUEST: &str = "invalid_request";
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The desktop app is not running (or browser integration is off).
    pub const APP_NOT_RUNNING: &str = "app_not_running";
    pub const NO_VAULT: &str = "no_vault";
    pub const LOCKED: &str = "locked";
    /// The page cannot be filled (not http/https).
    pub const NOT_FILLABLE: &str = "not_fillable";
    pub const NOT_FOUND: &str = "not_found";
    /// The entry exists but is not saved for this site.
    pub const NO_MATCH: &str = "no_match";
    /// Insecure page or cross-origin frame: the user must confirm the fill.
    pub const CONSENT_REQUIRED: &str = "consent_required";
    pub const INTERNAL: &str = "internal";
}

/// A protocol-level error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: &'static str,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        ProtocolError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

/// A secret string (password) that is wiped when dropped.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// What the extension asks for. `url` is the frame that holds the form and
/// `top_url` the tab's page; both come from the browser's tab and sender
/// information, never from page content.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// App and vault state. Needs no unlocked vault.
    Status {},
    /// Logins saved for this site (titles and usernames, never passwords).
    List { url: String, top_url: String },
    /// The username and password of one login, for the fill the user chose.
    Fill {
        url: String,
        top_url: String,
        entry_id: String,
        #[serde(default)]
        consent: bool,
    },
    /// After a form submission: should the extension offer to save or update?
    CheckSave {
        url: String,
        top_url: String,
        #[serde(default)]
        username: String,
        password: Secret,
    },
    /// The user accepted the save or update prompt.
    Save {
        url: String,
        top_url: String,
        #[serde(default)]
        username: String,
        password: Secret,
        /// Update this existing entry instead of creating one.
        #[serde(default)]
        entry_id: Option<String>,
    },
}

/// A request envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: Value,
    pub command: Command,
}

impl Request {
    /// Parse and validate a request. On failure, the error carries the request
    /// id when one could be read, so the response can still be correlated.
    pub fn parse(bytes: &[u8]) -> Result<Request, (Value, ProtocolError)> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| {
            (
                Value::Null,
                ProtocolError::new(codes::INVALID_REQUEST, "malformed JSON"),
            )
        })?;
        Self::from_value(value)
    }

    pub fn from_value(mut value: Value) -> Result<Request, (Value, ProtocolError)> {
        let obj = value.as_object_mut().ok_or_else(|| {
            (
                Value::Null,
                ProtocolError::new(codes::INVALID_REQUEST, "expected an object"),
            )
        })?;
        let id = match obj.remove("id") {
            Some(id @ (Value::String(_) | Value::Number(_) | Value::Null)) => id,
            None => Value::Null,
            Some(_) => {
                return Err((
                    Value::Null,
                    ProtocolError::new(codes::INVALID_REQUEST, "id must be a string or number"),
                ))
            }
        };
        match obj.remove("v").and_then(|v| v.as_u64()) {
            Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
            Some(v) => {
                return Err((
                    id,
                    ProtocolError::new(
                        codes::UNSUPPORTED_VERSION,
                        format!("protocol version {v} is not supported; update KeyRoam"),
                    ),
                ))
            }
            None => {
                return Err((
                    id,
                    ProtocolError::new(codes::INVALID_REQUEST, "missing protocol version"),
                ))
            }
        }
        let command = serde_json::from_value(value)
            .map_err(|e| (id.clone(), ProtocolError::new(codes::INVALID_REQUEST, e.to_string())))?;
        Ok(Request { id, command })
    }
}

/// A successful response.
pub fn ok_response(id: &Value, body: Value) -> Value {
    json!({ "v": PROTOCOL_VERSION, "id": id, "ok": body })
}

/// An error response.
pub fn error_response(id: &Value, e: &ProtocolError) -> Value {
    json!({ "v": PROTOCOL_VERSION, "id": id, "error": { "code": e.code, "message": e.message } })
}

/// Read one frame. Returns `Ok(None)` on a clean end of stream (the browser
/// closed the port). Frames larger than `limit` are refused without reading
/// their body.
pub fn read_frame(r: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated length")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message of {len} bytes exceeds the {limit}-byte limit"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Write one frame and flush.
pub fn write_frame(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len())
        .ok()
        .filter(|&l| l as usize <= MAX_MESSAGE_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "message too large"))?;
    w.write_all(&len.to_ne_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// Serialise `value` and write it as a frame, wiping the serialised bytes
/// afterwards (responses may contain a password).
pub fn write_json_frame(w: &mut impl Write, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let result = write_frame(w, &bytes);
    bytes.zeroize();
    result
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn parse(v: Value) -> Result<Request, (Value, ProtocolError)> {
        Request::from_value(v)
    }

    #[test]
    fn frame_roundtrip_and_eof() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"a":1}"#).unwrap();
        write_frame(&mut buf, b"{}").unwrap();
        assert_eq!(&buf[..4], &7u32.to_ne_bytes());
        let mut r = Cursor::new(buf);
        assert_eq!(read_frame(&mut r, 100).unwrap().unwrap(), br#"{"a":1}"#);
        assert_eq!(read_frame(&mut r, 100).unwrap().unwrap(), b"{}");
        assert!(read_frame(&mut r, 100).unwrap().is_none());
    }

    #[test]
    fn frame_errors() {
        // Oversized frames are refused before their body is read.
        let mut r = Cursor::new(1_000_000u32.to_ne_bytes().to_vec());
        assert_eq!(read_frame(&mut r, 1024).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Truncated length and truncated body.
        let mut r = Cursor::new(vec![1u8, 0]);
        assert_eq!(
            read_frame(&mut r, 1024).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut bytes = 10u32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"abc");
        assert_eq!(
            read_frame(&mut Cursor::new(bytes), 1024).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        // Writers refuse to produce frames the reader would reject.
        let big = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        assert!(write_frame(&mut Vec::new(), &big).is_err());
    }

    #[test]
    fn parses_every_command() {
        let r = parse(json!({"v": 1, "id": "r1", "type": "status"})).unwrap();
        assert_eq!(r.id, json!("r1"));
        assert_eq!(r.command, Command::Status {});

        let r = parse(json!({"v": 1, "id": 7, "type": "list", "url": "https://a.test/", "top_url": "https://a.test/"}))
            .unwrap();
        assert!(matches!(r.command, Command::List { .. }));

        let r = parse(json!({"v": 1, "type": "fill", "url": "u", "top_url": "t", "entry_id": "e"})).unwrap();
        assert_eq!(r.id, Value::Null);
        assert!(matches!(r.command, Command::Fill { consent: false, .. }));

        let r = parse(json!({"v": 1, "type": "check_save", "url": "u", "top_url": "t", "password": "synthetic-pw"}))
            .unwrap();
        match &r.command {
            Command::CheckSave { username, password, .. } => {
                assert_eq!(username, "");
                assert_eq!(password.expose(), "synthetic-pw");
            }
            other => panic!("{other:?}"),
        }
        // Passwords never appear in debug output.
        assert!(!format!("{r:?}").contains("synthetic-pw"));

        let r = parse(
            json!({"v": 1, "type": "save", "url": "u", "top_url": "t", "username": "a",
                             "password": "p", "entry_id": "e"}),
        )
        .unwrap();
        assert!(matches!(r.command, Command::Save { entry_id: Some(_), .. }));
    }

    #[test]
    fn rejects_bad_requests() {
        let code = |v: Value| parse(v).unwrap_err().1.code;
        assert_eq!(code(json!([])), codes::INVALID_REQUEST);
        assert_eq!(code(json!({"type": "status"})), codes::INVALID_REQUEST);
        assert_eq!(code(json!({"v": 2, "type": "status"})), codes::UNSUPPORTED_VERSION);
        assert_eq!(code(json!({"v": 1, "type": "export_vault"})), codes::INVALID_REQUEST);
        assert_eq!(
            code(json!({"v": 1, "type": "list", "url": "u"})),
            codes::INVALID_REQUEST
        );
        assert_eq!(
            code(json!({"v": 1, "type": "status", "extra": 1})),
            codes::INVALID_REQUEST
        );
        assert_eq!(
            code(json!({"v": 1, "id": {"x": 1}, "type": "status"})),
            codes::INVALID_REQUEST
        );
        assert!(Request::parse(b"not json").is_err());

        // The id survives a failure after it was read.
        let (id, e) = parse(json!({"v": 9, "id": "keep", "type": "status"})).unwrap_err();
        assert_eq!((id, e.code), (json!("keep"), codes::UNSUPPORTED_VERSION));
    }

    #[test]
    fn response_envelopes() {
        let ok = ok_response(&json!("a"), json!({"x": 1}));
        assert_eq!(ok, json!({"v": 1, "id": "a", "ok": {"x": 1}}));
        let err = error_response(&json!(3), &ProtocolError::new(codes::LOCKED, "locked"));
        assert_eq!(err["error"]["code"], "locked");
        assert_eq!(err["id"], 3);
    }
}
