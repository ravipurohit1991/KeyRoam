//! The local channel between the native-messaging host and the desktop app.
//!
//! The browser starts the host (`keyroam-native-host`) as a separate process,
//! but the unlocked vault lives inside the desktop app. While the app runs it
//! listens on a random loopback port and writes the port and a fresh random
//! 256-bit token to `autofill-endpoint.json` in its data directory (mode 0600
//! on Unix; the per-user profile on Windows). The host reads that file and
//! must present the token, compared in constant time, before any request is
//! answered. Each connection carries one request and one response, using the
//! native-messaging framing from [`crate::protocol`].

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use keyroam_core::LocalStore;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::protocol::{
    codes, error_response, ok_response, read_frame, write_json_frame, ProtocolError, Request, MAX_MESSAGE_BYTES,
};

/// File in the app's data directory that tells the host where to connect.
pub const ENDPOINT_FILE: &str = "autofill-endpoint.json";

/// The macOS app is sandboxed, so its data directory is inside its container.
pub const MACOS_BUNDLE_ID: &str = "io.github.ravipurohit1991.keyroam";

/// Per-connection read and write timeout on the app side.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the host waits for an answer. The app may briefly be busy, for
/// example while a sync round holds the vault.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Simultaneous connections served; further connections are dropped.
const MAX_CONNECTIONS: usize = 8;
/// Room for the token and caller around a maximal request.
const HELLO_LIMIT: usize = MAX_MESSAGE_BYTES + 4096;

/// Contents of [`ENDPOINT_FILE`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("pid", &self.pid)
            .finish()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// The first and only frame a client sends.
#[derive(Serialize, Deserialize)]
struct Hello {
    token: String,
    /// The extension that started the host, as reported by the browser.
    #[serde(default)]
    caller: String,
    request: Value,
}

/// Answers one parsed request. The second argument is the caller (extension
/// origin or id) for diagnostics.
pub type Handler = Arc<dyn Fn(&Request, &str) -> Result<Value, ProtocolError> + Send + Sync>;

/// The app side: a loopback listener that runs until dropped.
pub struct Server {
    addr: SocketAddr,
    dir: PathBuf,
    token: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("addr", &self.addr)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect();
    bytes.zeroize();
    token
}

fn io_err(e: keyroam_core::Error) -> io::Error {
    match e {
        keyroam_core::Error::Io(e) => e,
        other => io::Error::other(other.to_string()),
    }
}

impl Server {
    /// Listen on a random loopback port and publish the endpoint in `dir`.
    pub fn start(dir: &Path, handler: Handler) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let token = random_token();
        let endpoint = Endpoint {
            port: addr.port(),
            token: token.clone(),
            pid: std::process::id(),
        };
        LocalStore::new(dir)
            .write_json(ENDPOINT_FILE, &endpoint)
            .map_err(io_err)?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            let token = token.clone();
            std::thread::Builder::new()
                .name("keyroam-autofill".into())
                .spawn(move || accept_loop(listener, token, handler, stop))?
        };
        Ok(Server {
            addr,
            dir: dir.to_path_buf(),
            token,
            stop,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The data directory holding the endpoint file.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept() so the thread sees the stop flag.
        let _ = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // Remove the endpoint file unless another app instance replaced it.
        let path = self.dir.join(ENDPOINT_FILE);
        if read_endpoint(&path).is_some_and(|e| e.token == self.token) {
            let _ = std::fs::remove_file(path);
        }
        self.token.zeroize();
    }
}

fn accept_loop(listener: TcpListener, token: String, handler: Handler, stop: Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = conn else { continue };
        if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            continue; // dropped: the client sees the connection close
        }
        active.fetch_add(1, Ordering::SeqCst);
        let (token, handler, counter) = (token.clone(), handler.clone(), active.clone());
        let spawned = std::thread::Builder::new()
            .name("keyroam-autofill-conn".into())
            .spawn(move || {
                let _ = serve(stream, &token, &handler);
                counter.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn serve(mut stream: TcpStream, token: &str, handler: &Handler) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let Some(mut frame) = read_frame(&mut stream, HELLO_LIMIT)? else {
        return Ok(());
    };
    let hello = serde_json::from_slice::<Hello>(&frame);
    frame.zeroize();
    let response = match hello {
        Err(_) => error_response(
            &Value::Null,
            &ProtocolError::new(codes::INVALID_REQUEST, "malformed hello"),
        ),
        Ok(h) if !bool::from(h.token.as_bytes().ct_eq(token.as_bytes())) => {
            error_response(&Value::Null, &ProtocolError::new(codes::UNAUTHORIZED, "invalid token"))
        }
        Ok(h) => match Request::from_value(h.request) {
            Err((id, e)) => error_response(&id, &e),
            Ok(req) => match handler(&req, &h.caller) {
                Ok(body) => ok_response(&req.id, body),
                Err(e) => error_response(&req.id, &e),
            },
        },
    };
    write_json_frame(&mut stream, &response)
}

// ----- client (native host) side -----

fn read_endpoint(path: &Path) -> Option<Endpoint> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Data directories where a running app may have published its endpoint.
/// `KEYROAM_HOME`, when set, overrides the defaults as it does for the app.
pub fn candidate_dirs() -> Vec<PathBuf> {
    if let Some(home) = std::env::var_os("KEYROAM_HOME").filter(|h| !h.is_empty()) {
        return vec![PathBuf::from(home)];
    }
    let mut dirs = Vec::new();
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("KeyRoam"));
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            dirs.push(
                home.join("Library/Containers")
                    .join(MACOS_BUNDLE_ID)
                    .join("Data/Library/Application Support/KeyRoam"),
            );
        }
    }
    dirs
}

/// The most recently published endpoint among `dirs`.
pub fn find_endpoint(dirs: &[PathBuf]) -> Option<Endpoint> {
    dirs.iter()
        .map(|d| d.join(ENDPOINT_FILE))
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((modified, read_endpoint(&p)?))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, e)| e)
}

/// Why a request could not be delivered to the app.
#[derive(Debug)]
pub enum ClientError {
    /// No app is listening (not running, or the endpoint file is stale).
    NotRunning,
    Io(io::Error),
    /// The app answered with something that is not a protocol response.
    BadResponse,
}

/// Deliver one request to the app and return its response envelope.
pub fn send(endpoint: &Endpoint, caller: &str, request: &Value) -> Result<Value, ClientError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut => ClientError::NotRunning,
        _ => ClientError::Io(e),
    })?;
    stream
        .set_read_timeout(Some(RESPONSE_TIMEOUT))
        .map_err(ClientError::Io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(ClientError::Io)?;
    let hello = serde_json::json!({ "token": endpoint.token, "caller": caller, "request": request });
    write_json_frame(&mut stream, &hello).map_err(ClientError::Io)?;
    let mut frame = match read_frame(&mut stream, MAX_MESSAGE_BYTES) {
        Ok(Some(f)) => f,
        Ok(None) => return Err(ClientError::BadResponse),
        Err(e) => return Err(ClientError::Io(e)),
    };
    let parsed = serde_json::from_slice::<Value>(&frame);
    frame.zeroize();
    match parsed {
        Ok(v) if v.get("ok").is_some() || v.get("error").is_some() => Ok(v),
        _ => Err(ClientError::BadResponse),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::protocol::{read_frame, write_frame, Command};

    fn echo_handler() -> Handler {
        Arc::new(|req: &Request, caller: &str| match &req.command {
            Command::Status {} => Ok(json!({ "caller": caller })),
            _ => Err(ProtocolError::new(codes::LOCKED, "locked")),
        })
    }

    fn status() -> Value {
        json!({ "v": 1, "id": "r1", "type": "status" })
    }

    #[test]
    fn roundtrip_through_published_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(dir.path(), echo_handler()).unwrap();
        let ep = find_endpoint(&[dir.path().join("missing"), dir.path().to_path_buf()]).unwrap();
        assert_eq!(ep.port, server.port());
        assert_eq!(ep.token.len(), 64);
        assert!(!format!("{ep:?}").contains(&ep.token));

        let r = send(&ep, "chrome-extension://abc/", &status()).unwrap();
        assert_eq!(
            r,
            json!({ "v": 1, "id": "r1", "ok": { "caller": "chrome-extension://abc/" } })
        );

        let r = send(
            &ep,
            "",
            &json!({ "v": 1, "id": 2, "type": "list", "url": "u", "top_url": "t" }),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], "locked");
        assert_eq!(r["id"], 2);

        let r = send(&ep, "", &json!({ "v": 5, "id": 3, "type": "status" })).unwrap();
        assert_eq!(r["error"]["code"], "unsupported_version");
    }

    #[test]
    fn wrong_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let _server = Server::start(dir.path(), echo_handler()).unwrap();
        let mut ep = find_endpoint(&[dir.path().to_path_buf()]).unwrap();
        ep.token = "0".repeat(64);
        let r = send(&ep, "", &status()).unwrap();
        assert_eq!(r["error"]["code"], "unauthorized");
        ep.token = String::new();
        assert_eq!(send(&ep, "", &status()).unwrap()["error"]["code"], "unauthorized");
    }

    #[test]
    fn malformed_and_oversized_hellos() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(dir.path(), echo_handler()).unwrap();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, server.port()));

        let mut s = TcpStream::connect(addr).unwrap();
        write_frame(&mut s, b"garbage").unwrap();
        let r: Value = serde_json::from_slice(&read_frame(&mut s, MAX_MESSAGE_BYTES).unwrap().unwrap()).unwrap();
        assert_eq!(r["error"]["code"], "invalid_request");

        // A huge length prefix makes the server hang up without reading on.
        let mut s = TcpStream::connect(addr).unwrap();
        use std::io::Write;
        s.write_all(&(u32::MAX).to_ne_bytes()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(matches!(read_frame(&mut s, MAX_MESSAGE_BYTES), Ok(None) | Err(_)));

        // The server keeps working afterwards.
        let ep = find_endpoint(&[dir.path().to_path_buf()]).unwrap();
        assert!(send(&ep, "", &status()).unwrap().get("ok").is_some());
    }

    #[test]
    fn drop_removes_endpoint_and_clients_see_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(dir.path(), echo_handler()).unwrap();
        let ep = find_endpoint(&[dir.path().to_path_buf()]).unwrap();
        drop(server);
        assert!(!dir.path().join(ENDPOINT_FILE).exists());
        assert!(find_endpoint(&[dir.path().to_path_buf()]).is_none());
        assert!(matches!(send(&ep, "", &status()), Err(ClientError::NotRunning)));
    }

    #[test]
    fn newer_endpoint_wins_and_is_kept_on_old_drop() {
        let a = tempfile::tempdir().unwrap();
        let first = Server::start(a.path(), echo_handler()).unwrap();
        // A second instance in the same directory replaces the endpoint.
        let second = Server::start(a.path(), echo_handler()).unwrap();
        drop(first);
        let ep = find_endpoint(&[a.path().to_path_buf()]).unwrap();
        assert_eq!(ep.port, second.port());
        assert!(send(&ep, "", &status()).unwrap().get("ok").is_some());
    }
}
