//! The local bridge between the native-messaging host and the desktop app.
//!
//! The browser starts the native-messaging host as a separate process, while
//! the unlocked vault lives in the desktop app. While browser integration is
//! enabled, the app listens on a random loopback TCP port and records it,
//! together with a fresh random 256-bit key, in [`BRIDGE_FILE`] in its data
//! directory. That directory belongs to the user (the file is mode 0600 on
//! Unix), so only the user's own processes can read the key.
//!
//! Each connection starts with a mutual challenge-response so that the key
//! never crosses the socket:
//!
//! ```text
//! host → app   {"hello": "keyroam-bridge/1", "nonce": Nc}
//! app  → host  {"nonce": Ns, "proof": HMAC(key, "keyroam-bridge/1 app" ‖ Nc ‖ Ns)}
//! host → app   {"proof": HMAC(key, "keyroam-bridge/1 host" ‖ Nc ‖ Ns)}
//! app  → host  {"ok": true}
//! ```
//!
//! After that, the connection carries protocol requests and responses (see
//! [`crate::protocol`]) using [`crate::framing`].

use std::io;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use keyroam_core::LocalStore;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::framing::{read_frame, write_frame};

pub const BRIDGE_FILE: &str = "autofill-bridge.json";
const VERSION: &str = "keyroam-bridge/1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 8;

/// Handles one request and returns the response.
pub type Handler = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

#[derive(Serialize, Deserialize)]
struct BridgeInfo {
    version: String,
    port: u16,
    key: String,
    pid: u32,
}

impl Drop for BridgeInfo {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

type Key = Zeroizing<[u8; 32]>;

fn proof(key: &[u8; 32], role: &str, client_nonce: &[u8], server_nonce: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(VERSION.as_bytes());
    mac.update(b" ");
    mac.update(role.as_bytes());
    mac.update(client_nonce);
    mac.update(server_nonce);
    mac
}

fn nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut n);
    n
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn read_json(mut stream: &TcpStream) -> io::Result<Value> {
    let frame = read_frame(&mut stream)?.ok_or_else(|| invalid("connection closed during handshake"))?;
    serde_json::from_slice(&frame).map_err(|_| invalid("malformed handshake"))
}

fn b64_field(v: &Value, field: &str) -> io::Result<Vec<u8>> {
    v.get(field)
        .and_then(Value::as_str)
        .and_then(|s| B64.decode(s).ok())
        .ok_or_else(|| invalid("malformed handshake"))
}

struct Shared {
    stop: AtomicBool,
    active: AtomicUsize,
    connections: Mutex<Vec<TcpStream>>,
}

/// The app side of the bridge. Dropping it stops listening, closes open
/// connections and removes [`BRIDGE_FILE`].
pub struct BridgeServer {
    addr: SocketAddr,
    store: LocalStore,
    key: Key,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for BridgeServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl BridgeServer {
    /// Listen on a random loopback port and publish it in `store`'s data
    /// directory. `handler` runs on a connection thread for every request.
    pub fn start(store: &LocalStore, handler: Handler) -> io::Result<BridgeServer> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let key: Key = Zeroizing::new(nonce());
        let info = BridgeInfo {
            version: VERSION.into(),
            port: addr.port(),
            key: B64.encode(*key),
            pid: std::process::id(),
        };
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(&info).map_err(io::Error::other)?);
        store
            .write_atomic(BRIDGE_FILE, &bytes)
            .map_err(|e| io::Error::other(e.to_string()))?;

        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            connections: Mutex::new(Vec::new()),
        });
        let accept = {
            let shared = shared.clone();
            let key = key.clone();
            std::thread::Builder::new()
                .name("keyroam-bridge".into())
                .spawn(move || accept_loop(listener, key, handler, shared))?
        };
        Ok(BridgeServer {
            addr,
            store: store.clone(),
            key,
            shared,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for c in self
            .shared
            .connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = c.shutdown(Shutdown::Both);
        }
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
        // Remove the file unless another app instance has replaced it.
        let path = self.store.root().join(BRIDGE_FILE);
        if let Ok(bytes) = std::fs::read(&path) {
            let bytes = Zeroizing::new(bytes);
            if let Ok(info) = serde_json::from_slice::<BridgeInfo>(&bytes) {
                if info.port == self.addr.port() && B64.decode(&info.key).ok().as_deref() == Some(&self.key[..]) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

fn accept_loop(listener: TcpListener, key: Key, handler: Handler, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        if shared.active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            continue;
        }
        let Ok(clone) = stream.try_clone() else { continue };
        {
            let mut conns = shared.connections.lock().unwrap_or_else(|e| e.into_inner());
            conns.retain(|c| c.peer_addr().is_ok());
            conns.push(clone);
        }
        shared.active.fetch_add(1, Ordering::SeqCst);
        let (key, handler, shared2) = (key.clone(), handler.clone(), shared.clone());
        let spawned = std::thread::Builder::new()
            .name("keyroam-bridge-conn".into())
            .spawn(move || {
                let _ = serve(&stream, &key, &handler);
                // The registered clone would otherwise keep the socket open.
                let _ = stream.shutdown(Shutdown::Both);
                shared2.active.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            shared.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn serve(mut stream: &TcpStream, key: &[u8; 32], handler: &Handler) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let hello = read_json(stream)?;
    if hello.get("hello").and_then(Value::as_str) != Some(VERSION) {
        return Err(invalid("unsupported bridge version"));
    }
    let client_nonce = b64_field(&hello, "nonce")?;
    if client_nonce.len() != 32 {
        return Err(invalid("malformed handshake"));
    }
    let server_nonce = nonce();
    let app_proof = proof(key, "app", &client_nonce, &server_nonce).finalize().into_bytes();
    let msg = json!({ "nonce": B64.encode(server_nonce), "proof": B64.encode(app_proof) });
    write_frame(&mut stream, msg.to_string().as_bytes())?;
    let answer = read_json(stream)?;
    let host_proof = b64_field(&answer, "proof")?;
    proof(key, "host", &client_nonce, &server_nonce)
        .verify_slice(&host_proof)
        .map_err(|_| invalid("bridge authentication failed"))?;
    write_frame(&mut stream, br#"{"ok":true}"#)?;
    stream.set_read_timeout(None)?;

    while let Some(mut request) = read_frame(&mut stream)? {
        let mut response = handler(&request);
        request.zeroize();
        let written = write_frame(&mut stream, &response);
        response.zeroize();
        written?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum ConnectError {
    /// No app is listening (not running, or browser integration is off).
    NotRunning,
    /// Something answered but failed authentication.
    Handshake(io::Error),
    Io(io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NotRunning => f.write_str("the KeyRoam app is not running, or browser integration is off"),
            ConnectError::Handshake(e) => write!(f, "could not authenticate the KeyRoam app: {e}"),
            ConnectError::Io(e) => write!(f, "could not reach the KeyRoam app: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// The host side of the bridge.
pub struct BridgeClient {
    stream: TcpStream,
}

impl BridgeClient {
    /// Connect to the app whose data directory is `data_dir`.
    pub fn connect(data_dir: &Path) -> Result<BridgeClient, ConnectError> {
        let bytes = match std::fs::read(data_dir.join(BRIDGE_FILE)) {
            Ok(b) => Zeroizing::new(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(ConnectError::NotRunning),
            Err(e) => return Err(ConnectError::Io(e)),
        };
        let info: BridgeInfo = serde_json::from_slice(&bytes).map_err(|_| ConnectError::NotRunning)?;
        if info.version != VERSION {
            return Err(ConnectError::Handshake(invalid("unsupported bridge version")));
        }
        let key: Key = match B64.decode(&info.key) {
            Ok(mut k) if k.len() == 32 => {
                let mut key = Zeroizing::new([0u8; 32]);
                key.copy_from_slice(&k);
                k.zeroize();
                key
            }
            _ => return Err(ConnectError::NotRunning),
        };
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
        let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => s,
            // A stale file left by an app that crashed. Windows retries a
            // refused connection until the timeout instead of refusing.
            Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut) => {
                return Err(ConnectError::NotRunning)
            }
            Err(e) => return Err(ConnectError::Io(e)),
        };
        Self::handshake(&mut stream, &key).map_err(ConnectError::Handshake)?;
        Ok(BridgeClient { stream })
    }

    fn handshake(stream: &mut TcpStream, key: &[u8; 32]) -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let client_nonce = nonce();
        let hello = json!({ "hello": VERSION, "nonce": B64.encode(client_nonce) });
        write_frame(stream, hello.to_string().as_bytes())?;
        let challenge = read_json(stream)?;
        let server_nonce = b64_field(&challenge, "nonce")?;
        let app_proof = b64_field(&challenge, "proof")?;
        proof(key, "app", &client_nonce, &server_nonce)
            .verify_slice(&app_proof)
            .map_err(|_| invalid("the listening process does not hold the bridge key"))?;
        let host_proof = proof(key, "host", &client_nonce, &server_nonce).finalize().into_bytes();
        write_frame(
            stream,
            json!({ "proof": B64.encode(host_proof) }).to_string().as_bytes(),
        )?;
        let done = read_json(stream)?;
        if done.get("ok") != Some(&Value::Bool(true)) {
            return Err(invalid("bridge authentication failed"));
        }
        stream.set_read_timeout(None)?;
        Ok(())
    }

    /// Send one request and wait for its response.
    pub fn call(&mut self, request: &[u8]) -> io::Result<Vec<u8>> {
        write_frame(&mut self.stream, request)?;
        read_frame(&mut self.stream)?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

/// Data directories where a running KeyRoam app may have published its
/// bridge: `KEYROAM_HOME` when set (the app uses it too), otherwise on macOS
/// the sandboxed app's container, then the platform data directory.
pub fn candidate_data_dirs() -> Vec<PathBuf> {
    if let Some(home) = std::env::var_os("KEYROAM_HOME").filter(|h| !h.is_empty()) {
        return vec![PathBuf::from(home)];
    }
    let mut dirs_out = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            dirs_out.push(
                home.join(
                    "Library/Containers/io.github.ravipurohit1991.keyroam/Data/Library/Application Support/KeyRoam",
                ),
            );
        }
    }
    if let Some(d) = dirs::data_dir() {
        dirs_out.push(d.join("KeyRoam"));
    }
    dirs_out
}

/// Connect to the first running app among `dirs`.
pub fn connect_any(dirs: &[PathBuf]) -> Result<BridgeClient, ConnectError> {
    let mut last = ConnectError::NotRunning;
    for d in dirs {
        match BridgeClient::connect(d) {
            Ok(c) => return Ok(c),
            Err(ConnectError::NotRunning) => {}
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo() -> Handler {
        Arc::new(|req: &[u8]| {
            let mut out = b"echo:".to_vec();
            out.extend_from_slice(req);
            out
        })
    }

    #[test]
    fn authenticated_roundtrip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path());
        assert!(matches!(
            BridgeClient::connect(dir.path()),
            Err(ConnectError::NotRunning)
        ));
        let server = BridgeServer::start(&store, echo()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(BRIDGE_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "bridge file must be private");
        }
        let mut a = BridgeClient::connect(dir.path()).unwrap();
        let mut b = connect_any(&[dir.path().join("missing"), dir.path().to_path_buf()]).unwrap();
        assert_eq!(a.call(b"one").unwrap(), b"echo:one");
        assert_eq!(b.call(b"two").unwrap(), b"echo:two");
        assert_eq!(a.call(b"three").unwrap(), b"echo:three");

        drop(server);
        assert!(!dir.path().join(BRIDGE_FILE).exists());
        assert!(a.call(b"after stop").is_err());
        assert!(matches!(
            BridgeClient::connect(dir.path()),
            Err(ConnectError::NotRunning)
        ));
    }

    #[test]
    fn wrong_key_is_rejected_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path());
        let server = BridgeServer::start(&store, echo()).unwrap();
        let path = dir.path().join(BRIDGE_FILE);
        let real = std::fs::read(&path).unwrap();

        // A client with the wrong key cannot authenticate the app (and the app
        // never accepts it).
        let mut info: Value = serde_json::from_slice(&real).unwrap();
        info["key"] = json!(B64.encode([7u8; 32]));
        std::fs::write(&path, info.to_string()).unwrap();
        assert!(matches!(
            BridgeClient::connect(dir.path()),
            Err(ConnectError::Handshake(_))
        ));

        // A raw client that skips the proof gets no service.
        let mut raw = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        write_frame(
            &mut raw,
            json!({"hello": VERSION, "nonce": B64.encode([1u8; 32])})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let _challenge = read_frame(&mut raw).unwrap().unwrap();
        write_frame(&mut raw, json!({"proof": B64.encode([0u8; 32])}).to_string().as_bytes()).unwrap();
        assert!(read_frame(&mut raw).map(|f| f.is_none()).unwrap_or(true));

        // A stale file (the app crashed) reads as "not running".
        std::fs::write(&path, &real).unwrap();
        let port = server.port();
        drop(server);
        let mut stale: Value = serde_json::from_slice(&real).unwrap();
        stale["port"] = json!(port);
        std::fs::write(&path, stale.to_string()).unwrap();
        assert!(matches!(
            BridgeClient::connect(dir.path()),
            Err(ConnectError::NotRunning)
        ));
    }

    #[test]
    fn stopping_keeps_a_newer_instances_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path());
        let first = BridgeServer::start(&store, echo()).unwrap();
        let second = BridgeServer::start(&store, echo()).unwrap();
        drop(first);
        let mut c = BridgeClient::connect(dir.path()).unwrap();
        assert_eq!(c.call(b"x").unwrap(), b"echo:x");
        drop(second);
        assert!(!dir.path().join(BRIDGE_FILE).exists());
    }
}
