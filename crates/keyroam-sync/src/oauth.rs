//! Google OAuth 2.0 for installed applications, with PKCE (RFC 7636, S256)
//! and the least-privilege `drive.appdata` scope.
//!
//! Desktop flow: a one-shot HTTP listener on `127.0.0.1:<random port>` receives
//! the authorization code (loopback redirect, RFC 8252 §7.3).
//!
//! Mobile flow (RFC 8252 §7.1): Google redirects to the reversed client id,
//! a private-use URI scheme (`com.googleusercontent.apps.<id>:/oauth2redirect`)
//! that the KeyRoam app registers. The platform hands that redirect to
//! [`RedirectSlot::deliver`]; PKCE protects the code even if another app
//! registered the same scheme.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;
use zeroize::Zeroizing;

use crate::error::{Result, SyncError};
use crate::http::{agent, check_endpoint};

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
/// The only scope KeyRoam requests: a private, app-specific Drive folder.
pub const DRIVE_APPDATA_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";

pub const ENV_CLIENT_ID: &str = "KEYROAM_GOOGLE_CLIENT_ID";
pub const ENV_CLIENT_SECRET: &str = "KEYROAM_GOOGLE_CLIENT_SECRET";
/// Client id of the Google OAuth client for the mobile apps (Android or iOS
/// application type). It has no client secret.
pub const ENV_MOBILE_CLIENT_ID: &str = "KEYROAM_GOOGLE_MOBILE_CLIENT_ID";
const GOOGLE_CLIENT_SUFFIX: &str = ".apps.googleusercontent.com";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthEndpoints {
    pub auth_url: String,
    pub token_url: String,
    pub revoke_url: String,
}

impl Default for OAuthEndpoints {
    fn default() -> Self {
        OAuthEndpoints {
            auth_url: GOOGLE_AUTH_URL.into(),
            token_url: GOOGLE_TOKEN_URL.into(),
            revoke_url: GOOGLE_REVOKE_URL.into(),
        }
    }
}

/// OAuth client registration. For Google "Desktop app" clients the client
/// secret is not confidential (it ships inside the app); PKCE is what protects
/// the authorization code.
#[derive(Clone)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub endpoints: OAuthEndpoints,
}

impl std::fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "<set>"))
            .field("endpoints", &self.endpoints)
            .finish()
    }
}

impl OAuthClientConfig {
    pub fn google(client_id: impl Into<String>, client_secret: Option<String>) -> Self {
        OAuthClientConfig {
            client_id: client_id.into(),
            client_secret,
            endpoints: OAuthEndpoints::default(),
        }
    }

    /// Read the client registration from the environment at runtime, falling
    /// back to values baked in at build time (release builds set these).
    /// Android and iOS builds use the mobile client when one is configured,
    /// and otherwise the desktop client with its loopback redirect, the same
    /// choice `begin_sign_in` makes, so tokens are refreshed with the client
    /// that issued them.
    pub fn from_env() -> Result<Self> {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            if let Ok(mobile) = Self::mobile_from_env() {
                return Ok(mobile);
            }
        }
        Self::desktop_from_env()
    }

    /// The mobile client: [`ENV_MOBILE_CLIENT_ID`], no secret.
    pub fn mobile_from_env() -> Result<Self> {
        let id = pick_setting(
            std::env::var(ENV_MOBILE_CLIENT_ID).ok(),
            option_env!("KEYROAM_GOOGLE_MOBILE_CLIENT_ID"),
        )
        .ok_or_else(|| {
            SyncError::NotConfigured(format!("set {ENV_MOBILE_CLIENT_ID} (see docs/google-cloud-setup.md)"))
        })?;
        mobile_redirect_uri(&id)?;
        Ok(Self::google(id, None))
    }

    /// The desktop ("Desktop app") client.
    pub fn desktop_from_env() -> Result<Self> {
        let id = pick_setting(
            std::env::var(ENV_CLIENT_ID).ok(),
            option_env!("KEYROAM_GOOGLE_CLIENT_ID"),
        )
        .ok_or_else(|| SyncError::NotConfigured(format!("set {ENV_CLIENT_ID} (see docs/google-cloud-setup.md)")))?;
        let secret = pick_setting(
            std::env::var(ENV_CLIENT_SECRET).ok(),
            option_env!("KEYROAM_GOOGLE_CLIENT_SECRET"),
        );
        Ok(Self::google(id, secret))
    }

    fn validate(&self) -> Result<()> {
        check_endpoint(&self.endpoints.auth_url)?;
        check_endpoint(&self.endpoints.token_url)?;
        check_endpoint(&self.endpoints.revoke_url)
    }
}

fn pick_setting(runtime: Option<String>, build: Option<&'static str>) -> Option<String> {
    runtime.or(build.map(String::from)).filter(|s| !s.trim().is_empty())
}

/// The custom-scheme redirect URI of a Google mobile client: its id reversed,
/// `com.googleusercontent.apps.<id>:/oauth2redirect`.
pub fn mobile_redirect_uri(client_id: &str) -> Result<String> {
    let id = client_id
        .trim()
        .strip_suffix(GOOGLE_CLIENT_SUFFIX)
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .ok_or_else(|| SyncError::NotConfigured(format!("{client_id:?} is not a Google OAuth client id")))?;
    Ok(format!("com.googleusercontent.apps.{id}:/oauth2redirect"))
}

/// Extract the authorization code from a custom-scheme redirect. The redirect
/// must be for `redirect_uri` and carry `expected_state` (compared in
/// constant time).
pub fn code_from_redirect(redirect: &str, redirect_uri: &str, expected_state: &str) -> Result<Zeroizing<String>> {
    let url = Url::parse(redirect.trim()).map_err(|_| SyncError::Auth("malformed sign-in redirect".into()))?;
    let expected = Url::parse(redirect_uri).map_err(|_| SyncError::Auth("malformed redirect URI".into()))?;
    if url.scheme() != expected.scheme() || url.path() != expected.path() || url.has_host() {
        return Err(SyncError::Auth("the sign-in redirect is for another app".into()));
    }
    let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let state_ok = get("state").is_some_and(|s| bool::from(s.as_bytes().ct_eq(expected_state.as_bytes())));
    if !state_ok {
        return Err(SyncError::Auth("OAuth state mismatch; sign-in aborted".into()));
    }
    if let Some(err) = get("error") {
        return Err(SyncError::Auth(format!("Google sign-in was not completed: {err}")));
    }
    get("code")
        .filter(|c| !c.is_empty())
        .map(Zeroizing::new)
        .ok_or_else(|| SyncError::Auth("the sign-in redirect has no authorization code".into()))
}

/// Where the platform delivers a custom-scheme redirect (on Android, the
/// activity that receives the redirect intent).
#[derive(Clone, Default)]
pub struct RedirectSlot(Arc<(Mutex<Option<Zeroizing<String>>>, Condvar)>);

impl RedirectSlot {
    pub fn deliver(&self, redirect: &str) {
        let (slot, ready) = &*self.0;
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Zeroizing::new(redirect.to_string()));
        ready.notify_all();
    }

    fn clear(&self) {
        self.0 .0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn wait(&self, timeout: Duration, cancel: &AtomicBool) -> Result<Zeroizing<String>> {
        let deadline = Instant::now() + timeout;
        let (slot, ready) = &*self.0;
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(redirect) = guard.take() {
                return Ok(redirect);
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(SyncError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(SyncError::Auth("timed out waiting for Google sign-in".into()));
            }
            let step = (deadline - now).min(Duration::from_millis(100));
            guard = ready.wait_timeout(guard, step).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// PKCE verifier/challenge pair (S256).
pub struct Pkce {
    verifier: Zeroizing<String>,
    challenge: String,
}

impl Pkce {
    pub fn generate() -> Pkce {
        let mut bytes = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(bytes.as_mut());
        Self::from_verifier(&B64URL.encode(bytes.as_ref())).expect("generated verifier is valid")
    }

    pub fn from_verifier(verifier: &str) -> Result<Pkce> {
        let ok_len = (43..=128).contains(&verifier.len());
        let ok_chars = verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'));
        if !ok_len || !ok_chars {
            return Err(SyncError::Auth("invalid PKCE verifier".into()));
        }
        let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
        Ok(Pkce {
            verifier: Zeroizing::new(verifier.to_string()),
            challenge,
        })
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// Unguessable `state` value binding the redirect to this request (CSRF defence).
pub fn random_state() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    B64URL.encode(b)
}

pub fn authorization_url(cfg: &OAuthClientConfig, redirect_uri: &str, pkce: &Pkce, state: &str) -> Result<String> {
    cfg.validate()?;
    let url = Url::parse_with_params(
        &cfg.endpoints.auth_url,
        &[
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", DRIVE_APPDATA_SCOPE),
            ("code_challenge", pkce.challenge()),
            ("code_challenge_method", "S256"),
            ("state", state),
            // A refresh token is needed so sync keeps working without re-consent.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|e| SyncError::NotConfigured(e.to_string()))?;
    Ok(url.into())
}

#[derive(Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("expires_in", &self.expires_in)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl TokenResponse {
    /// Reject tokens that do not carry the Drive app-data scope (the user can
    /// untick scopes on Google's consent screen).
    pub fn ensure_scope(&self) -> Result<()> {
        match &self.scope {
            Some(s) if !s.split_whitespace().any(|x| x == DRIVE_APPDATA_SCOPE) => Err(SyncError::Auth(
                "permission to store KeyRoam data in Google Drive was not granted".into(),
            )),
            _ => Ok(()),
        }
    }
}

impl Drop for TokenResponse {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

fn token_request(cfg: &OAuthClientConfig, params: &[(&str, &str)]) -> Result<TokenResponse> {
    cfg.validate()?;
    let mut form: Vec<(&str, &str)> = params.to_vec();
    form.push(("client_id", &cfg.client_id));
    if let Some(secret) = &cfg.client_secret {
        form.push(("client_secret", secret));
    }
    match agent().post(&cfg.endpoints.token_url).send_form(&form) {
        Ok(resp) => resp
            .into_json::<TokenResponse>()
            .map_err(|e| SyncError::Protocol(format!("token response: {e}"))),
        Err(ureq::Error::Status(status, resp)) => {
            let body: OAuthErrorBody = resp.into_json().unwrap_or(OAuthErrorBody {
                error: String::new(),
                error_description: String::new(),
            });
            let msg = format!("{} {}", body.error, body.error_description).trim().to_string();
            if body.error == "invalid_grant" {
                Err(SyncError::AuthRequired(msg))
            } else {
                Err(SyncError::Auth(format!("token endpoint returned {status}: {msg}")))
            }
        }
        Err(ureq::Error::Transport(t)) => Err(SyncError::Network(t.to_string())),
    }
}

/// Exchange an authorization code (plus PKCE verifier) for tokens.
pub fn exchange_code(cfg: &OAuthClientConfig, code: &str, pkce: &Pkce, redirect_uri: &str) -> Result<TokenResponse> {
    let t = token_request(
        cfg,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", pkce.verifier()),
            ("redirect_uri", redirect_uri),
        ],
    )?;
    t.ensure_scope()?;
    Ok(t)
}

pub fn refresh_access_token(cfg: &OAuthClientConfig, refresh_token: &str) -> Result<TokenResponse> {
    token_request(
        cfg,
        &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)],
    )
}

/// Revoke a refresh or access token (used on sign-out).
pub fn revoke_token(cfg: &OAuthClientConfig, token: &str) -> Result<()> {
    cfg.validate()?;
    match agent().post(&cfg.endpoints.revoke_url).send_form(&[("token", token)]) {
        Ok(_) => Ok(()),
        // Already invalid: nothing left to revoke.
        Err(ureq::Error::Status(400, _)) => Ok(()),
        Err(e) => Err(crate::http::map_error(e)),
    }
}

/// One-shot loopback HTTP listener that receives the OAuth redirect.
pub struct LoopbackReceiver {
    listener: TcpListener,
    port: u16,
}

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>KeyRoam</title>\
<body style=\"font-family:sans-serif;text-align:center;margin-top:15vh\">\
<h2>KeyRoam is connected to Google Drive.</h2><p>You can close this tab and return to KeyRoam.</p>";
const FAILURE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>KeyRoam</title>\
<body style=\"font-family:sans-serif;text-align:center;margin-top:15vh\">\
<h2>Sign-in was not completed.</h2><p>Return to KeyRoam and try again.</p>";

impl LoopbackReceiver {
    pub fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        Ok(LoopbackReceiver { listener, port })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Wait for the browser redirect and return the authorization code.
    /// Requests without a code (e.g. `/favicon.ico`) are ignored.
    pub fn wait_for_code(&self, expected_state: &str, timeout: Duration) -> Result<Zeroizing<String>> {
        self.wait_for_code_cancellable(expected_state, timeout, &AtomicBool::new(false))
    }

    /// Like [`Self::wait_for_code`], but returns [`SyncError::Cancelled`] as
    /// soon as `cancel` is set (e.g. the user closed the sign-in screen).
    pub fn wait_for_code_cancellable(
        &self,
        expected_state: &str,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<Zeroizing<String>> {
        let deadline = Instant::now() + timeout;
        self.listener.set_nonblocking(true)?;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = handle_redirect(stream, expected_state) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if cancel.load(Ordering::SeqCst) {
                        return Err(SyncError::Cancelled);
                    }
                    if Instant::now() >= deadline {
                        return Err(SyncError::Auth("timed out waiting for Google sign-in".into()));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

fn respond(mut stream: &TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn handle_redirect(mut stream: TcpStream, expected_state: &str) -> Option<Result<Zeroizing<String>>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("GET "))
        .and_then(|l| l.split(' ').next())
        .unwrap_or("");
    let url = match Url::parse(&format!("http://127.0.0.1{target}")) {
        Ok(u) => u,
        Err(_) => {
            respond(&stream, "400 Bad Request", FAILURE_PAGE);
            return None;
        }
    };
    let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let (code, state, error) = (get("code"), get("state"), get("error"));
    if code.is_none() && error.is_none() {
        respond(&stream, "404 Not Found", "");
        return None;
    }
    let state_ok = state
        .as_deref()
        .is_some_and(|s| bool::from(s.as_bytes().ct_eq(expected_state.as_bytes())));
    if !state_ok {
        respond(&stream, "400 Bad Request", FAILURE_PAGE);
        return Some(Err(SyncError::Auth("OAuth state mismatch; sign-in aborted".into())));
    }
    if let Some(err) = error {
        respond(&stream, "200 OK", FAILURE_PAGE);
        return Some(Err(SyncError::Auth(format!("Google sign-in was not completed: {err}"))));
    }
    respond(&stream, "200 OK", SUCCESS_PAGE);
    Some(Ok(Zeroizing::new(code.unwrap_or_default())))
}

/// Source of short-lived Drive access tokens.
pub trait AccessTokenSource {
    fn access_token(&self) -> Result<Zeroizing<String>>;
    /// Discard any cached token (called after a 401).
    fn invalidate(&self);
}

/// A signed-in Google session backed by a refresh token.
pub struct GoogleSession {
    cfg: OAuthClientConfig,
    refresh_token: Zeroizing<String>,
    cached: Mutex<Option<(Zeroizing<String>, Instant)>>,
}

impl std::fmt::Debug for GoogleSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleSession").finish_non_exhaustive()
    }
}

impl GoogleSession {
    pub fn new(cfg: OAuthClientConfig, refresh_token: &str) -> Self {
        GoogleSession {
            cfg,
            refresh_token: Zeroizing::new(refresh_token.to_string()),
            cached: Mutex::new(None),
        }
    }

    fn with_access_token(self, token: &TokenResponse) -> Self {
        let expiry = Instant::now() + Duration::from_secs(token.expires_in.unwrap_or(3600));
        *self.cached.lock().unwrap() = Some((Zeroizing::new(token.access_token.clone()), expiry));
        self
    }

    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    /// Revoke the refresh token at Google (sign out).
    pub fn revoke(&self) -> Result<()> {
        revoke_token(&self.cfg, &self.refresh_token)
    }
}

impl AccessTokenSource for GoogleSession {
    fn access_token(&self) -> Result<Zeroizing<String>> {
        let mut cached = self.cached.lock().unwrap();
        if let Some((tok, expiry)) = cached.as_ref() {
            if Instant::now() + Duration::from_secs(60) < *expiry {
                return Ok(tok.clone());
            }
        }
        let t = refresh_access_token(&self.cfg, &self.refresh_token)?;
        t.ensure_scope()?;
        let tok = Zeroizing::new(t.access_token.clone());
        let expiry = Instant::now() + Duration::from_secs(t.expires_in.unwrap_or(3600));
        *cached = Some((tok.clone(), expiry));
        Ok(tok)
    }

    fn invalidate(&self) {
        *self.cached.lock().unwrap() = None;
    }
}

/// Run the complete desktop sign-in: open the browser at Google's consent
/// page, receive the redirect on a loopback port and exchange the code.
pub fn authorize_installed_app(
    cfg: &OAuthClientConfig,
    timeout: Duration,
    open_browser: impl FnOnce(&str),
) -> Result<GoogleSession> {
    let pending = PendingAuthorization::begin(cfg)?;
    open_browser(pending.url());
    pending.complete(timeout, &AtomicBool::new(false))
}

/// A desktop sign-in that has been started (loopback listener bound, PKCE and
/// state generated) but not yet completed. Split in two so GUIs can show the
/// URL, open the browser themselves and offer a Cancel button.
pub struct PendingAuthorization {
    cfg: OAuthClientConfig,
    receiver: Receiver,
    pkce: Pkce,
    state: String,
    url: String,
}

enum Receiver {
    Loopback(LoopbackReceiver),
    CustomScheme { redirect_uri: String, slot: RedirectSlot },
}

impl Receiver {
    fn redirect_uri(&self) -> String {
        match self {
            Receiver::Loopback(r) => r.redirect_uri(),
            Receiver::CustomScheme { redirect_uri, .. } => redirect_uri.clone(),
        }
    }
}

impl PendingAuthorization {
    /// Desktop: bind a loopback listener for the redirect.
    pub fn begin(cfg: &OAuthClientConfig) -> Result<Self> {
        Self::start(cfg, Receiver::Loopback(LoopbackReceiver::bind()?))
    }

    /// Mobile: the redirect to `redirect_uri` arrives through `slot`. Anything
    /// already in the slot (from an earlier attempt) is discarded.
    pub fn begin_custom_scheme(cfg: &OAuthClientConfig, redirect_uri: &str, slot: RedirectSlot) -> Result<Self> {
        slot.clear();
        Self::start(
            cfg,
            Receiver::CustomScheme {
                redirect_uri: redirect_uri.to_string(),
                slot,
            },
        )
    }

    fn start(cfg: &OAuthClientConfig, receiver: Receiver) -> Result<Self> {
        let pkce = Pkce::generate();
        let state = random_state();
        let url = authorization_url(cfg, &receiver.redirect_uri(), &pkce, &state)?;
        Ok(PendingAuthorization {
            cfg: cfg.clone(),
            receiver,
            pkce,
            state,
            url,
        })
    }

    /// The Google consent URL to open in the user's browser.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Wait for the redirect, then exchange the code for tokens.
    pub fn complete(self, timeout: Duration, cancel: &AtomicBool) -> Result<GoogleSession> {
        let redirect_uri = self.receiver.redirect_uri();
        let code = match &self.receiver {
            Receiver::Loopback(r) => r.wait_for_code_cancellable(&self.state, timeout, cancel)?,
            Receiver::CustomScheme { slot, .. } => {
                code_from_redirect(&slot.wait(timeout, cancel)?, &redirect_uri, &self.state)?
            }
        };
        let tokens = exchange_code(&self.cfg, &code, &self.pkce, &redirect_uri)?;
        let refresh = tokens.refresh_token.clone().ok_or_else(|| {
            SyncError::Auth("Google did not issue a refresh token; remove KeyRoam's access in your Google account settings and try again".into())
        })?;
        Ok(GoogleSession::new(self.cfg, &refresh).with_access_token(&tokens))
    }
}

/// Open `url` in the system browser. Returns false if no browser could be launched.
pub fn open_system_browser(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{HttpRequest, HttpResponse, TestServer};
    use std::sync::Arc;

    #[test]
    fn pkce_rfc7636_test_vector() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk").unwrap();
        assert_eq!(p.challenge(), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_generation() {
        let a = Pkce::generate();
        let b = Pkce::generate();
        assert_eq!(a.verifier().len(), 43);
        assert_ne!(a.verifier(), b.verifier());
        assert!(Pkce::from_verifier("short").is_err());
        assert!(Pkce::from_verifier(&"a b".repeat(20)).is_err());
    }

    #[test]
    fn authorization_url_requests_only_appdata_scope() {
        let cfg = OAuthClientConfig::google("client-123.apps.googleusercontent.com", None);
        let pkce = Pkce::generate();
        let url = authorization_url(&cfg, "http://127.0.0.1:5555", &pkce, "st4te").unwrap();
        let u = Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(u.host_str(), Some("accounts.google.com"));
        assert_eq!(q["scope"], DRIVE_APPDATA_SCOPE);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], pkce.challenge());
        assert_eq!(q["state"], "st4te");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555");
        assert_eq!(q["response_type"], "code");
        assert!(!url.contains(pkce.verifier()));
    }

    #[test]
    fn insecure_endpoints_rejected() {
        let mut cfg = OAuthClientConfig::google("id", None);
        cfg.endpoints.token_url = "http://evil.example/token".into();
        assert!(matches!(
            authorization_url(&cfg, "http://127.0.0.1:1", &Pkce::generate(), "s"),
            Err(SyncError::NotConfigured(_))
        ));
    }

    fn send_redirect(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn loopback_receives_code_and_ignores_noise() {
        let r = LoopbackReceiver::bind().unwrap();
        let port = r.port;
        let t = std::thread::spawn(move || {
            send_redirect(port, "/favicon.ico");
            send_redirect(port, "/?state=expected&code=4%2Fsynthetic-code&scope=x")
        });
        let code = r.wait_for_code("expected", Duration::from_secs(10)).unwrap();
        assert_eq!(code.as_str(), "4/synthetic-code");
        assert!(t.join().unwrap().contains("connected to Google Drive"));
    }

    #[test]
    fn loopback_rejects_state_mismatch_and_errors() {
        let r = LoopbackReceiver::bind().unwrap();
        let port = r.port;
        std::thread::spawn(move || send_redirect(port, "/?state=forged&code=abc"));
        assert!(matches!(
            r.wait_for_code("expected", Duration::from_secs(10)),
            Err(SyncError::Auth(_))
        ));

        let r = LoopbackReceiver::bind().unwrap();
        let port = r.port;
        std::thread::spawn(move || send_redirect(port, "/?state=expected&error=access_denied"));
        let err = r.wait_for_code("expected", Duration::from_secs(10)).unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn loopback_times_out() {
        let r = LoopbackReceiver::bind().unwrap();
        assert!(r.wait_for_code("s", Duration::from_millis(200)).is_err());
    }

    #[test]
    fn pending_authorization_can_be_cancelled() {
        let cfg = OAuthClientConfig::google("client-id", None);
        let pending = PendingAuthorization::begin(&cfg).unwrap();
        assert!(pending.url().starts_with(GOOGLE_AUTH_URL));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c2.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let r = pending.complete(Duration::from_secs(30), &cancel);
        assert!(matches!(r, Err(SyncError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A local stand-in for Google's token endpoint that enforces PKCE.
    fn fake_token_server() -> (TestServer, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let challenge_for_code = Arc::new(Mutex::new(None::<String>));
        let server = TestServer::start(move |req: HttpRequest| {
            let form: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(&req.body).into_owned().collect();
            seen2
                .lock()
                .unwrap()
                .push(form.get("grant_type").cloned().unwrap_or_default());
            if req.path == "/challenge" {
                *challenge_for_code.lock().unwrap() = Some(String::from_utf8(req.body.clone()).unwrap());
                return HttpResponse::json(200, "{}");
            }
            match form.get("grant_type").map(String::as_str) {
                Some("authorization_code") => {
                    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                    let expected = challenge_for_code.lock().unwrap().clone().unwrap_or_default();
                    let actual = B64URL.encode(Sha256::digest(verifier.as_bytes()));
                    if actual != expected || form.get("code").map(String::as_str) != Some("good-code") {
                        return HttpResponse::json(
                            400,
                            r#"{"error":"invalid_grant","error_description":"bad verifier"}"#,
                        );
                    }
                    HttpResponse::json(
                        200,
                        &format!(
                            r#"{{"access_token":"at-1","expires_in":3600,"refresh_token":"rt-1","scope":"{DRIVE_APPDATA_SCOPE}","token_type":"Bearer"}}"#
                        ),
                    )
                }
                Some("refresh_token") if form.get("refresh_token").map(String::as_str) == Some("rt-1") => {
                    HttpResponse::json(
                        200,
                        &format!(r#"{{"access_token":"at-2","expires_in":3600,"scope":"{DRIVE_APPDATA_SCOPE}"}}"#),
                    )
                }
                Some("refresh_token") => {
                    HttpResponse::json(400, r#"{"error":"invalid_grant","error_description":"revoked"}"#)
                }
                _ => HttpResponse::json(400, r#"{"error":"unsupported_grant_type"}"#),
            }
        });
        (server, seen)
    }

    fn test_cfg(server: &TestServer) -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: "test-client".into(),
            client_secret: None,
            endpoints: OAuthEndpoints {
                auth_url: format!("{}/auth", server.base_url()),
                token_url: format!("{}/token", server.base_url()),
                revoke_url: format!("{}/revoke", server.base_url()),
            },
        }
    }

    #[test]
    fn code_exchange_enforces_pkce_and_refresh_works() {
        let (server, _) = fake_token_server();
        let cfg = test_cfg(&server);
        let pkce = Pkce::generate();
        agent()
            .post(&format!("{}/challenge", server.base_url()))
            .send_string(pkce.challenge())
            .unwrap();

        // Wrong verifier is rejected.
        let wrong = Pkce::generate();
        assert!(exchange_code(&cfg, "good-code", &wrong, "http://127.0.0.1:1").is_err());

        let t = exchange_code(&cfg, "good-code", &pkce, "http://127.0.0.1:1").unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("rt-1"));

        let session = GoogleSession::new(cfg.clone(), "rt-1");
        assert_eq!(session.access_token().unwrap().as_str(), "at-2");
        // Cached on second call.
        assert_eq!(session.access_token().unwrap().as_str(), "at-2");

        let revoked = GoogleSession::new(cfg, "rt-revoked");
        assert!(matches!(revoked.access_token(), Err(SyncError::AuthRequired(_))));
    }

    #[test]
    fn mobile_redirect_uris() {
        assert_eq!(
            mobile_redirect_uri("1234-abc.apps.googleusercontent.com").unwrap(),
            "com.googleusercontent.apps.1234-abc:/oauth2redirect"
        );
        assert!(mobile_redirect_uri("1234-abc").is_err());
        assert!(mobile_redirect_uri(".apps.googleusercontent.com").is_err());
        assert!(mobile_redirect_uri("evil:x.apps.googleusercontent.com").is_err());
    }

    #[test]
    fn custom_scheme_redirects_are_checked() {
        let uri = "com.googleusercontent.apps.1234-abc:/oauth2redirect";
        let ok = format!("{uri}?state=s1&code=4%2Fsynthetic&scope=x");
        assert_eq!(code_from_redirect(&ok, uri, "s1").unwrap().as_str(), "4/synthetic");
        let code = |r: &str| code_from_redirect(r, uri, "s1").unwrap_err().to_string();
        assert!(code(&format!("{uri}?state=forged&code=x")).contains("state"));
        assert!(code(&format!("{uri}?state=s1&error=access_denied")).contains("access_denied"));
        assert!(code(&format!("{uri}?state=s1")).contains("no authorization code"));
        assert!(code("com.googleusercontent.apps.other:/oauth2redirect?state=s1&code=x").contains("another app"));
        assert!(code("com.googleusercontent.apps.1234-abc:/elsewhere?state=s1&code=x").contains("another app"));
        assert!(code("not a url").contains("malformed"));
    }

    #[test]
    fn custom_scheme_sign_in_end_to_end() {
        let (server, seen) = fake_token_server();
        let cfg = test_cfg(&server);
        let uri = "com.googleusercontent.apps.1234-abc:/oauth2redirect";
        let slot = RedirectSlot::default();
        slot.deliver("stale redirect from an earlier attempt");
        let pending = PendingAuthorization::begin_custom_scheme(&cfg, uri, slot.clone()).unwrap();
        let url = Url::parse(pending.url()).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["redirect_uri"], uri);
        assert!(!q.contains_key("client_secret"));
        agent()
            .post(&format!("{}/challenge", server.base_url()))
            .send_string(&q["code_challenge"])
            .unwrap();
        let state = q["state"].clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            slot.deliver(&format!("{uri}?state={state}&code=good-code"));
        });
        let session = pending
            .complete(Duration::from_secs(10), &AtomicBool::new(false))
            .unwrap();
        t.join().unwrap();
        assert_eq!(session.refresh_token(), "rt-1");
        assert!(seen.lock().unwrap().contains(&"authorization_code".to_string()));
    }

    #[test]
    fn custom_scheme_sign_in_cancel_and_timeout() {
        let cfg = OAuthClientConfig::google("1234-abc.apps.googleusercontent.com", None);
        let uri = mobile_redirect_uri(&cfg.client_id).unwrap();
        let pending = PendingAuthorization::begin_custom_scheme(&cfg, &uri, RedirectSlot::default()).unwrap();
        let r = pending.complete(Duration::from_millis(200), &AtomicBool::new(false));
        assert!(matches!(r, Err(SyncError::Auth(m)) if m.contains("timed out")));

        let pending = PendingAuthorization::begin_custom_scheme(&cfg, &uri, RedirectSlot::default()).unwrap();
        let r = pending.complete(Duration::from_secs(30), &AtomicBool::new(true));
        assert!(matches!(r, Err(SyncError::Cancelled)));
    }

    #[test]
    fn scope_must_be_granted() {
        let t = TokenResponse {
            access_token: "x".into(),
            expires_in: None,
            refresh_token: None,
            scope: Some("openid email".into()),
            token_type: None,
        };
        assert!(t.ensure_scope().is_err());
    }
}
