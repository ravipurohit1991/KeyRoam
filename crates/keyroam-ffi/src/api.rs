//! The engine API exposed to the Flutter apps.
//!
//! Requests are `{"method": "...", "params": {...}}`; responses are
//! `{"ok": ...}` or `{"error": {"code": "...", "message": "..."}}`. The JSON
//! surface keeps the FFI boundary to two C functions and makes the protocol
//! easy to test from both Rust and Dart.
//!
//! One `Engine` exists per process and owns at most one unlocked vault. Calls
//! may arrive concurrently from several Dart isolates: the session mutex
//! serialises them, and long Google sign-in waits run without holding it so
//! they can be cancelled.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use keyroam_core::files::{detect_format, BACKUP_FORMAT, LOCAL_VAULT_FORMAT};
use keyroam_core::generator::{generate_password, GeneratorOptions};
use keyroam_core::header::check_password_policy;
use keyroam_core::{
    BackupFile, EntryData, EntryRecord, KdfCost, LocalSecrets, LocalStore, LocalVaultFile, Vault, VaultHeader, Version,
};
use keyroam_sync::oauth::revoke_token;
use keyroam_sync::{
    commit_local_change, create_vault, fetch_remote_header, join_vault, open_system_browser, sync, DriveClient,
    GoogleSession, OAuthClientConfig, PendingAuthorization, RemoteStore, SyncError, SyncReport, SyncState, SyncStatus,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
        }
    }
}

impl From<keyroam_core::Error> for ApiError {
    fn from(e: keyroam_core::Error) -> Self {
        use keyroam_core::Error as E;
        let code = match &e {
            E::WrongPassword => "wrong_password",
            E::Authentication => "authentication_failed",
            E::InvalidKdfParams(_) | E::Format(_) => "invalid_data",
            E::UnsupportedVersion { .. } => "update_required",
            E::VaultMismatch => "vault_mismatch",
            E::EntryNotFound => "not_found",
            E::InvalidInput(_) => "invalid_input",
            E::NoVault => "no_vault",
            E::VaultExists => "vault_exists",
            E::Io(_) => "io",
        };
        ApiError::new(code, e.to_string())
    }
}

impl From<SyncError> for ApiError {
    fn from(e: SyncError) -> Self {
        let code = match &e {
            SyncError::Core(c) => return ApiError::from(clone_core(c)),
            SyncError::Network(_) => "offline",
            SyncError::AuthRequired(_) => "auth_required",
            SyncError::Auth(_) => "auth_failed",
            SyncError::Http { .. } | SyncError::Protocol(_) => "google_error",
            SyncError::NoRemoteVault => "no_remote_vault",
            SyncError::RemoteVaultExists => "remote_vault_exists",
            SyncError::NotConfigured(_) => "not_configured",
            SyncError::Cancelled => "cancelled",
        };
        ApiError::new(code, e.to_string())
    }
}

/// `keyroam_core::Error` is not `Clone`; rebuild the variants we map on.
fn clone_core(e: &keyroam_core::Error) -> keyroam_core::Error {
    use keyroam_core::Error as E;
    match e {
        E::WrongPassword => E::WrongPassword,
        E::Authentication => E::Authentication,
        E::InvalidKdfParams(s) => E::InvalidKdfParams(s.clone()),
        E::Format(s) => E::Format(s.clone()),
        E::UnsupportedVersion { what, found, supported } => E::UnsupportedVersion {
            what,
            found: *found,
            supported: *supported,
        },
        E::VaultMismatch => E::VaultMismatch,
        E::EntryNotFound => E::EntryNotFound,
        E::InvalidInput(s) => E::InvalidInput(s.clone()),
        E::NoVault => E::NoVault,
        E::VaultExists => E::VaultExists,
        E::Io(io) => E::Io(std::io::Error::new(io.kind(), io.to_string())),
    }
}

type ApiResult<T = Value> = Result<T, ApiError>;

#[derive(Default)]
struct Session {
    store: Option<LocalStore>,
    vault: Option<Vault>,
    remote: Option<Box<dyn RemoteStore + Send>>,
    /// Refresh token obtained during onboarding, before a vault exists.
    refresh_token: Option<Zeroizing<String>>,
    /// Header downloaded during existing-user onboarding.
    remote_header: Option<VaultHeader>,
}

pub struct Engine {
    session: Mutex<Session>,
    pending: Mutex<Option<PendingAuthorization>>,
    cancel: AtomicBool,
    kdf_cost: KdfCost,
}

impl Default for Engine {
    fn default() -> Self {
        Self::with_kdf_cost(KdfCost::DEFAULT)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn params<T: DeserializeOwned>(v: Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|e| ApiError::new("invalid_request", e.to_string()))
}

#[derive(Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    params: Value,
}

// ----- DTOs -----

#[derive(Serialize)]
struct SyncInfo {
    status: &'static str,
    last_success_at: Option<i64>,
    message: Option<String>,
    pending_upload: bool,
}

#[derive(Serialize)]
struct AppState {
    data_dir: Option<String>,
    has_vault: bool,
    unlocked: bool,
    signed_in: bool,
    google_configured: bool,
    vault_id: Option<String>,
    device_id: Option<String>,
    entry_count: usize,
    conflicts: usize,
    sync: SyncInfo,
}

#[derive(Serialize)]
struct EntrySummary<'a> {
    id: String,
    title: &'a str,
    username: &'a str,
    url: &'a str,
    tags: &'a [String],
    favorite: bool,
    conflict: bool,
    deleted: bool,
    modified_at: i64,
}

#[derive(Serialize)]
struct VersionDto<'a> {
    modified_at: i64,
    modified_by: String,
    deleted: bool,
    data: Option<&'a EntryData>,
}

impl<'a> From<&'a Version> for VersionDto<'a> {
    fn from(v: &'a Version) -> Self {
        VersionDto {
            modified_at: v.modified_at,
            modified_by: v.modified_by.to_string(),
            deleted: v.deleted,
            data: v.data.as_ref(),
        }
    }
}

fn report_json(r: &SyncReport) -> Value {
    json!({
        "replicas_merged": r.replicas_merged,
        "local_changed": r.local_changed,
        "uploaded": r.uploaded,
        "header_adopted": r.header_adopted,
        "conflicts": r.conflicts,
        "warnings": r.warnings,
    })
}

fn summary(rec: &EntryRecord) -> EntrySummary<'_> {
    let data = rec.versions.iter().chain(&rec.history).find_map(|v| v.data.as_ref());
    EntrySummary {
        id: rec.id.to_string(),
        title: data.map(|d| d.title.as_str()).unwrap_or(""),
        username: data.map(|d| d.username.as_str()).unwrap_or(""),
        url: data.and_then(|d| d.urls.first()).map(String::as_str).unwrap_or(""),
        tags: data.map(|d| d.tags.as_slice()).unwrap_or(&[]),
        favorite: data.is_some_and(|d| d.favorite),
        conflict: rec.has_conflict(),
        deleted: rec.is_deleted(),
        modified_at: rec.current().modified_at,
    }
}

fn parse_id(s: &str) -> ApiResult<keyroam_core::EntryId> {
    s.parse()
        .map_err(|_| ApiError::new("invalid_input", "invalid entry id"))
}

impl Engine {
    pub fn with_kdf_cost(kdf_cost: KdfCost) -> Self {
        Engine {
            session: Mutex::new(Session::default()),
            pending: Mutex::new(None),
            cancel: AtomicBool::new(false),
            kdf_cost,
        }
    }

    /// Handle one JSON request and produce a JSON response string.
    pub fn handle_json(&self, request: &str) -> String {
        let response = match serde_json::from_str::<Request>(request) {
            Ok(req) => self.dispatch(&req.method, req.params),
            Err(e) => Err(ApiError::new("invalid_request", e.to_string())),
        };
        match response {
            Ok(v) => json!({ "ok": v }).to_string(),
            Err(e) => json!({ "error": { "code": e.code, "message": e.message } }).to_string(),
        }
    }

    pub fn dispatch(&self, method: &str, p: Value) -> ApiResult {
        match method {
            "open" => self.open(params(p)?),
            "state" => self.state(),
            "check_password" => {
                #[derive(Deserialize)]
                struct P {
                    password: String,
                }
                let p: P = params(p)?;
                let p = Zeroizing::new(p.password);
                check_password_policy(&p)?;
                Ok(Value::Null)
            }
            "generate_password" => self.generate(params(p)?),
            "default_backup_path" => Ok(json!(default_backup_path())),
            "begin_sign_in" => self.begin_sign_in(),
            "complete_sign_in" => self.complete_sign_in(),
            "cancel_sign_in" => {
                self.cancel.store(true, Ordering::SeqCst);
                Ok(Value::Null)
            }
            "create_vault" => self.create(params(p)?),
            "fetch_remote_vault" => self.fetch_remote(),
            "join_vault" => self.join(params(p)?),
            "unlock" => self.unlock(params(p)?),
            "lock" => {
                let mut s = lock(&self.session);
                s.vault = None;
                s.remote = None;
                s.refresh_token = None;
                s.remote_header = None;
                Ok(Value::Null)
            }
            "list_entries" => self.list(params(p)?),
            "get_entry" => self.get(params(p)?),
            "add_entry" => self.add(params(p)?),
            "update_entry" => self.update(params(p)?),
            "delete_entry" => self.modify(params(p)?, |v, id| v.delete_entry(id)),
            "restore_entry" => self.modify(params(p)?, |v, id| v.restore_entry(id)),
            "resolve_conflict" => self.resolve(params(p)?),
            "sync" => self.sync_now(),
            "export_backup" => self.export(params(p)?),
            "import_backup" => self.import(params(p)?),
            "change_password" => self.change_password(params(p)?),
            "sign_out" => self.sign_out(),
            other => Err(ApiError::new("invalid_request", format!("unknown method {other}"))),
        }
    }

    // ----- session -----

    fn open(&self, p: OpenParams) -> ApiResult {
        let dir = p
            .data_dir
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("KEYROAM_HOME").map(PathBuf::from))
            .or_else(|| dirs::data_dir().map(|d| d.join("KeyRoam")))
            .ok_or_else(|| ApiError::new("io", "cannot determine a data directory"))?;
        *lock(&self.session) = Session {
            store: Some(LocalStore::new(dir)),
            ..Default::default()
        };
        self.state()
    }

    fn state(&self) -> ApiResult {
        let s = lock(&self.session);
        Ok(serde_json::to_value(Self::state_of(&s))?)
    }

    fn state_of(s: &Session) -> AppState {
        let store = s.store.as_ref();
        let has_vault = store.is_some_and(|st| st.has_vault());
        let has_secrets = store.and_then(|st| st.load_secrets().ok().flatten()).is_some();
        let sync_state = store.and_then(|st| SyncState::load(st).ok()).unwrap_or_default();
        let (status, message) = match sync_state.status() {
            SyncStatus::NeverSynced => ("never", None),
            SyncStatus::UpToDate { .. } => ("up_to_date", None),
            SyncStatus::Pending { .. } => ("pending", None),
            SyncStatus::Offline { .. } => ("offline", sync_state.last_error.clone()),
            SyncStatus::Error { message, .. } => ("error", Some(message)),
        };
        let (vault_id, device_id) = match (&s.vault, store.and_then(|st| st.load_vault().ok())) {
            (Some(v), _) => (Some(v.vault_id().to_string()), Some(v.device_id().to_string())),
            (None, Some(f)) => (Some(f.header.vault_id.to_string()), Some(f.device_id.to_string())),
            _ => (None, None),
        };
        AppState {
            data_dir: store.map(|st| st.root().display().to_string()),
            has_vault,
            unlocked: s.vault.is_some(),
            signed_in: s.remote.is_some() || has_secrets || s.refresh_token.is_some(),
            google_configured: OAuthClientConfig::from_env().is_ok(),
            vault_id,
            device_id,
            entry_count: s.vault.as_ref().map(|v| v.entries().count()).unwrap_or(0),
            conflicts: s.vault.as_ref().map(|v| v.conflicts().count()).unwrap_or(0),
            sync: SyncInfo {
                status,
                last_success_at: sync_state.last_success_at,
                message,
                pending_upload: sync_state.pending_upload,
            },
        }
    }

    fn store(s: &Session) -> ApiResult<&LocalStore> {
        s.store
            .as_ref()
            .ok_or_else(|| ApiError::new("invalid_request", "call open first"))
    }

    fn unlocked(s: &mut Session) -> ApiResult<(&mut Vault, &LocalStore)> {
        let store = s
            .store
            .as_ref()
            .ok_or_else(|| ApiError::new("invalid_request", "call open first"))?;
        let vault = s
            .vault
            .as_mut()
            .ok_or_else(|| ApiError::new("locked", "the vault is locked"))?;
        Ok((vault, store))
    }

    fn drive_from_token(token: &str) -> ApiResult<Box<dyn RemoteStore + Send>> {
        let cfg = OAuthClientConfig::from_env()?;
        Ok(Box::new(DriveClient::new(GoogleSession::new(cfg, token))))
    }

    // ----- Google sign-in -----

    fn begin_sign_in(&self) -> ApiResult {
        let cfg = OAuthClientConfig::from_env()?;
        let pending = PendingAuthorization::begin(&cfg)?;
        let url = pending.url().to_string();
        self.cancel.store(false, Ordering::SeqCst);
        *lock(&self.pending) = Some(pending);
        let opened = open_system_browser(&url);
        Ok(json!({ "url": url, "browser_opened": opened }))
    }

    fn complete_sign_in(&self) -> ApiResult {
        let pending = lock(&self.pending)
            .take()
            .ok_or_else(|| ApiError::new("invalid_request", "no sign-in in progress"))?;
        // Wait without holding the session lock so cancel/state stay responsive.
        let google = pending.complete(SIGN_IN_TIMEOUT, &self.cancel)?;
        let token = Zeroizing::new(google.refresh_token().to_string());
        let drive = DriveClient::new(google);
        self.finish_sign_in(Box::new(drive), token)
    }

    /// Store a fresh Google session. With an unlocked vault (re-sign-in) the
    /// account must hold the same vault, and the token is saved encrypted.
    fn finish_sign_in(&self, remote: Box<dyn RemoteStore + Send>, token: Zeroizing<String>) -> ApiResult {
        let mut s = lock(&self.session);
        if let (Some(vault), Some(store)) = (s.vault.as_ref(), s.store.as_ref()) {
            match fetch_remote_header(remote.as_ref()) {
                Ok(h) if h.vault_id != vault.vault_id() => {
                    return Err(ApiError::new(
                        "vault_mismatch",
                        "this Google account holds a different KeyRoam vault",
                    ))
                }
                Ok(_) | Err(SyncError::NoRemoteVault) => {}
                Err(e) => return Err(e.into()),
            }
            let secrets = LocalSecrets {
                google_refresh_token: Some(token.to_string()),
            };
            store.save_secrets(&vault.seal_secrets(&secrets)?)?;
        }
        s.refresh_token = Some(token);
        s.remote = Some(remote);
        Ok(serde_json::to_value(Self::state_of(&s))?)
    }

    /// Test hook: install a remote store as if Google sign-in had completed.
    #[doc(hidden)]
    pub fn sign_in_with_remote(&self, remote: Box<dyn RemoteStore + Send>, refresh_token: &str) -> ApiResult {
        self.finish_sign_in(remote, Zeroizing::new(refresh_token.to_string()))
    }

    fn sign_out(&self) -> ApiResult {
        let mut s = lock(&self.session);
        let store = Self::store(&s)?.clone();
        if let (Some(vault), Ok(Some(file))) = (s.vault.as_ref(), store.load_secrets()) {
            if let (Ok(secrets), Ok(cfg)) = (vault.open_secrets(&file), OAuthClientConfig::from_env()) {
                if let Some(t) = &secrets.google_refresh_token {
                    // Best effort: the local token is removed regardless.
                    let _ = revoke_token(&cfg, t);
                }
            }
        }
        store.delete_secrets()?;
        s.remote = None;
        s.refresh_token = None;
        Ok(serde_json::to_value(Self::state_of(&s))?)
    }

    // ----- onboarding -----

    fn create(&self, p: PasswordParams) -> ApiResult {
        let password = Zeroizing::new(p.password);
        let mut s = lock(&self.session);
        let store = Self::store(&s)?.clone();
        let (remote, token) = match (s.remote.as_ref(), s.refresh_token.as_ref()) {
            (Some(r), Some(t)) => (r, t),
            _ => return Err(ApiError::new("not_signed_in", "sign in with Google first")),
        };
        let secrets = LocalSecrets {
            google_refresh_token: Some(token.to_string()),
        };
        let (vault, report) = create_vault(&store, remote.as_ref(), &password, self.kdf_cost, &secrets)?;
        s.vault = Some(vault);
        let sync = match report {
            Ok(r) => json!({ "report": report_json(&r) }),
            Err(e) => json!({ "error": ApiError::from(e).message }),
        };
        Ok(json!({ "state": Self::state_of(&s), "sync": sync }))
    }

    fn fetch_remote(&self) -> ApiResult {
        let mut s = lock(&self.session);
        let remote = s
            .remote
            .as_ref()
            .ok_or_else(|| ApiError::new("not_signed_in", "sign in with Google first"))?;
        let header = fetch_remote_header(remote.as_ref())?;
        let vault_id = header.vault_id.to_string();
        s.remote_header = Some(header);
        Ok(json!({ "vault_id": vault_id }))
    }

    fn join(&self, p: PasswordParams) -> ApiResult {
        let password = Zeroizing::new(p.password);
        let mut s = lock(&self.session);
        let store = Self::store(&s)?.clone();
        let header = s
            .remote_header
            .clone()
            .ok_or_else(|| ApiError::new("invalid_request", "download the vault first"))?;
        let (remote, token) = match (s.remote.as_ref(), s.refresh_token.as_ref()) {
            (Some(r), Some(t)) => (r, t),
            _ => return Err(ApiError::new("not_signed_in", "sign in with Google first")),
        };
        let secrets = LocalSecrets {
            google_refresh_token: Some(token.to_string()),
        };
        let (vault, report) = join_vault(&store, remote.as_ref(), header, &password, &secrets)?;
        s.vault = Some(vault);
        s.remote_header = None;
        let sync = match report {
            Ok(r) => json!({ "report": report_json(&r) }),
            Err(e) => json!({ "error": ApiError::from(e).message }),
        };
        Ok(json!({ "state": Self::state_of(&s), "sync": sync }))
    }

    fn unlock(&self, p: PasswordParams) -> ApiResult {
        let password = Zeroizing::new(p.password);
        let store = Self::store(&lock(&self.session))?.clone();
        // Argon2 runs without holding the session lock.
        let vault = Vault::open(&store.load_vault()?, &password)?;
        let token = match store.load_secrets()? {
            Some(f) => vault.open_secrets(&f)?.google_refresh_token.clone(),
            None => None,
        };
        let mut s = lock(&self.session);
        s.remote = match &token {
            Some(t) => Self::drive_from_token(t).ok(),
            None => None,
        };
        s.refresh_token = token.map(Zeroizing::new);
        s.vault = Some(vault);
        Ok(serde_json::to_value(Self::state_of(&s))?)
    }

    // ----- entries -----

    fn list(&self, p: ListParams) -> ApiResult {
        let mut s = lock(&self.session);
        let (vault, _) = Self::unlocked(&mut s)?;
        let recs: Vec<&EntryRecord> = if p.deleted {
            vault.deleted_entries().collect()
        } else if let Some(q) = p.query.as_deref().filter(|q| !q.trim().is_empty()) {
            vault.search(q.trim())
        } else {
            vault.entries().collect()
        };
        let mut out: Vec<EntrySummary> = recs.into_iter().map(summary).collect();
        out.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(serde_json::to_value(out)?)
    }

    fn get(&self, p: IdParams) -> ApiResult {
        let mut s = lock(&self.session);
        let (vault, _) = Self::unlocked(&mut s)?;
        let rec = vault
            .get(parse_id(&p.id)?)
            .ok_or_else(|| ApiError::from(keyroam_core::Error::EntryNotFound))?;
        Ok(json!({
            "id": rec.id.to_string(),
            "conflict": rec.has_conflict(),
            "deleted": rec.is_deleted(),
            "versions": rec.versions.iter().map(VersionDto::from).collect::<Vec<_>>(),
            "history": rec.history.iter().map(VersionDto::from).collect::<Vec<_>>(),
        }))
    }

    fn add(&self, p: DataParams) -> ApiResult {
        let mut s = lock(&self.session);
        let (vault, store) = Self::unlocked(&mut s)?;
        let id = vault.add_entry(p.data)?;
        commit_local_change(vault, store)?;
        Ok(json!({ "id": id.to_string() }))
    }

    fn update(&self, p: UpdateParams) -> ApiResult {
        let mut s = lock(&self.session);
        let (vault, store) = Self::unlocked(&mut s)?;
        vault.update_entry(parse_id(&p.id)?, p.data)?;
        commit_local_change(vault, store)?;
        Ok(Value::Null)
    }

    fn modify(
        &self,
        p: IdParams,
        f: impl FnOnce(&mut Vault, keyroam_core::EntryId) -> keyroam_core::Result<()>,
    ) -> ApiResult {
        let mut s = lock(&self.session);
        let (vault, store) = Self::unlocked(&mut s)?;
        f(vault, parse_id(&p.id)?)?;
        commit_local_change(vault, store)?;
        Ok(Value::Null)
    }

    fn resolve(&self, p: ResolveParams) -> ApiResult {
        self.modify(IdParams { id: p.id }, |v, id| v.resolve_conflict(id, p.choice))
    }

    fn generate(&self, p: GenerateParams) -> ApiResult {
        let pw = generate_password(&GeneratorOptions {
            length: p.length,
            lowercase: p.lowercase,
            uppercase: p.uppercase,
            digits: p.digits,
            symbols: p.symbols,
            avoid_ambiguous: p.avoid_ambiguous,
        })?;
        Ok(json!(pw.as_str()))
    }

    // ----- sync, backups, password -----

    fn sync_now(&self) -> ApiResult {
        let mut guard = lock(&self.session);
        let s = &mut *guard;
        let store = Self::store(s)?.clone();
        let vault = s
            .vault
            .as_mut()
            .ok_or_else(|| ApiError::new("locked", "the vault is locked"))?;
        let remote = s.remote.as_ref().ok_or_else(|| match OAuthClientConfig::from_env() {
            Err(e) if s.refresh_token.is_some() => ApiError::from(e),
            _ => ApiError::new("not_signed_in", "this device is not signed in to Google"),
        })?;
        let report = sync(vault, &store, remote.as_ref())?;
        Ok(json!({ "report": report_json(&report), "state": Self::state_of(s) }))
    }

    fn export(&self, p: ExportParams) -> ApiResult {
        let password = Zeroizing::new(p.password);
        let path = PathBuf::from(&p.path);
        if path.exists() && !p.overwrite {
            return Err(ApiError::new("exists", format!("{} already exists", path.display())));
        }
        let mut s = lock(&self.session);
        let (vault, _) = Self::unlocked(&mut s)?;
        let backup = vault.export_backup(&password, self.kdf_cost)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(keyroam_core::Error::from)?;
        }
        std::fs::write(&path, backup.to_bytes()?).map_err(keyroam_core::Error::from)?;
        Ok(json!({ "entries": backup.entry_count, "path": path.display().to_string() }))
    }

    fn import(&self, p: ImportParams) -> ApiResult {
        let password = Zeroizing::new(p.password);
        let bytes = std::fs::read(&p.path).map_err(keyroam_core::Error::from)?;
        let format = detect_format(&bytes)?;
        let mut s = lock(&self.session);
        let (vault, store) = Self::unlocked(&mut s)?;
        let state = if format == BACKUP_FORMAT {
            BackupFile::from_bytes(&bytes)?.open(&password)?
        } else if format == LOCAL_VAULT_FORMAT {
            vault.open_local_backup(&LocalVaultFile::from_bytes(&bytes)?, &password)?
        } else {
            return Err(ApiError::new("invalid_data", "not a KeyRoam backup file"));
        };
        let before = vault.entries().count();
        let changed = vault.merge_state(&state)?;
        if changed {
            store.save_vault(&vault.to_local_file()?, true)?;
            commit_local_change(vault, store)?;
        }
        Ok(json!({ "before": before, "after": vault.entries().count(), "changed": changed }))
    }

    fn change_password(&self, p: ChangePasswordParams) -> ApiResult {
        let current = Zeroizing::new(p.current);
        let new = Zeroizing::new(p.new);
        let mut s = lock(&self.session);
        let (vault, store) = Self::unlocked(&mut s)?;
        vault.change_password(&current, &new, self.kdf_cost)?;
        store.save_vault(&vault.to_local_file()?, true)?;
        commit_local_change(vault, store)?;
        Ok(Value::Null)
    }
}

fn default_backup_path() -> String {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let (y, m, d) = civil_date(keyroam_core::now_millis() / 86_400_000);
    dir.join(format!("KeyRoam backup {y:04}-{m:02}-{d:02}.krbackup"))
        .display()
        .to_string()
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar.
fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::new("internal", e.to_string())
    }
}

#[derive(Deserialize)]
struct OpenParams {
    #[serde(default)]
    data_dir: Option<String>,
}

#[derive(Deserialize)]
struct PasswordParams {
    password: String,
}

#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    deleted: bool,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct DataParams {
    data: EntryData,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    data: EntryData,
}

#[derive(Deserialize)]
struct ResolveParams {
    id: String,
    choice: usize,
}

fn default_true() -> bool {
    true
}
fn default_length() -> usize {
    20
}

#[derive(Deserialize)]
struct GenerateParams {
    #[serde(default = "default_length")]
    length: usize,
    #[serde(default = "default_true")]
    lowercase: bool,
    #[serde(default = "default_true")]
    uppercase: bool,
    #[serde(default = "default_true")]
    digits: bool,
    #[serde(default = "default_true")]
    symbols: bool,
    #[serde(default = "default_true")]
    avoid_ambiguous: bool,
}

#[derive(Deserialize)]
struct ExportParams {
    path: String,
    password: String,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize)]
struct ImportParams {
    path: String,
    password: String,
}

#[derive(Deserialize)]
struct ChangePasswordParams {
    current: String,
    new: String,
}

#[cfg(test)]
mod tests;
