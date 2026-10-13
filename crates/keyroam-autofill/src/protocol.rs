//! The browser-extension protocol, handled by the desktop app.
//!
//! The extension sends JSON requests through its native-messaging host, which
//! relays them unchanged over the local bridge. Every request carries a `rid`
//! that is echoed in the response:
//!
//! ```text
//! {"rid": 7, "type": "logins", "page": {"url": "https://login.example.com/", "top_url": "https://login.example.com/"}}
//! {"rid": 7, "ok": {"origin": "https://login.example.com", "logins": [{"id": "…", "title": "…", "username": "…", "match": "exact"}]}}
//! {"rid": 8, "error": {"code": "locked", "message": "KeyRoam is locked"}}
//! ```
//!
//! | `type` | Parameters | Result |
//! |---|---|---|
//! | `hello` | | `app`, `protocol`, `unlocked` |
//! | `logins` | `page` | `origin`, `logins` (no passwords) |
//! | `credential` | `page`, `id` | `id`, `username`, `password` |
//! | `check_save` | `page`, `username`, `password` | `action` (`new`, `update`, `none`), `id`, `title`, `site` |
//! | `save` | `page`, `username`, `password`, optional `update_id` | `id`, `updated` |
//! | `generate` | optional `length` | `password` |
//!
//! `page` describes where the request comes from: `url` is the frame holding
//! the form and `top_url` the tab's page. The extension derives both from the
//! browser, never from page content. Credentials are released only to https
//! pages whose frame belongs to the same site as the tab, unless the user
//! explicitly consented for this request (`allow_http`, `allow_cross_site`).
//! The engine re-checks every rule; the extension is not trusted to.

use keyroam_core::generator::{generate_password, GeneratorOptions};
use keyroam_core::{EntryId, Vault};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::matching::{apply_save, credential_for, find_logins, plan_save, SavePlan};
use crate::origin::{OriginError, WebOrigin};

pub const PROTOCOL_VERSION: u32 = 1;

/// Error codes. `app_not_running` is produced by the native-messaging host.
pub mod code {
    pub const INVALID_REQUEST: &str = "invalid_request";
    pub const LOCKED: &str = "locked";
    pub const UNSUPPORTED_PAGE: &str = "unsupported_page";
    pub const INSECURE_PAGE: &str = "insecure_page";
    pub const CROSS_SITE_FRAME: &str = "cross_site_frame";
    pub const NOT_FOUND: &str = "not_found";
    pub const FAILED: &str = "failed";
    pub const APP_NOT_RUNNING: &str = "app_not_running";
}

#[derive(Debug, Deserialize)]
pub struct PageInfo {
    pub url: String,
    pub top_url: String,
    #[serde(default)]
    pub allow_http: bool,
    #[serde(default)]
    pub allow_cross_site: bool,
}

/// A string that is wiped when dropped.
#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello,
    Logins {
        page: PageInfo,
    },
    Credential {
        page: PageInfo,
        id: String,
    },
    CheckSave {
        page: PageInfo,
        username: String,
        password: Secret,
    },
    Save {
        page: PageInfo,
        username: String,
        password: Secret,
        #[serde(default)]
        update_id: Option<String>,
    },
    Generate {
        #[serde(default)]
        length: Option<usize>,
    },
}

impl Request {
    /// True for requests that change the vault (the caller commits them).
    pub fn is_write(&self) -> bool {
        matches!(self, Request::Save { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

type Reply = Result<Value, ProtocolError>;

/// Applies a vault change durably (in the app: under the data-directory
/// lock, then persisted and queued for sync).
pub trait Committer {
    fn commit(
        &mut self,
        vault: &mut Vault,
        change: &mut dyn FnMut(&mut Vault) -> keyroam_core::Result<(EntryId, bool)>,
    ) -> Result<(EntryId, bool), String>;
}

/// The response for a request that could not be parsed or routed.
pub fn error_response(rid: &Value, code: &str, message: &str) -> Vec<u8> {
    json!({ "rid": rid, "error": { "code": code, "message": message } })
        .to_string()
        .into_bytes()
}

/// Extract the `rid` of a request without parsing the rest (used to answer
/// requests that fail early, for example when the app is not running).
pub fn request_id(request: &[u8]) -> Value {
    #[derive(Deserialize)]
    struct Rid {
        #[serde(default)]
        rid: Value,
    }
    serde_json::from_slice::<Rid>(request)
        .map(|r| r.rid)
        .unwrap_or(Value::Null)
}

/// Parse a request. On failure the error response is returned instead.
pub fn parse_request(request: &[u8]) -> Result<(Value, Request), Vec<u8>> {
    let rid = request_id(request);
    match serde_json::from_slice::<Request>(request) {
        Ok(r) => Ok((rid, r)),
        Err(e) => Err(error_response(&rid, code::INVALID_REQUEST, &e.to_string())),
    }
}

/// Handle a parsed request. `vault` is `None` while KeyRoam is locked.
/// The response bytes may contain a password; callers wipe them after use.
pub fn handle(rid: &Value, request: Request, vault: Option<&mut Vault>, committer: &mut dyn Committer) -> Vec<u8> {
    let reply = match vault {
        _ if matches!(request, Request::Generate { .. }) => generate(&request),
        None => match request {
            Request::Hello => Ok(hello(false)),
            _ => Err(ProtocolError::new(code::LOCKED, "KeyRoam is locked")),
        },
        Some(vault) => dispatch(request, vault, committer),
    };
    let body = match reply {
        Ok(ok) => json!({ "rid": rid, "ok": ok }),
        Err(e) => json!({ "rid": rid, "error": { "code": e.code, "message": e.message } }),
    };
    let text = Zeroizing::new(body.to_string());
    text.as_bytes().to_vec()
}

fn hello(unlocked: bool) -> Value {
    json!({ "app": "KeyRoam", "protocol": PROTOCOL_VERSION, "unlocked": unlocked })
}

fn generate(request: &Request) -> Reply {
    let Request::Generate { length } = request else {
        unreachable!()
    };
    let pw = generate_password(&GeneratorOptions {
        length: length.unwrap_or(20),
        ..Default::default()
    })
    .map_err(|e| ProtocolError::new(code::INVALID_REQUEST, e.to_string()))?;
    Ok(json!({ "password": pw.as_str() }))
}

fn parse_page(url: &str) -> Result<WebOrigin, ProtocolError> {
    WebOrigin::parse(url).map_err(|e| match e {
        OriginError::Invalid => ProtocolError::new(code::INVALID_REQUEST, e.to_string()),
        OriginError::UnsupportedScheme => ProtocolError::new(code::UNSUPPORTED_PAGE, e.to_string()),
    })
}

/// The origin of the frame holding the form, after applying the release
/// policy: https only, and the frame must belong to the tab's site.
fn release_origin(page: &PageInfo) -> Result<WebOrigin, ProtocolError> {
    let frame = parse_page(&page.url)?;
    let top = parse_page(&page.top_url)?;
    if (!frame.is_secure() || !top.is_secure()) && !page.allow_http {
        return Err(ProtocolError::new(
            code::INSECURE_PAGE,
            "this page is not secure (http); filling needs your confirmation",
        ));
    }
    if !frame.same_site(&top) && !page.allow_cross_site {
        return Err(ProtocolError::new(
            code::CROSS_SITE_FRAME,
            format!("the form is in an embedded frame from {frame}; filling needs your confirmation"),
        ));
    }
    Ok(frame)
}

fn parse_id(id: &str) -> Result<EntryId, ProtocolError> {
    id.parse()
        .map_err(|_| ProtocolError::new(code::INVALID_REQUEST, "invalid entry id"))
}

fn dispatch(request: Request, vault: &mut Vault, committer: &mut dyn Committer) -> Reply {
    match request {
        Request::Hello => Ok(hello(true)),
        Request::Generate { .. } => unreachable!("handled before"),
        Request::Logins { page } => {
            let origin = release_origin(&page)?;
            let logins: Vec<Value> = find_logins(vault, &origin)
                .into_iter()
                .map(|m| {
                    json!({
                        "id": m.id.to_string(),
                        "title": m.title,
                        "username": m.username,
                        "match": m.kind.as_str(),
                    })
                })
                .collect();
            Ok(json!({ "origin": origin.to_string(), "logins": logins }))
        }
        Request::Credential { page, id } => {
            let origin = release_origin(&page)?;
            let id = parse_id(&id)?;
            let data = credential_for(vault, id, &origin)
                .ok_or_else(|| ProtocolError::new(code::NOT_FOUND, "no such login for this site"))?;
            Ok(json!({ "id": id.to_string(), "username": data.username, "password": data.password }))
        }
        Request::CheckSave {
            page,
            username,
            password,
        } => {
            let origin = parse_page(&page.url)?;
            let site = origin.site().unwrap_or(origin.host()).to_string();
            Ok(match plan_save(vault, &origin, &username, &password.0) {
                SavePlan::New => json!({ "action": "new", "site": site }),
                SavePlan::Update { id, title } => {
                    json!({ "action": "update", "id": id.to_string(), "title": title, "site": site })
                }
                SavePlan::Unchanged { id } => json!({ "action": "none", "id": id.to_string(), "site": site }),
            })
        }
        Request::Save {
            page,
            username,
            password,
            update_id,
        } => {
            let origin = parse_page(&page.url)?;
            let update_id = update_id.as_deref().map(parse_id).transpose()?;
            let (id, updated) = committer
                .commit(vault, &mut |v| {
                    apply_save(v, &origin, &username, &password.0, update_id)
                })
                .map_err(|m| ProtocolError::new(code::FAILED, m))?;
            Ok(json!({ "id": id.to_string(), "updated": updated }))
        }
    }
}

#[cfg(test)]
mod tests {
    use keyroam_core::{EntryData, KdfCost};

    use super::*;

    struct Direct;
    impl Committer for Direct {
        fn commit(
            &mut self,
            vault: &mut Vault,
            change: &mut dyn FnMut(&mut Vault) -> keyroam_core::Result<(EntryId, bool)>,
        ) -> Result<(EntryId, bool), String> {
            change(vault).map_err(|e| e.to_string())
        }
    }

    fn call(vault: Option<&mut Vault>, req: Value) -> Value {
        let bytes = req.to_string().into_bytes();
        let out = match parse_request(&bytes) {
            Ok((rid, r)) => handle(&rid, r, vault, &mut Direct),
            Err(e) => e,
        };
        serde_json::from_slice(&out).unwrap()
    }

    fn page(url: &str, top: &str) -> Value {
        json!({ "url": url, "top_url": top })
    }

    fn vault() -> (Vault, EntryId) {
        let mut v = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
        let id = v
            .add_entry(EntryData {
                title: "Example".into(),
                username: "alice".into(),
                password: "synthetic-secret".into(),
                urls: vec!["https://example.com".into()],
                ..Default::default()
            })
            .unwrap();
        (v, id)
    }

    #[test]
    fn locked_app_reveals_nothing() {
        let r = call(None, json!({ "rid": 1, "type": "hello" }));
        assert_eq!(r["rid"], 1);
        assert_eq!(r["ok"]["unlocked"], false);
        let p = page("https://example.com", "https://example.com");
        let r = call(None, json!({ "rid": "x", "type": "logins", "page": p }));
        assert_eq!(r["rid"], "x");
        assert_eq!(r["error"]["code"], code::LOCKED);
        // The generator does not need the vault.
        let r = call(None, json!({ "rid": 2, "type": "generate", "length": 24 }));
        assert_eq!(r["ok"]["password"].as_str().unwrap().chars().count(), 24);
    }

    #[test]
    fn logins_and_credentials_follow_the_release_policy() {
        let (mut v, id) = vault();
        let ok = page("https://login.example.com/x", "https://login.example.com/");
        let r = call(Some(&mut v), json!({ "rid": 1, "type": "logins", "page": ok }));
        assert_eq!(r["ok"]["origin"], "https://login.example.com");
        assert_eq!(r["ok"]["logins"][0]["id"], id.to_string());
        assert_eq!(r["ok"]["logins"][0]["match"], "same_site");
        assert!(!r.to_string().contains("synthetic-secret"));

        let r = call(
            Some(&mut v),
            json!({ "rid": 2, "type": "credential", "page": ok, "id": id.to_string() }),
        );
        assert_eq!(r["ok"]["password"], "synthetic-secret");

        // Another site cannot obtain it, even by id.
        let other = page("https://example.org", "https://example.org");
        let r = call(
            Some(&mut v),
            json!({ "rid": 3, "type": "credential", "page": other, "id": id.to_string() }),
        );
        assert_eq!(r["error"]["code"], code::NOT_FOUND);

        // A same-site form inside a third-party page needs consent.
        let framed = page("https://example.com/login", "https://evil.test/");
        let r = call(
            Some(&mut v),
            json!({ "rid": 4, "type": "credential", "page": framed, "id": id.to_string() }),
        );
        assert_eq!(r["error"]["code"], code::CROSS_SITE_FRAME);
        let mut consented = framed.clone();
        consented["allow_cross_site"] = json!(true);
        let r = call(
            Some(&mut v),
            json!({ "rid": 5, "type": "credential", "page": consented, "id": id.to_string() }),
        );
        assert_eq!(r["ok"]["username"], "alice");

        // http pages need consent, and https-saved logins are never offered there.
        let mut v2 = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
        let http_id = v2
            .add_entry(EntryData {
                title: "Router".into(),
                password: "synthetic-router".into(),
                urls: vec!["http://192.168.1.1".into()],
                ..Default::default()
            })
            .unwrap();
        let plain = page("http://192.168.1.1/", "http://192.168.1.1/");
        let r = call(Some(&mut v2), json!({ "rid": 6, "type": "logins", "page": plain }));
        assert_eq!(r["error"]["code"], code::INSECURE_PAGE);
        let mut plain_ok = plain.clone();
        plain_ok["allow_http"] = json!(true);
        let r = call(Some(&mut v2), json!({ "rid": 7, "type": "logins", "page": plain_ok }));
        assert_eq!(r["ok"]["logins"][0]["id"], http_id.to_string());
        let mut http_example = page("http://example.com", "http://example.com");
        http_example["allow_http"] = json!(true);
        let r = call(
            Some(&mut v),
            json!({ "rid": 8, "type": "logins", "page": http_example }),
        );
        assert_eq!(r["ok"]["logins"].as_array().unwrap().len(), 0);

        let r = call(
            Some(&mut v),
            json!({ "rid": 9, "type": "logins", "page": page("chrome://settings", "chrome://settings") }),
        );
        assert_eq!(r["error"]["code"], code::UNSUPPORTED_PAGE);
    }

    #[test]
    fn check_and_save() {
        let (mut v, id) = vault();
        let p = page("https://example.com/login", "https://example.com/login");
        let check = |v: &mut Vault, user: &str, pw: &str| {
            call(
                Some(v),
                json!({ "rid": 1, "type": "check_save", "page": p, "username": user, "password": pw }),
            )
        };
        assert_eq!(check(&mut v, "alice", "synthetic-secret")["ok"]["action"], "none");
        let r = check(&mut v, "alice", "rotated");
        assert_eq!(r["ok"]["action"], "update");
        assert_eq!(r["ok"]["id"], id.to_string());
        let r = check(&mut v, "bob", "bob-secret");
        assert_eq!(r["ok"]["action"], "new");
        assert_eq!(r["ok"]["site"], "example.com");

        let r = call(
            Some(&mut v),
            json!({ "rid": 2, "type": "save", "page": p, "username": "alice", "password": "rotated", "update_id": id.to_string() }),
        );
        assert_eq!(r["ok"]["updated"], true);
        let r = call(
            Some(&mut v),
            json!({ "rid": 3, "type": "save", "page": p, "username": "bob", "password": "bob-secret" }),
        );
        assert_eq!(r["ok"]["updated"], false);
        assert_eq!(v.entries().count(), 2);
        assert_eq!(check(&mut v, "alice", "rotated")["ok"]["action"], "none");
    }

    #[test]
    fn malformed_requests() {
        let (mut v, _) = vault();
        let r = call(Some(&mut v), json!({ "rid": 1, "type": "unknown" }));
        assert_eq!(r["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(r["rid"], 1);
        let out = parse_request(b"not json").unwrap_err();
        let r: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(r["rid"], Value::Null);
        let p = page("https://example.com", "https://example.com");
        let r = call(
            Some(&mut v),
            json!({ "rid": 2, "type": "credential", "page": p, "id": "nope" }),
        );
        assert_eq!(r["error"]["code"], code::INVALID_REQUEST);
    }
}
