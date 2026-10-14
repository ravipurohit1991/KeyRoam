//! Answers autofill requests against the vault held by the desktop app.
//!
//! Every request names the page it is for, and every answer is limited to
//! logins saved for that page's site. Passwords are returned only by `fill`,
//! for one entry the user picked, and only after the entry is re-checked
//! against the page. Insecure pages and cross-origin frames need the user's
//! explicit consent for each fill.

use keyroam_core::{EntryData, EntryId, EntryRecord, Vault};
use serde_json::{json, Value};

use crate::origin::{best_match, FillContext, MatchKind, OriginError};
use crate::protocol::{codes, Command, ProtocolError, Secret, MAX_LOGINS, PROTOCOL_VERSION};

/// The app's vault, as seen by an autofill request.
pub enum VaultAccess<'a> {
    NoVault,
    Locked,
    Unlocked(&'a mut Vault),
}

/// The result of a request: the response body, and whether the vault changed
/// (the caller must then persist it and schedule a sync).
#[derive(Debug)]
pub struct Outcome {
    pub body: Value,
    pub changed: bool,
}

impl Outcome {
    fn read(body: Value) -> Self {
        Outcome { body, changed: false }
    }
}

type Result<T> = std::result::Result<T, ProtocolError>;

impl From<OriginError> for ProtocolError {
    fn from(e: OriginError) -> Self {
        ProtocolError::new(codes::NOT_FILLABLE, e.to_string())
    }
}

impl From<keyroam_core::Error> for ProtocolError {
    fn from(e: keyroam_core::Error) -> Self {
        match e {
            keyroam_core::Error::EntryNotFound => ProtocolError::new(codes::NOT_FOUND, "no such login"),
            keyroam_core::Error::InvalidInput(m) => ProtocolError::new(codes::INVALID_REQUEST, m),
            other => ProtocolError::new(codes::INTERNAL, other.to_string()),
        }
    }
}

fn unlocked<'a>(access: VaultAccess<'a>) -> Result<&'a mut Vault> {
    match access {
        VaultAccess::Unlocked(v) => Ok(v),
        VaultAccess::Locked => Err(ProtocolError::new(codes::LOCKED, "KeyRoam is locked")),
        VaultAccess::NoVault => Err(ProtocolError::new(codes::NO_VAULT, "KeyRoam has no vault yet")),
    }
}

/// The content shown for an entry: its newest version, if not deleted.
fn live_data(rec: &EntryRecord) -> Option<&EntryData> {
    if rec.is_deleted() {
        return None;
    }
    rec.current().data.as_ref()
}

/// Live entries saved for the frame's site, best matches first.
fn matching<'v>(vault: &'v Vault, ctx: &FillContext) -> Vec<(MatchKind, &'v EntryRecord, &'v EntryData)> {
    let mut out: Vec<_> = vault
        .entries()
        .filter_map(|rec| {
            let data = live_data(rec)?;
            Some((best_match(&ctx.frame, &data.urls)?, rec, data))
        })
        .collect();
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.2.favorite.cmp(&a.2.favorite))
            .then_with(|| a.2.title.to_lowercase().cmp(&b.2.title.to_lowercase()))
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    out
}

fn parse_id(s: &str) -> Result<EntryId> {
    s.parse()
        .map_err(|_| ProtocolError::new(codes::INVALID_REQUEST, "invalid entry id"))
}

/// One live entry that is saved for the frame's site.
fn matching_entry<'v>(vault: &'v Vault, ctx: &FillContext, id: &str) -> Result<&'v EntryData> {
    let rec = vault
        .get(parse_id(id)?)
        .ok_or_else(|| ProtocolError::new(codes::NOT_FOUND, "no such login"))?;
    let data = live_data(rec).ok_or_else(|| ProtocolError::new(codes::NOT_FOUND, "no such login"))?;
    if best_match(&ctx.frame, &data.urls).is_none() {
        return Err(ProtocolError::new(
            codes::NO_MATCH,
            format!("this login is not saved for {}", ctx.frame.site),
        ));
    }
    Ok(data)
}

fn context_json(ctx: &FillContext) -> Value {
    json!({
        "origin": ctx.frame.serialize(),
        "site": ctx.frame.site,
        "top_origin": ctx.top.serialize(),
        "secure": ctx.secure(),
        "cross_origin": ctx.cross_origin(),
        "needs_consent": ctx.needs_consent(),
    })
}

/// What a submitted login means for the vault.
enum SaveAction<'v> {
    /// Already saved with this password.
    None(&'v EntryRecord),
    /// Saved with another password.
    Update(&'v EntryRecord, &'v EntryData),
    /// Not saved yet.
    Save,
}

fn classify<'v>(vault: &'v Vault, ctx: &FillContext, username: &str, password: &str) -> SaveAction<'v> {
    let same_user = matching(vault, ctx)
        .into_iter()
        .filter(|(_, _, d)| d.username.trim() == username.trim())
        .collect::<Vec<_>>();
    if let Some((_, rec, _)) = same_user.iter().find(|(_, _, d)| d.password == password) {
        return SaveAction::None(rec);
    }
    match same_user.first() {
        Some((_, rec, data)) => SaveAction::Update(rec, data),
        None => SaveAction::Save,
    }
}

/// Handle one request.
pub fn handle(command: &Command, access: VaultAccess<'_>) -> Result<Outcome> {
    match command {
        Command::Status {} => {
            let state = match access {
                VaultAccess::NoVault => "no_vault",
                VaultAccess::Locked => "locked",
                VaultAccess::Unlocked(_) => "unlocked",
            };
            Ok(Outcome::read(json!({
                "state": state,
                "protocol": PROTOCOL_VERSION,
                "app_version": env!("CARGO_PKG_VERSION"),
            })))
        }
        Command::List { url, top_url } => {
            let ctx = FillContext::new(url, top_url)?;
            let vault = unlocked(access)?;
            let logins: Vec<Value> = matching(vault, &ctx)
                .into_iter()
                .take(MAX_LOGINS)
                .map(|(kind, rec, data)| {
                    json!({
                        "id": rec.id.to_string(),
                        "title": data.title,
                        "username": data.username,
                        "match": kind.as_str(),
                    })
                })
                .collect();
            let mut body = context_json(&ctx);
            body["logins"] = Value::Array(logins);
            Ok(Outcome::read(body))
        }
        Command::Fill {
            url,
            top_url,
            entry_id,
            consent,
        } => {
            let ctx = FillContext::new(url, top_url)?;
            let vault = unlocked(access)?;
            let data = matching_entry(vault, &ctx, entry_id)?;
            if ctx.needs_consent() && !consent {
                let why = if !ctx.secure() {
                    "this page is not secure (HTTP)"
                } else {
                    "the login form belongs to another site embedded in this page"
                };
                return Err(ProtocolError::new(
                    codes::CONSENT_REQUIRED,
                    format!("confirm the fill: {why}"),
                ));
            }
            Ok(Outcome::read(json!({
                "username": data.username,
                "password": data.password,
            })))
        }
        Command::CheckSave {
            url,
            top_url,
            username,
            password,
        } => {
            let ctx = FillContext::new(url, top_url)?;
            let vault = unlocked(access)?;
            let mut body = json!({ "site": ctx.frame.site, "action": "none" });
            if password.expose().is_empty() {
                return Ok(Outcome::read(body));
            }
            match classify(vault, &ctx, username, password.expose()) {
                SaveAction::None(rec) => body["entry_id"] = json!(rec.id.to_string()),
                SaveAction::Update(rec, data) => {
                    body["action"] = json!("update");
                    body["entry_id"] = json!(rec.id.to_string());
                    body["title"] = json!(data.title);
                }
                SaveAction::Save => body["action"] = json!("save"),
            }
            Ok(Outcome::read(body))
        }
        Command::Save {
            url,
            top_url,
            username,
            password,
            entry_id,
        } => {
            let ctx = FillContext::new(url, top_url)?;
            let vault = unlocked(access)?;
            save(vault, &ctx, username, password, entry_id.as_deref())
        }
    }
}

fn save(
    vault: &mut Vault,
    ctx: &FillContext,
    username: &str,
    password: &Secret,
    entry_id: Option<&str>,
) -> Result<Outcome> {
    if password.expose().is_empty() {
        return Err(ProtocolError::new(codes::INVALID_REQUEST, "the password is empty"));
    }
    let username = username.trim();
    if let Some(id) = entry_id {
        // Only an entry saved for this site may be updated from this page.
        let mut data = matching_entry(vault, ctx, id)?.clone();
        if data.password == password.expose() && data.username.trim() == username {
            return Ok(Outcome {
                body: json!({ "result": "unchanged", "entry_id": id }),
                changed: false,
            });
        }
        data.password = password.expose().to_string();
        if data.username.trim().is_empty() {
            data.username = username.to_string();
        }
        vault.update_entry(parse_id(id)?, data)?;
        return Ok(Outcome {
            body: json!({ "result": "updated", "entry_id": id }),
            changed: true,
        });
    }
    if let SaveAction::None(rec) = classify(vault, ctx, username, password.expose()) {
        return Ok(Outcome {
            body: json!({ "result": "unchanged", "entry_id": rec.id.to_string() }),
            changed: false,
        });
    }
    let id = vault.add_entry(EntryData {
        title: ctx.frame.site.clone(),
        username: username.to_string(),
        password: password.expose().to_string(),
        urls: vec![ctx.frame.serialize()],
        ..Default::default()
    })?;
    Ok(Outcome {
        body: json!({ "result": "created", "entry_id": id.to_string() }),
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use keyroam_core::KdfCost;

    use super::*;

    const PW: &str = "synthetic-master-pw-1";

    fn vault() -> Vault {
        Vault::create(PW, KdfCost::MINIMUM).unwrap()
    }

    fn add(v: &mut Vault, title: &str, user: &str, pw: &str, urls: &[&str]) -> String {
        v.add_entry(EntryData {
            title: title.into(),
            username: user.into(),
            password: pw.into(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
            ..Default::default()
        })
        .unwrap()
        .to_string()
    }

    fn list(url: &str, top: &str) -> Command {
        Command::List {
            url: url.into(),
            top_url: top.into(),
        }
    }

    fn fill(url: &str, top: &str, id: &str, consent: bool) -> Command {
        Command::Fill {
            url: url.into(),
            top_url: top.into(),
            entry_id: id.into(),
            consent,
        }
    }

    fn run(v: &mut Vault, c: Command) -> Result<Outcome> {
        handle(&c, VaultAccess::Unlocked(v))
    }

    #[test]
    fn status_reports_vault_state() {
        let state = |a| handle(&Command::Status {}, a).unwrap().body["state"].clone();
        assert_eq!(state(VaultAccess::NoVault), "no_vault");
        assert_eq!(state(VaultAccess::Locked), "locked");
        let mut v = vault();
        assert_eq!(state(VaultAccess::Unlocked(&mut v)), "unlocked");
    }

    #[test]
    fn locked_or_missing_vault_refuses_everything_else() {
        let c = list("https://example.com/", "https://example.com/");
        assert_eq!(handle(&c, VaultAccess::Locked).unwrap_err().code, codes::LOCKED);
        assert_eq!(handle(&c, VaultAccess::NoVault).unwrap_err().code, codes::NO_VAULT);
    }

    #[test]
    fn list_returns_only_this_sites_logins_without_passwords() {
        let mut v = vault();
        let exact = add(&mut v, "Example", "alice", "synthetic-1", &["https://example.com"]);
        let site = add(
            &mut v,
            "Example SSO",
            "bob",
            "synthetic-2",
            &["https://sso.example.com"],
        );
        add(&mut v, "Other", "carol", "synthetic-3", &["https://other.test"]);
        let gone = add(&mut v, "Deleted", "dave", "synthetic-4", &["https://example.com"]);
        v.delete_entry(gone.parse().unwrap()).unwrap();

        let out = run(&mut v, list("https://example.com/login", "https://example.com/login")).unwrap();
        assert!(!out.changed);
        let logins = out.body["logins"].as_array().unwrap();
        let ids: Vec<&str> = logins.iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert_eq!(ids, [exact.as_str(), site.as_str()]);
        assert_eq!(logins[0]["match"], "exact");
        assert_eq!(logins[1]["match"], "site");
        assert_eq!(out.body["site"], "example.com");
        assert_eq!(out.body["secure"], true);
        assert!(!out.body.to_string().contains("synthetic-"));
    }

    #[test]
    fn list_refuses_non_web_pages() {
        let mut v = vault();
        let e = run(&mut v, list("chrome://settings", "chrome://settings")).unwrap_err();
        assert_eq!(e.code, codes::NOT_FILLABLE);
    }

    #[test]
    fn fill_returns_credentials_for_matching_entry() {
        let mut v = vault();
        let id = add(&mut v, "Example", "alice", "synthetic-1", &["https://example.com"]);
        let out = run(&mut v, fill("https://example.com/", "https://example.com/", &id, false)).unwrap();
        assert_eq!(out.body["username"], "alice");
        assert_eq!(out.body["password"], "synthetic-1");
    }

    #[test]
    fn fill_refuses_entries_for_other_sites() {
        let mut v = vault();
        let id = add(&mut v, "Bank", "alice", "synthetic-1", &["https://bank.test"]);
        let e = run(&mut v, fill("https://evil.test/", "https://evil.test/", &id, true)).unwrap_err();
        assert_eq!(e.code, codes::NO_MATCH);
        let e = run(&mut v, fill("https://evil.test/", "https://evil.test/", "nope", true)).unwrap_err();
        assert_eq!(e.code, codes::INVALID_REQUEST);
        let missing = uuid::Uuid::new_v4().to_string();
        let e = run(&mut v, fill("https://bank.test/", "https://bank.test/", &missing, true)).unwrap_err();
        assert_eq!(e.code, codes::NOT_FOUND);
    }

    #[test]
    fn fill_needs_consent_on_http_and_cross_origin_frames() {
        let mut v = vault();
        let id = add(&mut v, "Router", "admin", "synthetic-1", &["http://router.example"]);
        let e = run(
            &mut v,
            fill("http://router.example/", "http://router.example/", &id, false),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::CONSENT_REQUIRED);
        assert!(run(
            &mut v,
            fill("http://router.example/", "http://router.example/", &id, true)
        )
        .is_ok());

        let id = add(&mut v, "Example", "alice", "synthetic-2", &["https://example.com"]);
        let frame = "https://accounts.example.com/embed";
        let e = run(&mut v, fill(frame, "https://shop.test/", &id, false)).unwrap_err();
        assert_eq!(e.code, codes::CONSENT_REQUIRED);
        assert!(run(&mut v, fill(frame, "https://shop.test/", &id, true)).is_ok());
    }

    #[test]
    fn deleted_entries_cannot_be_filled() {
        let mut v = vault();
        let id = add(&mut v, "Example", "alice", "synthetic-1", &["https://example.com"]);
        v.delete_entry(id.parse().unwrap()).unwrap();
        let e = run(&mut v, fill("https://example.com/", "https://example.com/", &id, false)).unwrap_err();
        assert_eq!(e.code, codes::NOT_FOUND);
    }

    fn check(v: &mut Vault, user: &str, pw: &str) -> Value {
        run(
            v,
            Command::CheckSave {
                url: "https://example.com/login".into(),
                top_url: "https://example.com/login".into(),
                username: user.into(),
                password: Secret::new(pw),
            },
        )
        .unwrap()
        .body
    }

    fn save_cmd(user: &str, pw: &str, id: Option<&str>) -> Command {
        Command::Save {
            url: "https://login.example.com/form".into(),
            top_url: "https://login.example.com/form".into(),
            username: user.into(),
            password: Secret::new(pw),
            entry_id: id.map(str::to_string),
        }
    }

    #[test]
    fn check_save_classifies_submissions() {
        let mut v = vault();
        let id = add(&mut v, "Example", "alice", "synthetic-1", &["https://example.com"]);
        assert_eq!(check(&mut v, "alice", "synthetic-1")["action"], "none");
        let upd = check(&mut v, "alice", "synthetic-new");
        assert_eq!(upd["action"], "update");
        assert_eq!(upd["entry_id"], id.as_str());
        assert_eq!(upd["title"], "Example");
        assert_eq!(check(&mut v, "bob", "synthetic-1")["action"], "save");
        assert_eq!(check(&mut v, "alice", "")["action"], "none");
    }

    #[test]
    fn save_creates_then_detects_duplicates() {
        let mut v = vault();
        let out = run(&mut v, save_cmd(" alice ", "synthetic-1", None)).unwrap();
        assert!(out.changed);
        assert_eq!(out.body["result"], "created");
        let id: EntryId = out.body["entry_id"].as_str().unwrap().parse().unwrap();
        let data = v.get(id).unwrap().current().data.clone().unwrap();
        assert_eq!(data.title, "example.com");
        assert_eq!(data.username, "alice");
        assert_eq!(data.urls, ["https://login.example.com"]);

        let again = run(&mut v, save_cmd("alice", "synthetic-1", None)).unwrap();
        assert!(!again.changed);
        assert_eq!(again.body["result"], "unchanged");
        assert_eq!(v.entries().count(), 1);
    }

    #[test]
    fn save_updates_matching_entry_only() {
        let mut v = vault();
        let id = add(&mut v, "Example", "", "synthetic-1", &["example.com"]);
        let out = run(&mut v, save_cmd("alice", "synthetic-2", Some(&id))).unwrap();
        assert!(out.changed);
        assert_eq!(out.body["result"], "updated");
        let rec = v.get(id.parse().unwrap()).unwrap();
        let data = rec.current().data.as_ref().unwrap();
        assert_eq!(
            (data.username.as_str(), data.password.as_str()),
            ("alice", "synthetic-2")
        );
        assert_eq!(data.title, "Example");
        // The old password is kept in the entry history.
        assert_eq!(rec.history.len(), 1);

        // Another site's entry cannot be overwritten from this page.
        let bank = add(&mut v, "Bank", "alice", "synthetic-3", &["https://bank.test"]);
        let e = run(&mut v, save_cmd("alice", "synthetic-4", Some(&bank))).unwrap_err();
        assert_eq!(e.code, codes::NO_MATCH);

        let e = run(&mut v, save_cmd("alice", "", None)).unwrap_err();
        assert_eq!(e.code, codes::INVALID_REQUEST);
    }
}
