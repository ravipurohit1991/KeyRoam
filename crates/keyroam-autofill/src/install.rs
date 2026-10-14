//! Registering the native-messaging host with the browsers.
//!
//! Each browser finds native-messaging hosts through a small JSON manifest
//! that names the host executable and the extensions allowed to start it:
//!
//! * **Windows:** the manifest lives in `%LOCALAPPDATA%\KeyRoam\NativeMessagingHosts`
//!   and a per-user registry key (`HKCU\Software\<browser>\NativeMessagingHosts\<name>`)
//!   points to it.
//! * **macOS and Linux:** the manifest is copied into each installed browser's
//!   `NativeMessagingHosts` folder.
//!
//! Only the KeyRoam extension may start the host: Chromium-based browsers
//! check `allowed_origins`, Firefox checks `allowed_extensions`.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::protocol::HOST_NAME;

/// ID of the KeyRoam extension in Chromium-based browsers. It is derived from
/// the public `key` in the extension manifest, so unpacked builds get it too.
pub const CHROMIUM_EXTENSION_ID: &str = "ghgndmpoakmlmcagombpmikjdalnmiec";

/// ID of the KeyRoam extension in Firefox (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "keyroam@ravipurohit1991.github.io";

const DESCRIPTION: &str = "KeyRoam password manager";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Browser {
    Chrome,
    Chromium,
    Edge,
    Brave,
    Firefox,
}

impl Browser {
    pub const ALL: [Browser; 5] = [
        Browser::Chrome,
        Browser::Chromium,
        Browser::Edge,
        Browser::Brave,
        Browser::Firefox,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Chromium => "Chromium",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave",
            Browser::Firefox => "Firefox",
        }
    }

    fn is_firefox(self) -> bool {
        self == Browser::Firefox
    }
}

/// Is `id` a well-formed Chromium extension id (32 letters a–p)?
pub fn is_chromium_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The host manifest for `browser`. `extra_chromium_ids` admits additional
/// Chromium extension ids, for example a store-assigned id.
pub fn host_manifest(browser: Browser, host_path: &Path, extra_chromium_ids: &[String]) -> Value {
    let mut m = json!({
        "name": HOST_NAME,
        "description": DESCRIPTION,
        "path": host_path.display().to_string(),
        "type": "stdio",
    });
    if browser.is_firefox() {
        m["allowed_extensions"] = json!([FIREFOX_EXTENSION_ID]);
    } else {
        let mut ids = vec![CHROMIUM_EXTENSION_ID.to_string()];
        ids.extend(
            extra_chromium_ids
                .iter()
                .filter(|i| *i != CHROMIUM_EXTENSION_ID)
                .cloned(),
        );
        m["allowed_origins"] = json!(ids
            .iter()
            .map(|id| format!("chrome-extension://{id}/"))
            .collect::<Vec<_>>());
    }
    m
}

/// The registration state for one browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Registration {
    pub browser: Browser,
    /// Manifest file or registry key.
    pub location: String,
    pub installed: bool,
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    /// Absolute path of the `keyroam-native-host` executable.
    pub host_path: PathBuf,
    pub extra_chromium_ids: Vec<String>,
}

fn validate(opts: &InstallOptions) -> io::Result<()> {
    if !opts.host_path.is_absolute() || !opts.host_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("native host not found at {}", opts.host_path.display()),
        ));
    }
    if let Some(bad) = opts.extra_chromium_ids.iter().find(|i| !is_chromium_extension_id(i)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{bad:?} is not a Chromium extension id"),
        ));
    }
    Ok(())
}

fn manifest_file_name() -> String {
    format!("{HOST_NAME}.json")
}

fn write_manifest(path: &Path, manifest: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?)
}

// ----- macOS and Linux: one manifest file per browser -----

/// Per-browser profile roots and their manifest folders, relative to `home`.
pub fn unix_layout(home: &Path) -> Vec<(Browser, PathBuf, PathBuf)> {
    let rows: [(Browser, &str, &str); 5] = if cfg!(target_os = "macos") {
        [
            (
                Browser::Chrome,
                "Library/Application Support/Google/Chrome",
                "NativeMessagingHosts",
            ),
            (
                Browser::Chromium,
                "Library/Application Support/Chromium",
                "NativeMessagingHosts",
            ),
            (
                Browser::Edge,
                "Library/Application Support/Microsoft Edge",
                "NativeMessagingHosts",
            ),
            (
                Browser::Brave,
                "Library/Application Support/BraveSoftware/Brave-Browser",
                "NativeMessagingHosts",
            ),
            (
                Browser::Firefox,
                "Library/Application Support/Mozilla",
                "NativeMessagingHosts",
            ),
        ]
    } else {
        [
            (Browser::Chrome, ".config/google-chrome", "NativeMessagingHosts"),
            (Browser::Chromium, ".config/chromium", "NativeMessagingHosts"),
            (Browser::Edge, ".config/microsoft-edge", "NativeMessagingHosts"),
            (
                Browser::Brave,
                ".config/BraveSoftware/Brave-Browser",
                "NativeMessagingHosts",
            ),
            (Browser::Firefox, ".mozilla", "native-messaging-hosts"),
        ]
    };
    rows.iter()
        .map(|(b, root, sub)| {
            (
                *b,
                home.join(root),
                home.join(root).join(sub).join(manifest_file_name()),
            )
        })
        .collect()
}

/// Install manifests for every browser whose profile root exists under `home`.
pub fn install_files(home: &Path, opts: &InstallOptions) -> io::Result<Vec<Registration>> {
    validate(opts)?;
    let mut out = Vec::new();
    for (browser, root, file) in unix_layout(home) {
        if root.is_dir() {
            write_manifest(
                &file,
                &host_manifest(browser, &opts.host_path, &opts.extra_chromium_ids),
            )?;
        }
        out.push(Registration {
            browser,
            installed: file.is_file(),
            location: file.display().to_string(),
        });
    }
    Ok(out)
}

pub fn uninstall_files(home: &Path) -> io::Result<()> {
    for (_, _, file) in unix_layout(home) {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

pub fn status_files(home: &Path) -> Vec<Registration> {
    unix_layout(home)
        .into_iter()
        .map(|(browser, _, file)| Registration {
            browser,
            installed: file.is_file(),
            location: file.display().to_string(),
        })
        .collect()
}

// ----- Windows: manifest files plus HKCU registry keys -----

/// Registry keys (under `HKEY_CURRENT_USER`) that each browser reads.
pub fn windows_registry_keys() -> Vec<(Browser, String)> {
    [
        (Browser::Chrome, r"Software\Google\Chrome"),
        (Browser::Chromium, r"Software\Chromium"),
        (Browser::Edge, r"Software\Microsoft\Edge"),
        (Browser::Brave, r"Software\BraveSoftware\Brave-Browser"),
        (Browser::Firefox, r"Software\Mozilla"),
    ]
    .iter()
    .map(|(b, k)| (*b, format!(r"{k}\NativeMessagingHosts\{HOST_NAME}")))
    .collect()
}

/// Manifest files on Windows, one per browser family.
pub fn windows_manifest_paths(base: &Path) -> (PathBuf, PathBuf) {
    let dir = base.join("KeyRoam").join("NativeMessagingHosts");
    (
        dir.join(format!("{HOST_NAME}.chromium.json")),
        dir.join(format!("{HOST_NAME}.firefox.json")),
    )
}

#[cfg(windows)]
mod registry {
    use std::io;
    use std::process::{Command, Stdio};

    fn reg(args: &[&str]) -> io::Result<std::process::Output> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("reg")
            .args(args)
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    pub fn set_default(key: &str, value: &str) -> io::Result<()> {
        let out = reg(&["add", &format!(r"HKCU\{key}"), "/ve", "/t", "REG_SZ", "/d", value, "/f"])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "reg add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    pub fn exists(key: &str) -> bool {
        reg(&["query", &format!(r"HKCU\{key}"), "/ve"]).is_ok_and(|o| o.status.success())
    }

    pub fn delete(key: &str) {
        let _ = reg(&["delete", &format!(r"HKCU\{key}"), "/f"]);
    }
}

#[cfg(windows)]
fn windows_base() -> io::Result<PathBuf> {
    dirs::data_local_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local app data folder"))
}

#[cfg(not(windows))]
fn home() -> io::Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home folder"))
}

/// Register the host for this user with every supported browser.
pub fn install(opts: &InstallOptions) -> io::Result<Vec<Registration>> {
    #[cfg(windows)]
    {
        validate(opts)?;
        let (chromium, firefox) = windows_manifest_paths(&windows_base()?);
        write_manifest(
            &chromium,
            &host_manifest(Browser::Chrome, &opts.host_path, &opts.extra_chromium_ids),
        )?;
        write_manifest(&firefox, &host_manifest(Browser::Firefox, &opts.host_path, &[]))?;
        for (browser, key) in windows_registry_keys() {
            let file = if browser.is_firefox() { &firefox } else { &chromium };
            registry::set_default(&key, &file.display().to_string())?;
        }
        Ok(status())
    }
    #[cfg(not(windows))]
    {
        install_files(&home()?, opts)
    }
}

/// Remove every registration made by [`install`].
pub fn uninstall() -> io::Result<()> {
    #[cfg(windows)]
    {
        for (_, key) in windows_registry_keys() {
            registry::delete(&key);
        }
        let (chromium, firefox) = windows_manifest_paths(&windows_base()?);
        for f in [chromium, firefox] {
            match std::fs::remove_file(&f) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        uninstall_files(&home()?)
    }
}

/// Which browsers currently have the host registered.
pub fn status() -> Vec<Registration> {
    #[cfg(windows)]
    {
        let Ok(base) = windows_base() else { return Vec::new() };
        let (chromium, firefox) = windows_manifest_paths(&base);
        windows_registry_keys()
            .into_iter()
            .map(|(browser, key)| {
                let file = if browser.is_firefox() { &firefox } else { &chromium };
                Registration {
                    browser,
                    installed: file.is_file() && registry::exists(&key),
                    location: format!(r"HKCU\{key}"),
                }
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        home().map(|h| status_files(&h)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_host(dir: &Path) -> PathBuf {
        let p = dir.join(if cfg!(windows) {
            "keyroam-native-host.exe"
        } else {
            "keyroam-native-host"
        });
        std::fs::write(&p, b"").unwrap();
        p
    }

    #[test]
    fn extension_id_format() {
        assert!(is_chromium_extension_id(CHROMIUM_EXTENSION_ID));
        assert!(!is_chromium_extension_id("abc"));
        assert!(!is_chromium_extension_id(&"z".repeat(32)));
        assert!(!is_chromium_extension_id(&"A".repeat(32)));
    }

    #[test]
    fn manifests_allow_only_keyroam() {
        let host = Path::new("/opt/keyroam/keyroam-native-host");
        let m = host_manifest(Browser::Chrome, host, &[]);
        assert_eq!(m["name"], HOST_NAME);
        assert_eq!(m["type"], "stdio");
        assert_eq!(m["path"], host.display().to_string());
        assert_eq!(
            m["allowed_origins"],
            json!([format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/")])
        );
        assert!(m.get("allowed_extensions").is_none());

        let extra = "a".repeat(32);
        let m = host_manifest(Browser::Edge, host, &[extra.clone(), CHROMIUM_EXTENSION_ID.into()]);
        assert_eq!(m["allowed_origins"].as_array().unwrap().len(), 2);
        assert!(m["allowed_origins"][1].as_str().unwrap().contains(&extra));

        let m = host_manifest(Browser::Firefox, host, &[extra]);
        assert_eq!(m["allowed_extensions"], json!([FIREFOX_EXTENSION_ID]));
        assert!(m.get("allowed_origins").is_none());
    }

    #[test]
    fn host_name_is_valid_for_browsers() {
        // Lower-case alphanumerics, dots and underscores only.
        assert!(HOST_NAME
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'));
    }

    #[test]
    fn file_install_only_for_present_browsers() {
        let home = tempfile::tempdir().unwrap();
        let host = fake_host(home.path());
        let layout = unix_layout(home.path());
        // Pretend Chrome and Firefox are installed.
        for (b, root, _) in &layout {
            if matches!(b, Browser::Chrome | Browser::Firefox) {
                std::fs::create_dir_all(root).unwrap();
            }
        }
        let opts = InstallOptions {
            host_path: host,
            extra_chromium_ids: vec![],
        };
        let regs = install_files(home.path(), &opts).unwrap();
        let installed: Vec<Browser> = regs.iter().filter(|r| r.installed).map(|r| r.browser).collect();
        assert_eq!(installed, [Browser::Chrome, Browser::Firefox]);

        let (_, _, ff) = layout.iter().find(|(b, _, _)| *b == Browser::Firefox).unwrap();
        let m: Value = serde_json::from_slice(&std::fs::read(ff).unwrap()).unwrap();
        assert_eq!(m["allowed_extensions"][0], FIREFOX_EXTENSION_ID);
        assert_eq!(status_files(home.path()), regs);

        uninstall_files(home.path()).unwrap();
        uninstall_files(home.path()).unwrap();
        assert!(status_files(home.path()).iter().all(|r| !r.installed));
    }

    #[test]
    fn install_validates_inputs() {
        let home = tempfile::tempdir().unwrap();
        let missing = InstallOptions {
            host_path: home.path().join("nope"),
            extra_chromium_ids: vec![],
        };
        assert_eq!(
            install_files(home.path(), &missing).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let relative = InstallOptions {
            host_path: PathBuf::from("keyroam-native-host"),
            extra_chromium_ids: vec![],
        };
        assert!(install_files(home.path(), &relative).is_err());
        let bad_id = InstallOptions {
            host_path: fake_host(home.path()),
            extra_chromium_ids: vec!["not-an-id".into()],
        };
        assert_eq!(
            install_files(home.path(), &bad_id).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn windows_locations() {
        let keys = windows_registry_keys();
        assert_eq!(keys.len(), Browser::ALL.len());
        assert!(keys
            .iter()
            .all(|(_, k)| k.ends_with(&format!(r"NativeMessagingHosts\{HOST_NAME}"))));
        let (c, f) = windows_manifest_paths(Path::new("base"));
        assert_ne!(c, f);
    }
}
