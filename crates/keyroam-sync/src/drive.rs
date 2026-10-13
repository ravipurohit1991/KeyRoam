//! Google Drive v3 REST client restricted to the hidden `appDataFolder`.

use std::io::Read;
use std::time::Duration;

use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use url::Url;

use crate::error::{Result, SyncError};
use crate::http::{agent, check_endpoint, map_error};
use crate::oauth::AccessTokenSource;
use crate::remote::{RemoteFile, RemoteStore};

pub const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
pub const DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";
const FILE_FIELDS: &str = "id,name,version,createdTime";
/// Refuse to download implausibly large files (a vault is typically < 1 MiB).
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveEndpoints {
    pub api_base: String,
    pub upload_base: String,
}

impl Default for DriveEndpoints {
    fn default() -> Self {
        DriveEndpoints {
            api_base: DRIVE_API_BASE.into(),
            upload_base: DRIVE_UPLOAD_BASE.into(),
        }
    }
}

pub struct DriveClient<T: AccessTokenSource> {
    agent: ureq::Agent,
    tokens: T,
    endpoints: DriveEndpoints,
    retry_base: Duration,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    created_time: String,
}

impl From<DriveFile> for RemoteFile {
    fn from(f: DriveFile) -> Self {
        RemoteFile {
            id: f.id,
            name: f.name,
            version: f.version,
            created_time: f.created_time,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(default)]
    next_page_token: Option<String>,
}

impl<T: AccessTokenSource> DriveClient<T> {
    pub fn new(tokens: T) -> Self {
        Self::with_endpoints(tokens, DriveEndpoints::default()).expect("default endpoints are valid")
    }

    pub fn with_endpoints(tokens: T, endpoints: DriveEndpoints) -> Result<Self> {
        check_endpoint(&endpoints.api_base)?;
        check_endpoint(&endpoints.upload_base)?;
        Ok(DriveClient {
            agent: agent(),
            tokens,
            endpoints,
            retry_base: Duration::from_millis(500),
        })
    }

    #[cfg(test)]
    fn fast_retries(mut self) -> Self {
        self.retry_base = Duration::from_millis(1);
        self
    }

    pub fn tokens(&self) -> &T {
        &self.tokens
    }

    /// Send a request with authentication, refreshing the token once on 401
    /// and backing off on rate limits / transient server errors.
    fn send(
        &self,
        mut call: impl FnMut(&str) -> std::result::Result<ureq::Response, ureq::Error>,
    ) -> Result<ureq::Response> {
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.tokens.access_token()?;
            match call(&format!("Bearer {}", token.as_str())) {
                Ok(r) => return Ok(r),
                Err(ureq::Error::Status(401, _)) if !refreshed => {
                    self.tokens.invalidate();
                    refreshed = true;
                }
                Err(ureq::Error::Status(code, resp))
                    if matches!(code, 429 | 500 | 502 | 503 | 504) && attempt < MAX_ATTEMPTS =>
                {
                    drop(resp);
                    std::thread::sleep(self.retry_base * 2u32.pow(attempt - 1));
                }
                Err(ureq::Error::Status(401, _)) => {
                    return Err(SyncError::AuthRequired("Google rejected the access token".into()))
                }
                Err(e) => return Err(map_error(e)),
            }
        }
    }

    fn files_url(&self, suffix: &str, params: &[(&str, &str)], upload: bool) -> Result<String> {
        let base = if upload {
            &self.endpoints.upload_base
        } else {
            &self.endpoints.api_base
        };
        Url::parse_with_params(&format!("{base}/files{suffix}"), params)
            .map(String::from)
            .map_err(|e| SyncError::Protocol(e.to_string()))
    }
}

/// Drive file ids are URL-safe tokens; reject anything else before putting
/// one into a request path.
fn check_file_id(id: &str) -> Result<()> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Ok(())
    } else {
        Err(SyncError::Protocol(format!("invalid Drive file id {id:?}")))
    }
}

fn parse_file(resp: ureq::Response) -> Result<RemoteFile> {
    resp.into_json::<DriveFile>()
        .map(Into::into)
        .map_err(|e| SyncError::Protocol(format!("file metadata: {e}")))
}

impl<T: AccessTokenSource> RemoteStore for DriveClient<T> {
    fn list(&self) -> Result<Vec<RemoteFile>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut params = vec![
                ("spaces", "appDataFolder"),
                ("q", "trashed = false"),
                ("pageSize", "1000"),
            ];
            let fields = format!("nextPageToken,files({FILE_FIELDS})");
            params.push(("fields", &fields));
            if let Some(p) = &page {
                params.push(("pageToken", p));
            }
            let url = self.files_url("", &params, false)?;
            let resp = self.send(|auth| self.agent.get(&url).set("Authorization", auth).call())?;
            let list: FileList = resp
                .into_json()
                .map_err(|e| SyncError::Protocol(format!("file list: {e}")))?;
            out.extend(list.files.into_iter().map(RemoteFile::from));
            match list.next_page_token {
                Some(t) if !t.is_empty() => page = Some(t),
                _ => return Ok(out),
            }
        }
    }

    fn download(&self, id: &str) -> Result<Vec<u8>> {
        check_file_id(id)?;
        let url = self.files_url(&format!("/{id}"), &[("alt", "media")], false)?;
        let resp = self.send(|auth| self.agent.get(&url).set("Authorization", auth).call())?;
        let mut bytes = Vec::new();
        resp.into_reader()
            .take(MAX_DOWNLOAD + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| SyncError::Network(e.to_string()))?;
        if bytes.len() as u64 > MAX_DOWNLOAD {
            return Err(SyncError::Protocol("remote file too large".into()));
        }
        Ok(bytes)
    }

    fn create(&self, name: &str, bytes: &[u8]) -> Result<RemoteFile> {
        let url = self.files_url("", &[("uploadType", "multipart"), ("fields", FILE_FIELDS)], true)?;
        let metadata = serde_json::json!({
            "name": name,
            "parents": ["appDataFolder"],
            "mimeType": "application/json",
        })
        .to_string();
        let (boundary, body) = multipart_body(&metadata, bytes);
        let content_type = format!("multipart/related; boundary={boundary}");
        let resp = self.send(|auth| {
            self.agent
                .post(&url)
                .set("Authorization", auth)
                .set("Content-Type", &content_type)
                .send_bytes(&body)
        })?;
        parse_file(resp)
    }

    fn update(&self, id: &str, bytes: &[u8]) -> Result<RemoteFile> {
        check_file_id(id)?;
        let url = self.files_url(
            &format!("/{id}"),
            &[("uploadType", "media"), ("fields", FILE_FIELDS)],
            true,
        )?;
        let resp = self.send(|auth| {
            self.agent
                .request("PATCH", &url)
                .set("Authorization", auth)
                .set("Content-Type", "application/json")
                .send_bytes(bytes)
        })?;
        parse_file(resp)
    }
}

fn multipart_body(metadata: &str, content: &[u8]) -> (String, Vec<u8>) {
    loop {
        let mut r = [0u8; 16];
        OsRng.fill_bytes(&mut r);
        let boundary = format!("keyroam_{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let marker = format!("--{boundary}");
        if content.windows(marker.len()).any(|w| w == marker.as_bytes()) {
            continue;
        }
        let mut body = Vec::with_capacity(content.len() + metadata.len() + 256);
        body.extend_from_slice(
            format!("{marker}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n").as_bytes(),
        );
        body.extend_from_slice(format!("{marker}\r\nContent-Type: application/json\r\n\r\n").as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n{marker}--\r\n").as_bytes());
        return (boundary, body);
    }
}

/// Test support: a faithful-enough local stand-in for the Drive v3 endpoints
/// KeyRoam uses, enforcing bearer auth and the appDataFolder parent.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use crate::test_server::{HttpRequest, HttpResponse, TestServer};
    use std::sync::{Arc, Mutex};
    use zeroize::Zeroizing;

    #[derive(Clone, Debug)]
    pub struct FakeFile {
        pub id: String,
        pub name: String,
        pub version: u64,
        pub created: u64,
        pub content: Vec<u8>,
    }

    #[derive(Default)]
    pub struct FakeDriveState {
        pub files: Vec<FakeFile>,
        pub next_id: u64,
        pub valid_token: String,
        /// Status codes to return for the next N requests.
        pub inject: Vec<u16>,
        pub requests: usize,
    }

    pub fn start(valid_token: &str) -> (TestServer, Arc<Mutex<FakeDriveState>>) {
        let state = Arc::new(Mutex::new(FakeDriveState {
            valid_token: valid_token.into(),
            ..Default::default()
        }));
        let s = state.clone();
        let server = TestServer::start(move |req| handle(&mut s.lock().unwrap(), req));
        (server, state)
    }

    pub fn endpoints(server: &TestServer) -> DriveEndpoints {
        DriveEndpoints {
            api_base: format!("{}/drive/v3", server.base_url()),
            upload_base: format!("{}/upload/drive/v3", server.base_url()),
        }
    }

    fn meta(f: &FakeFile) -> serde_json::Value {
        serde_json::json!({
            "id": f.id, "name": f.name, "version": f.version.to_string(),
            "createdTime": format!("2026-01-01T00:00:{:02}.000Z", f.created % 60),
        })
    }

    fn handle(st: &mut FakeDriveState, req: HttpRequest) -> HttpResponse {
        st.requests += 1;
        if !st.inject.is_empty() {
            let code = st.inject.remove(0);
            return HttpResponse::json(code, r#"{"error":{"code":0,"message":"injected"}}"#);
        }
        if req.header("authorization") != Some(&format!("Bearer {}", st.valid_token)) {
            return HttpResponse::json(401, r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#);
        }
        let path = req.path.clone();
        match (req.method.as_str(), path.as_str()) {
            ("GET", "/drive/v3/files") => {
                if req.query("spaces") != Some("appDataFolder") {
                    return HttpResponse::json(400, r#"{"error":{"message":"wrong space"}}"#);
                }
                let files: Vec<_> = st.files.iter().map(meta).collect();
                // Two items per page to exercise pagination.
                let start: usize = req.query("pageToken").and_then(|p| p.parse().ok()).unwrap_or(0);
                let page: Vec<_> = files.iter().skip(start).take(2).cloned().collect();
                let mut body = serde_json::json!({ "files": page });
                if start + 2 < files.len() {
                    body["nextPageToken"] = (start + 2).to_string().into();
                }
                HttpResponse::json(200, &body.to_string())
            }
            ("GET", p) if p.starts_with("/drive/v3/files/") => {
                let id = &p["/drive/v3/files/".len()..];
                match st.files.iter().find(|f| f.id == id) {
                    Some(f) if req.query("alt") == Some("media") => HttpResponse::bytes(200, f.content.clone()),
                    Some(f) => HttpResponse::json(200, &meta(f).to_string()),
                    None => HttpResponse::json(404, r#"{"error":{"message":"File not found"}}"#),
                }
            }
            ("POST", "/upload/drive/v3/files") => {
                let ct = req.header("content-type").unwrap_or("").to_string();
                let Some(boundary) = ct.split("boundary=").nth(1) else {
                    return HttpResponse::json(400, r#"{"error":{"message":"no boundary"}}"#);
                };
                let marker = format!("--{boundary}");
                let body = Zeroizing::new(req.body.clone());
                let parts = split(&body, marker.as_bytes());
                if parts.len() < 2 {
                    return HttpResponse::json(400, r#"{"error":{"message":"bad multipart"}}"#);
                }
                let part_body = |p: &[u8]| -> Vec<u8> {
                    let start = p.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let mut b = p[start..].to_vec();
                    if b.ends_with(b"\r\n") {
                        b.truncate(b.len() - 2);
                    }
                    b
                };
                let metadata: serde_json::Value = serde_json::from_slice(&part_body(parts[0])).unwrap();
                if metadata["parents"] != serde_json::json!(["appDataFolder"]) {
                    return HttpResponse::json(403, r#"{"error":{"message":"must be in appDataFolder"}}"#);
                }
                st.next_id += 1;
                let f = FakeFile {
                    id: format!("file_{}", st.next_id),
                    name: metadata["name"].as_str().unwrap().to_string(),
                    version: 1,
                    created: st.next_id,
                    content: part_body(parts[1]),
                };
                let m = meta(&f);
                st.files.push(f);
                HttpResponse::json(200, &m.to_string())
            }
            ("PATCH", p) if p.starts_with("/upload/drive/v3/files/") => {
                let id = &p["/upload/drive/v3/files/".len()..];
                match st.files.iter_mut().find(|f| f.id == id) {
                    Some(f) => {
                        f.content = req.body.clone();
                        f.version += 1;
                        HttpResponse::json(200, &meta(f).to_string())
                    }
                    None => HttpResponse::json(404, r#"{"error":{"message":"File not found"}}"#),
                }
            }
            _ => HttpResponse::json(404, r#"{"error":{"message":"no route"}}"#),
        }
    }

    fn split<'a>(body: &'a [u8], marker: &[u8]) -> Vec<&'a [u8]> {
        let mut parts = Vec::new();
        let mut idx = Vec::new();
        let mut i = 0;
        while i + marker.len() <= body.len() {
            if &body[i..i + marker.len()] == marker {
                idx.push(i);
                i += marker.len();
            } else {
                i += 1;
            }
        }
        for w in idx.windows(2) {
            let start = w[0] + marker.len() + 2; // skip CRLF after marker
            parts.push(&body[start.min(w[1])..w[1]]);
        }
        parts
    }

    /// Token source returning a fixed (synthetic) access token. The first
    /// token handed out can be made stale to exercise the 401 → refresh path.
    pub struct StaticToken {
        pub tokens: Mutex<Vec<String>>,
        pub invalidations: Mutex<usize>,
    }

    impl StaticToken {
        pub fn new(seq: &[&str]) -> Self {
            StaticToken {
                tokens: Mutex::new(seq.iter().rev().map(|s| s.to_string()).collect()),
                invalidations: Mutex::new(0),
            }
        }
    }

    impl AccessTokenSource for StaticToken {
        fn access_token(&self) -> Result<Zeroizing<String>> {
            let t = self.tokens.lock().unwrap();
            Ok(Zeroizing::new(t.last().cloned().unwrap_or_default()))
        }
        fn invalidate(&self) {
            *self.invalidations.lock().unwrap() += 1;
            let mut t = self.tokens.lock().unwrap();
            if t.len() > 1 {
                t.pop();
            }
        }
    }

    pub fn client(server: &TestServer, token: StaticToken) -> DriveClient<StaticToken> {
        DriveClient::with_endpoints(token, endpoints(server))
            .unwrap()
            .fast_retries()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn crud_roundtrip_with_pagination() {
        let (server, _) = start("tok");
        let c = client(&server, StaticToken::new(&["tok"]));
        assert!(c.list().unwrap().is_empty());
        let mut ids = Vec::new();
        for i in 0..5 {
            let content = format!("{{\"n\":{i},\"tricky\":\"--keyroam_\\r\\n\"}}");
            let f = c.create(&format!("keyroam.test.{i}"), content.as_bytes()).unwrap();
            assert_eq!(f.version, "1");
            ids.push(f.id);
        }
        let listed = c.list().unwrap();
        assert_eq!(listed.len(), 5);
        assert_eq!(
            c.download(&ids[2]).unwrap(),
            b"{\"n\":2,\"tricky\":\"--keyroam_\\r\\n\"}".to_vec()
        );
        let u = c.update(&ids[2], b"new").unwrap();
        assert_eq!(u.version, "2");
        assert_eq!(c.download(&ids[2]).unwrap(), b"new");
    }

    #[test]
    fn refreshes_token_on_401() {
        let (server, _) = start("fresh");
        let tokens = StaticToken::new(&["stale", "fresh"]);
        let c = client(&server, tokens);
        assert!(c.list().unwrap().is_empty());
        assert_eq!(*c.tokens().invalidations.lock().unwrap(), 1);

        let (server, _) = start("other");
        let c = client(&server, StaticToken::new(&["stale"]));
        assert!(matches!(c.list(), Err(SyncError::AuthRequired(_))));
    }

    #[test]
    fn retries_transient_errors_then_gives_up() {
        let (server, state) = start("tok");
        let c = client(&server, StaticToken::new(&["tok"]));
        state.lock().unwrap().inject = vec![503, 429];
        assert!(c.list().is_ok());

        state.lock().unwrap().inject = vec![500; 10];
        assert!(matches!(c.list(), Err(SyncError::Http { status: 500, .. })));

        state.lock().unwrap().inject = vec![403];
        let err = c.list().unwrap_err();
        assert!(matches!(err, SyncError::Http { status: 403, .. }));
        assert!(err.to_string().contains("injected"));
    }

    #[test]
    fn offline_is_reported_as_network_error() {
        let endpoints = DriveEndpoints {
            api_base: "http://127.0.0.1:9/drive/v3".into(),
            upload_base: "http://127.0.0.1:9/upload/drive/v3".into(),
        };
        let c = DriveClient::with_endpoints(StaticToken::new(&["t"]), endpoints).unwrap();
        let err = c.list().unwrap_err();
        assert!(err.is_offline(), "{err}");
    }

    #[test]
    fn rejects_bad_ids_and_insecure_endpoints() {
        let (server, _) = start("tok");
        let c = client(&server, StaticToken::new(&["tok"]));
        assert!(matches!(c.download("../etc"), Err(SyncError::Protocol(_))));
        assert!(DriveClient::with_endpoints(
            StaticToken::new(&["t"]),
            DriveEndpoints {
                api_base: "http://example.com".into(),
                upload_base: DRIVE_UPLOAD_BASE.into()
            }
        )
        .is_err());
    }
}
