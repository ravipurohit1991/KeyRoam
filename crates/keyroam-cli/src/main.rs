//! `keyroam` — command-line front-end for the KeyRoam vault and sync engine.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use keyroam_core::files::{detect_format, BACKUP_FORMAT, LOCAL_VAULT_FORMAT};
use keyroam_core::generator::{generate_password, GeneratorOptions};
use keyroam_core::header::check_password_policy;
use keyroam_core::{
    now_millis, BackupFile, EntryData, EntryRecord, KdfCost, LocalSecrets, LocalStore, LocalVaultFile, Vault,
};
use keyroam_sync::{
    authorize_installed_app, commit_local_change, create_vault, fetch_remote_header, join_vault, sync, DriveClient,
    GoogleSession, OAuthClientConfig, SyncError, SyncReport, SyncState, SyncStatus,
};
use zeroize::Zeroizing;

type CliResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "keyroam", version, about = "KeyRoam — Your keys. Wherever you go.")]
struct Cli {
    /// Directory holding this device's encrypted vault.
    #[arg(long, env = "KEYROAM_HOME", global = true)]
    data_dir: Option<PathBuf>,
    /// Read master/backup passwords from stdin, one per line (for scripting).
    #[arg(long, global = true)]
    password_stdin: bool,
    /// Do not synchronise with Google Drive for this command.
    #[arg(long, global = true)]
    no_sync: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// New user: create a master password, sign in to Google and create the vault.
    Init,
    /// Existing user on a new device: sign in to Google and unlock your vault.
    Join,
    /// Re-connect this device to Google Drive (after sign-out or expiry).
    Login,
    /// Revoke this device's Google access. The local vault stays usable offline.
    Logout,
    /// Show synchronisation status (no password needed).
    Status,
    /// Synchronise with Google Drive now.
    Sync,
    /// List entries.
    List {
        #[arg(long, short)]
        search: Option<String>,
        /// List deleted (restorable) entries instead.
        #[arg(long)]
        deleted: bool,
    },
    /// Show one entry.
    Show {
        id: String,
        /// Print the password.
        #[arg(long)]
        reveal: bool,
        /// Include previous versions.
        #[arg(long)]
        history: bool,
    },
    /// Add a login entry. The password is prompted for (empty = generate).
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        username: String,
        #[arg(long = "url")]
        urls: Vec<String>,
        #[arg(long, default_value = "")]
        notes: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Generate the password instead of prompting.
        #[arg(long)]
        generate: bool,
        #[arg(long, default_value_t = 20)]
        length: usize,
    },
    /// Edit an entry. Unspecified fields are kept. Editing resolves conflicts.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        username: Option<String>,
        /// Replace the URL list (repeatable).
        #[arg(long = "url")]
        urls: Option<Vec<String>>,
        #[arg(long)]
        notes: Option<String>,
        /// Prompt for a new password.
        #[arg(long)]
        password: bool,
        /// Generate a new password.
        #[arg(long)]
        generate: bool,
        #[arg(long, default_value_t = 20)]
        length: usize,
    },
    /// Delete an entry (restorable with `restore`).
    Rm { id: String },
    /// Restore a deleted entry.
    Restore { id: String },
    /// List entries edited concurrently on different devices.
    Conflicts,
    /// Resolve a conflict by keeping version CHOICE (see `conflicts`).
    Resolve { id: String, choice: usize },
    /// Generate a password without storing it.
    Generate {
        #[arg(long, default_value_t = 20)]
        length: usize,
        #[arg(long)]
        no_symbols: bool,
        #[arg(long)]
        no_digits: bool,
        #[arg(long)]
        no_uppercase: bool,
        #[arg(long)]
        allow_ambiguous: bool,
    },
    /// Export an encrypted backup file protected by a backup password.
    Export {
        path: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Merge an encrypted backup (or a local vault backup) into the vault.
    Import { path: PathBuf },
    /// Change the master password on all devices.
    Passwd,
}

/// Reads secrets from the terminal, or from stdin lines with --password-stdin.
struct Prompter {
    from_stdin: bool,
}

impl Prompter {
    fn secret(&self, prompt: &str) -> CliResult<Zeroizing<String>> {
        if self.from_stdin {
            let mut line = Zeroizing::new(String::new());
            std::io::stdin().lock().read_line(&mut line)?;
            let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
            return Ok(Zeroizing::new(trimmed));
        }
        Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
    }

    fn new_password(&self, what: &str) -> CliResult<Zeroizing<String>> {
        loop {
            let a = self.secret(&format!("{what}: "))?;
            if let Err(e) = check_password_policy(&a) {
                if self.from_stdin {
                    return Err(e.into());
                }
                eprintln!("{e}");
                continue;
            }
            let b = self.secret(&format!("Confirm {}: ", what.to_lowercase()))?;
            if *a == *b {
                return Ok(a);
            }
            if self.from_stdin {
                return Err("passwords do not match".into());
            }
            eprintln!("Passwords do not match, try again.");
        }
    }
}

struct Ctx {
    store: LocalStore,
    prompt: Prompter,
    no_sync: bool,
}

impl Ctx {
    fn unlock(&self) -> CliResult<(Vault, Zeroizing<String>)> {
        let file = self.store.load_vault().map_err(|e| match e {
            keyroam_core::Error::NoVault => {
                "no vault on this device; run `keyroam init` (new user) or `keyroam join`".into()
            }
            e => Box::<dyn std::error::Error>::from(e),
        })?;
        let pw = self.prompt.secret("Master password: ")?;
        let vault = Vault::open(&file, &pw)?;
        Ok((vault, pw))
    }

    fn drive(&self, vault: &Vault) -> CliResult<DriveClient<GoogleSession>> {
        let secrets = match self.store.load_secrets()? {
            Some(f) => vault.open_secrets(&f)?,
            None => LocalSecrets::default(),
        };
        let token = secrets
            .google_refresh_token
            .as_deref()
            .ok_or("this device is not signed in to Google; run `keyroam login`")?;
        let cfg = OAuthClientConfig::from_env()?;
        Ok(DriveClient::new(GoogleSession::new(cfg, token)))
    }

    /// Best-effort sync used on open and after modifications.
    fn auto_sync(&self, vault: &mut Vault) {
        if self.no_sync {
            return;
        }
        let result = self.drive(vault).and_then(|d| Ok(sync(vault, &self.store, &d)?));
        match result {
            Ok(r) => report_sync(&r, true),
            Err(e) => eprintln!("sync: {e} (changes are kept locally and will sync later)"),
        }
    }

    fn commit(&self, vault: &mut Vault) -> CliResult {
        commit_local_change(vault, &self.store)?;
        self.auto_sync(vault);
        Ok(())
    }
}

fn report_sync(r: &SyncReport, quiet: bool) {
    let mut parts = Vec::new();
    if r.replicas_merged > 0 {
        parts.push(format!("merged {} device replica(s)", r.replicas_merged));
    }
    if r.uploaded {
        parts.push("uploaded".into());
    }
    if parts.is_empty() && quiet {
        eprintln!("sync: up to date");
    } else if parts.is_empty() {
        eprintln!("sync: up to date, nothing to transfer");
    } else {
        eprintln!("sync: {}", parts.join(", "));
    }
    if r.header_adopted {
        eprintln!("sync: the master password was changed on another device; use the new password from now on");
    }
    if r.conflicts > 0 {
        eprintln!(
            "sync: {} entr{} edited on multiple devices — run `keyroam conflicts`",
            r.conflicts,
            if r.conflicts == 1 { "y was" } else { "ies were" }
        );
    }
    for w in &r.warnings {
        eprintln!("sync warning: {w}");
    }
}

fn open_browser(url: &str) {
    eprintln!("\nOpen this URL to sign in with Google (opening your browser):\n\n  {url}\n");
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if r.is_err() {
        eprintln!("(could not open a browser automatically)");
    }
}

fn google_sign_in() -> CliResult<GoogleSession> {
    let cfg = OAuthClientConfig::from_env()?;
    eprintln!("KeyRoam requests access only to its own hidden app folder in your Google Drive.");
    Ok(authorize_installed_app(&cfg, Duration::from_secs(300), open_browser)?)
}

fn ago(ms: i64) -> String {
    let s = (now_millis() - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

fn short(id: &keyroam_core::EntryId) -> String {
    id.to_string()[..8].to_string()
}

fn mask(_s: &str) -> &'static str {
    "••••••••"
}

fn print_entry(rec: &EntryRecord, reveal: bool, history: bool) {
    let cur = rec.current();
    println!("id:        {}", rec.id);
    if rec.has_conflict() {
        println!(
            "status:    CONFLICT — {} versions; run `keyroam conflicts`",
            rec.versions.len()
        );
    } else if rec.is_deleted() {
        println!("status:    deleted (restore with `keyroam restore {}`)", short(&rec.id));
    }
    if let Some(d) = &cur.data {
        println!("title:     {}", d.title);
        println!("username:  {}", d.username);
        println!(
            "password:  {}",
            if reveal { d.password.as_str() } else { mask(&d.password) }
        );
        for u in &d.urls {
            println!("url:       {u}");
        }
        if !d.tags.is_empty() {
            println!("tags:      {}", d.tags.join(", "));
        }
        if !d.notes.is_empty() {
            println!("notes:     {}", d.notes);
        }
    }
    println!("modified:  {}", ago(cur.modified_at));
    if history {
        println!("history:");
        for v in &rec.history {
            if let Some(d) = &v.data {
                let pw = if reveal { d.password.as_str() } else { mask(&d.password) };
                println!("  {:<12} {}  {}", ago(v.modified_at), d.title, pw);
            }
        }
    }
}

fn gen(length: usize) -> CliResult<Zeroizing<String>> {
    Ok(generate_password(&GeneratorOptions {
        length,
        ..Default::default()
    })?)
}

fn run(cli: Cli) -> CliResult {
    let data_dir = cli
        .data_dir
        .or_else(|| dirs::data_dir().map(|d| d.join("KeyRoam")))
        .ok_or("cannot determine a data directory; pass --data-dir")?;
    let ctx = Ctx {
        store: LocalStore::new(data_dir),
        prompt: Prompter {
            from_stdin: cli.password_stdin,
        },
        no_sync: cli.no_sync,
    };

    match cli.cmd {
        Cmd::Init => {
            println!("Welcome to KeyRoam — Your keys. Wherever you go.\n");
            if ctx.store.has_vault() {
                return Err("this device already has a vault".into());
            }
            println!("Step 1/3: choose a master password (at least 12 characters).");
            println!("It is never stored or sent anywhere and cannot be recovered if forgotten.");
            let pw = ctx.prompt.new_password("Master password")?;
            println!("\nStep 2/3: sign in with Google.");
            let session = google_sign_in()?;
            let secrets = LocalSecrets {
                google_refresh_token: Some(session.refresh_token().to_string()),
            };
            let drive = DriveClient::new(session);
            println!("\nStep 3/3: creating your encrypted vault and synchronising it to Google Drive…");
            let (vault, report) = create_vault(&ctx.store, &drive, &pw, KdfCost::DEFAULT, &secrets)?;
            match report {
                Ok(r) => report_sync(&r, false),
                Err(e) => {
                    eprintln!("sync: {e}\nYour vault was created on this device and will upload on the next sync.")
                }
            }
            println!(
                "\nYour vault is ready ({} entries). Try `keyroam add --title Example`.",
                vault.entries().count()
            );
            println!("Tip: `keyroam export <file>` creates an independent encrypted backup.");
        }
        Cmd::Join => {
            println!("Welcome back to KeyRoam.\n");
            if ctx.store.has_vault() {
                return Err("this device already has a vault".into());
            }
            println!("Step 1/3: sign in with the Google account that holds your vault.");
            let session = google_sign_in()?;
            let secrets = LocalSecrets {
                google_refresh_token: Some(session.refresh_token().to_string()),
            };
            let drive = DriveClient::new(session);
            println!("\nStep 2/3: downloading your encrypted vault…");
            let header = fetch_remote_header(&drive).map_err(|e| match e {
                SyncError::NoRemoteVault => {
                    "no KeyRoam vault found in this Google account; run `keyroam init` to create one".into()
                }
                e => Box::<dyn std::error::Error>::from(e),
            })?;
            println!("\nStep 3/3: enter your existing master password.");
            let mut attempts = 0;
            let (vault, report) = loop {
                let pw = ctx.prompt.secret("Master password: ")?;
                match join_vault(&ctx.store, &drive, header.clone(), &pw, &secrets) {
                    Err(SyncError::Core(keyroam_core::Error::WrongPassword))
                        if attempts < 2 && !ctx.prompt.from_stdin =>
                    {
                        attempts += 1;
                        eprintln!("Incorrect master password, try again.");
                    }
                    other => break other?,
                }
            };
            match report {
                Ok(r) => report_sync(&r, false),
                Err(e) => eprintln!("sync: {e}\nRun `keyroam sync` to finish downloading."),
            }
            println!(
                "\nUnlocked. {} entries available on this device.",
                vault.entries().count()
            );
        }
        Cmd::Login => {
            let (mut vault, _pw) = ctx.unlock()?;
            let session = google_sign_in()?;
            let drive = DriveClient::new(session);
            if let Ok(h) = fetch_remote_header(&drive) {
                if h.vault_id != vault.vault_id() {
                    return Err("this Google account holds a different KeyRoam vault".into());
                }
            }
            let secrets = LocalSecrets {
                google_refresh_token: Some(drive.tokens().refresh_token().to_string()),
            };
            ctx.store.save_secrets(&vault.seal_secrets(&secrets)?)?;
            println!("Signed in.");
            if !ctx.no_sync {
                report_sync(&sync(&mut vault, &ctx.store, &drive)?, false);
            }
        }
        Cmd::Logout => {
            let (vault, _pw) = ctx.unlock()?;
            if let Ok(drive) = ctx.drive(&vault) {
                if let Err(e) = drive.tokens().revoke() {
                    eprintln!("warning: could not revoke the token at Google ({e}); removing it locally anyway");
                }
            }
            ctx.store.delete_secrets()?;
            println!("Signed out of Google on this device. Your vault remains available offline.");
        }
        Cmd::Status => {
            if !ctx.store.has_vault() {
                println!("No vault on this device. Run `keyroam init` (new user) or `keyroam join` (existing user).");
                return Ok(());
            }
            let file = ctx.store.load_vault()?;
            let st = SyncState::load(&ctx.store)?;
            println!("vault:     {}", file.header.vault_id);
            println!("device:    {}", file.device_id);
            println!(
                "google:    {}",
                if ctx.store.load_secrets()?.is_some() {
                    "signed in"
                } else {
                    "signed out"
                }
            );
            let since = |t: Option<i64>| t.map(ago).unwrap_or_else(|| "never".into());
            let line = match st.status() {
                SyncStatus::NeverSynced => "never synchronised".to_string(),
                SyncStatus::UpToDate { at } => format!("up to date (last sync {})", ago(at)),
                SyncStatus::Pending { last_success_at } => {
                    format!("changes waiting to upload (last sync {})", since(last_success_at))
                }
                SyncStatus::Offline { last_success_at } => {
                    format!("offline — will retry (last sync {})", since(last_success_at))
                }
                SyncStatus::Error {
                    message,
                    last_success_at,
                } => {
                    format!("error: {message} (last sync {})", since(last_success_at))
                }
            };
            println!("sync:      {line}");
            if st.pending_upload && !matches!(st.status(), SyncStatus::Pending { .. }) {
                println!("           local changes are waiting to upload");
            }
        }
        Cmd::Sync => {
            let (mut vault, _pw) = ctx.unlock()?;
            let drive = ctx.drive(&vault)?;
            let r = sync(&mut vault, &ctx.store, &drive)?;
            report_sync(&r, false);
        }
        Cmd::List { search, deleted } => {
            let (mut vault, _pw) = ctx.unlock()?;
            ctx.auto_sync(&mut vault);
            let mut recs: Vec<&EntryRecord> = if deleted {
                vault.deleted_entries().collect()
            } else if let Some(q) = &search {
                vault.search(q)
            } else {
                vault.entries().collect()
            };
            recs.sort_by_key(|r| {
                r.current()
                    .data
                    .as_ref()
                    .map(|d| d.title.to_lowercase())
                    .unwrap_or_default()
            });
            for r in &recs {
                let shown = r.versions.iter().find_map(|v| v.data.as_ref());
                let (title, user, url) = shown
                    .map(|d| {
                        (
                            d.title.as_str(),
                            d.username.as_str(),
                            d.urls.first().map(String::as_str).unwrap_or(""),
                        )
                    })
                    .unwrap_or(("(deleted)", "", ""));
                let flag = if r.has_conflict() { "  [CONFLICT]" } else { "" };
                println!("{}  {:<28} {:<28} {}{}", short(&r.id), title, user, url, flag);
            }
            if recs.is_empty() {
                println!("(no entries)");
            }
        }
        Cmd::Show { id, reveal, history } => {
            let (mut vault, _pw) = ctx.unlock()?;
            ctx.auto_sync(&mut vault);
            let id = vault.resolve_id(&id)?;
            print_entry(vault.get(id).expect("resolved"), reveal, history);
        }
        Cmd::Add {
            title,
            username,
            urls,
            notes,
            tags,
            generate,
            length,
        } => {
            let (mut vault, _pw) = ctx.unlock()?;
            let password = if generate {
                gen(length)?
            } else {
                let p = ctx.prompt.secret("Entry password (empty to generate): ")?;
                if p.is_empty() {
                    gen(length)?
                } else {
                    p
                }
            };
            let id = vault.add_entry(EntryData {
                title,
                username,
                password: password.to_string(),
                urls,
                notes,
                tags,
                favorite: false,
            })?;
            println!("Added {}", short(&id));
            ctx.commit(&mut vault)?;
        }
        Cmd::Edit {
            id,
            title,
            username,
            urls,
            notes,
            password,
            generate,
            length,
        } => {
            let (mut vault, _pw) = ctx.unlock()?;
            let id = vault.resolve_id(&id)?;
            let rec = vault.get(id).expect("resolved");
            let mut data = rec
                .versions
                .iter()
                .chain(&rec.history)
                .find_map(|v| v.data.clone())
                .unwrap_or_default();
            if let Some(t) = title {
                data.title = t;
            }
            if let Some(u) = username {
                data.username = u;
            }
            if let Some(u) = urls {
                data.urls = u;
            }
            if let Some(n) = notes {
                data.notes = n;
            }
            if generate {
                data.password = gen(length)?.to_string();
            } else if password {
                data.password = ctx.prompt.secret("New entry password: ")?.to_string();
            }
            vault.update_entry(id, data)?;
            println!("Updated {}", short(&id));
            ctx.commit(&mut vault)?;
        }
        Cmd::Rm { id } => {
            let (mut vault, _pw) = ctx.unlock()?;
            let id = vault.resolve_id(&id)?;
            vault.delete_entry(id)?;
            println!("Deleted {} (restore with `keyroam restore {}`)", short(&id), short(&id));
            ctx.commit(&mut vault)?;
        }
        Cmd::Restore { id } => {
            let (mut vault, _pw) = ctx.unlock()?;
            let id = vault.resolve_id(&id)?;
            vault.restore_entry(id)?;
            println!("Restored {}", short(&id));
            ctx.commit(&mut vault)?;
        }
        Cmd::Conflicts => {
            let (mut vault, _pw) = ctx.unlock()?;
            ctx.auto_sync(&mut vault);
            let mut any = false;
            for rec in vault.conflicts() {
                any = true;
                println!("{}  — edited on several devices at once:", rec.id);
                for (i, v) in rec.versions.iter().enumerate() {
                    let desc = match &v.data {
                        Some(d) => format!("{} / {} / password {}", d.title, d.username, mask(&d.password)),
                        None => "deleted".into(),
                    };
                    println!(
                        "   [{i}] {desc}  ({} by device {})",
                        ago(v.modified_at),
                        &v.modified_by.to_string()[..8]
                    );
                }
                println!(
                    "   keep one with: keyroam resolve {} <n>   (inspect with `show --reveal`)",
                    short(&rec.id)
                );
            }
            if !any {
                println!("No conflicts.");
            }
        }
        Cmd::Resolve { id, choice } => {
            let (mut vault, _pw) = ctx.unlock()?;
            let id = vault.resolve_id(&id)?;
            vault.resolve_conflict(id, choice)?;
            println!(
                "Resolved {}; the other versions remain in the entry history.",
                short(&id)
            );
            ctx.commit(&mut vault)?;
        }
        Cmd::Generate {
            length,
            no_symbols,
            no_digits,
            no_uppercase,
            allow_ambiguous,
        } => {
            let p = generate_password(&GeneratorOptions {
                length,
                lowercase: true,
                uppercase: !no_uppercase,
                digits: !no_digits,
                symbols: !no_symbols,
                avoid_ambiguous: !allow_ambiguous,
            })?;
            println!("{}", p.as_str());
        }
        Cmd::Export { path, force } => {
            if path.exists() && !force {
                return Err(format!("{} exists; pass --force to overwrite", path.display()).into());
            }
            let (vault, _pw) = ctx.unlock()?;
            println!("Choose a password for this backup file (store it somewhere safe).");
            let bpw = ctx.prompt.new_password("Backup password")?;
            let backup = vault.export_backup(&bpw, KdfCost::DEFAULT)?;
            std::fs::write(&path, backup.to_bytes()?)?;
            println!("Exported {} entries to {}", backup.entry_count, path.display());
        }
        Cmd::Import { path } => {
            let bytes = std::fs::read(&path)?;
            let format = detect_format(&bytes)?;
            let (mut vault, _pw) = ctx.unlock()?;
            let state = if format == BACKUP_FORMAT {
                let backup = BackupFile::from_bytes(&bytes)?;
                let bpw = ctx.prompt.secret("Backup password: ")?;
                backup.open(&bpw)?
            } else if format == LOCAL_VAULT_FORMAT {
                let file = LocalVaultFile::from_bytes(&bytes)?;
                let opw = ctx.prompt.secret("Master password at the time of that backup: ")?;
                vault.open_local_backup(&file, &opw)?
            } else {
                return Err(format!("{} is not a KeyRoam backup", path.display()).into());
            };
            let before = vault.entries().count();
            let changed = vault.merge_state(&state)?;
            println!(
                "Merged backup: {} entries before, {} after{}.",
                before,
                vault.entries().count(),
                if changed { "" } else { " (nothing new)" }
            );
            if changed {
                ctx.store.save_vault(&vault.to_local_file()?, true)?;
                ctx.commit(&mut vault)?;
            }
        }
        Cmd::Passwd => {
            let (mut vault, current) = ctx.unlock()?;
            let new = ctx.prompt.new_password("New master password")?;
            vault.change_password(&current, &new, KdfCost::DEFAULT)?;
            ctx.store.save_vault(&vault.to_local_file()?, true)?;
            println!("Master password changed. Other devices will ask for the new password after they sync.");
            ctx.commit(&mut vault)?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
