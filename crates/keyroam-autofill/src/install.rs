//! Registering the native-messaging host with the user's browsers.
//!
//! Each browser looks the host up by [`HOST_NAME`] in a JSON manifest that
//! names the host executable and the extensions allowed to start it. On macOS
//! and Linux the manifest lives in a per-browser folder of the user's home;
//! on Windows it can live anywhere and a per-user registry value points to it.
//! Only the current user is affected; nothing needs administrator rights.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Native-messaging host name (lower case, dots allowed).
pub const HOST_NAME: &str = "io.github.ravipurohit1991.keyroam";
/// ID of the extension when loaded from `browser-extension/` (fixed by the
/// public key in its manifest).
pub const CHROMIUM_EXTENSION_ID: &str = "ihplgegclknoghjpemmnfaoeogcahpba";
/// Firefox add-on ID (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "keyroam@ravipurohit1991.github.io";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// Where one browser expects the host manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub browser: &'static str,
    pub family: Family,
    pub manifest: PathBuf,
    /// Windows only: `HKCU\…` key whose default value is the manifest path.
    pub registry_key: Option<String>,
}

struct Known {
    browser: &'static str,
    family: Family,
    /// macOS: under `~/Library/Application Support`. Linux: under `~`.
    mac: &'static str,
    linux: &'static str,
    windows_key: Option<&'static str>,
}

const BROWSERS: &[Known] = &[
    Known {
        browser: "Google Chrome",
        family: Family::Chromium,
        mac: "Google/Chrome",
        linux: ".config/google-chrome",
        // Brave and Vivaldi also read Chrome's key on Windows.
        windows_key: Some(r"Software\Google\Chrome\NativeMessagingHosts"),
    },
    Known {
        browser: "Chromium",
        family: Family::Chromium,
        mac: "Chromium",
        linux: ".config/chromium",
        windows_key: Some(r"Software\Chromium\NativeMessagingHosts"),
    },
    Known {
        browser: "Microsoft Edge",
        family: Family::Chromium,
        mac: "Microsoft Edge",
        linux: ".config/microsoft-edge",
        windows_key: Some(r"Software\Microsoft\Edge\NativeMessagingHosts"),
    },
    Known {
        browser: "Brave",
        family: Family::Chromium,
        mac: "BraveSoftware/Brave-Browser",
        linux: ".config/BraveSoftware/Brave-Browser",
        windows_key: None,
    },
    Known {
        browser: "Vivaldi",
        family: Family::Chromium,
        mac: "Vivaldi",
        linux: ".config/vivaldi",
        windows_key: None,
    },
    Known {
        browser: "Firefox",
        family: Family::Firefox,
        mac: "Mozilla",
        linux: ".mozilla",
        windows_key: Some(r"Software\Mozilla\NativeMessagingHosts"),
    },
];

/// Manifest locations for the browsers that appear to be installed (on
/// Windows: all supported browsers, since registry values are harmless).
/// `windows_dir` is where Windows manifests are written.
pub fn targets(platform: Platform, home: &Path, windows_dir: &Path) -> Vec<Target> {
    let file = format!("{HOST_NAME}.json");
    BROWSERS
        .iter()
        .filter_map(|b| {
            let (profile, hosts) = match (platform, b.family) {
                (Platform::Windows, _) => {
                    let key = b.windows_key?;
                    let name = match b.family {
                        Family::Chromium => "chromium",
                        Family::Firefox => "firefox",
                    };
                    return Some(Target {
                        browser: b.browser,
                        family: b.family,
                        manifest: windows_dir.join(format!("{name}.json")),
                        registry_key: Some(format!(r"HKCU\{key}\{HOST_NAME}")),
                    });
                }
                (Platform::MacOs, family) => {
                    let support = home.join("Library/Application Support");
                    let profile = match family {
                        // Firefox's profile folder is separate from Mozilla's
                        // shared NativeMessagingHosts folder.
                        Family::Firefox => support.join("Firefox"),
                        Family::Chromium => support.join(b.mac),
                    };
                    (profile, support.join(b.mac).join("NativeMessagingHosts"))
                }
                (Platform::Linux, Family::Chromium) => {
                    let profile = home.join(b.linux);
                    let hosts = profile.join("NativeMessagingHosts");
                    (profile, hosts)
                }
                (Platform::Linux, Family::Firefox) => {
                    let profile = home.join(b.linux);
                    let hosts = profile.join("native-messaging-hosts");
                    (profile, hosts)
                }
            };
            profile.is_dir().then(|| Target {
                browser: b.browser,
                family: b.family,
                manifest: hosts.join(&file),
                registry_key: None,
            })
        })
        .collect()
}

/// The host manifest for one browser family. `extra_ids` adds extension IDs
/// (for example a store-assigned Chromium ID) to the built-in one.
pub fn manifest(family: Family, host_path: &Path, extra_ids: &[String]) -> Value {
    let mut m = json!({
        "name": HOST_NAME,
        "description": "KeyRoam password manager",
        "path": host_path.display().to_string(),
        "type": "stdio",
    });
    match family {
        Family::Chromium => {
            let origins: Vec<String> = std::iter::once(CHROMIUM_EXTENSION_ID)
                .chain(extra_ids.iter().map(String::as_str).filter(|id| !id.contains('@')))
                .map(|id| format!("chrome-extension://{id}/"))
                .collect();
            m["allowed_origins"] = json!(origins);
        }
        Family::Firefox => {
            let ids: Vec<&str> = std::iter::once(FIREFOX_EXTENSION_ID)
                .chain(extra_ids.iter().map(String::as_str).filter(|id| id.contains('@')))
                .collect();
            m["allowed_extensions"] = json!(ids);
        }
    }
    m
}

/// Writes or removes a Windows registry value (`None` removes the key).
pub type RegistryWriter<'a> = &'a mut dyn FnMut(&str, Option<&Path>) -> io::Result<()>;

/// Write the host manifest for every target.
pub fn install_to(
    targets: &[Target],
    host_path: &Path,
    extra_ids: &[String],
    registry: RegistryWriter<'_>,
) -> io::Result<()> {
    if !host_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the host path must be absolute",
        ));
    }
    for t in targets {
        if let Some(parent) = t.manifest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec_pretty(&manifest(t.family, host_path, extra_ids)).map_err(io::Error::other)?;
        std::fs::write(&t.manifest, body)?;
        if let Some(key) = &t.registry_key {
            registry(key, Some(&t.manifest))?;
        }
    }
    Ok(())
}

/// Remove the manifests (and registry values) for every target.
pub fn uninstall_from(targets: &[Target], registry: RegistryWriter<'_>) -> io::Result<()> {
    for t in targets {
        match std::fs::remove_file(&t.manifest) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        if let Some(key) = &t.registry_key {
            registry(key, None)?;
        }
    }
    Ok(())
}

/// The real Windows registry, through `reg.exe` (per-user keys only).
pub fn system_registry(key: &str, manifest: Option<&Path>) -> io::Result<()> {
    if !cfg!(windows) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no registry on this platform",
        ));
    }
    let mut cmd = std::process::Command::new("reg");
    match manifest {
        Some(m) => cmd.args(["add", key, "/ve", "/t", "REG_SZ", "/d"]).arg(m).arg("/f"),
        None => cmd.args(["delete", key, "/f"]),
    };
    let out = cmd.output()?;
    // Deleting a key that does not exist is not an error.
    if out.status.success() || manifest.is_none() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "reg.exe failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Targets for this user on this machine. Windows manifests go to
/// `<data dir>/native-messaging`.
pub fn current_targets() -> io::Result<Vec<Target>> {
    let home = dirs::home_dir().ok_or_else(|| io::Error::other("cannot determine the home folder"))?;
    let windows_dir = dirs::data_dir()
        .map(|d| d.join("KeyRoam").join("native-messaging"))
        .unwrap_or_else(|| home.join("KeyRoam-native-messaging"));
    Ok(targets(Platform::current(), &home, &windows_dir))
}

/// Register `host_path` with this user's browsers. Returns the browsers.
pub fn install(host_path: &Path, extra_ids: &[String]) -> io::Result<Vec<Target>> {
    let t = current_targets()?;
    install_to(&t, host_path, extra_ids, &mut system_registry)?;
    Ok(t)
}

/// Unregister the host from this user's browsers.
pub fn uninstall() -> io::Result<Vec<Target>> {
    let t = current_targets()?;
    uninstall_from(&t, &mut system_registry)?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests() {
        let host = Path::new("/opt/KeyRoam/keyroam-native-host");
        let extra = vec![
            "abcdefghijklmnopabcdefghijklmnop".to_string(),
            "other@example.test".to_string(),
        ];
        let c = manifest(Family::Chromium, host, &extra);
        assert_eq!(c["name"], HOST_NAME);
        assert_eq!(c["type"], "stdio");
        assert_eq!(c["path"], "/opt/KeyRoam/keyroam-native-host");
        assert_eq!(
            c["allowed_origins"],
            json!([
                format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/"),
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/"
            ])
        );
        assert!(c.get("allowed_extensions").is_none());
        let f = manifest(Family::Firefox, host, &extra);
        assert_eq!(
            f["allowed_extensions"],
            json!([FIREFOX_EXTENSION_ID, "other@example.test"])
        );
        assert!(f.get("allowed_origins").is_none());
    }

    #[test]
    fn linux_and_macos_targets_follow_installed_browsers() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert!(targets(Platform::Linux, h, h).is_empty());
        std::fs::create_dir_all(h.join(".config/google-chrome")).unwrap();
        std::fs::create_dir_all(h.join(".mozilla")).unwrap();
        let t = targets(Platform::Linux, h, h);
        let names: Vec<_> = t.iter().map(|t| t.browser).collect();
        assert_eq!(names, vec!["Google Chrome", "Firefox"]);
        assert_eq!(
            t[0].manifest,
            h.join(".config/google-chrome/NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json"))
        );
        assert_eq!(
            t[1].manifest,
            h.join(".mozilla/native-messaging-hosts")
                .join(format!("{HOST_NAME}.json"))
        );

        let support = h.join("Library/Application Support");
        std::fs::create_dir_all(support.join("Microsoft Edge")).unwrap();
        std::fs::create_dir_all(support.join("Firefox")).unwrap();
        let t = targets(Platform::MacOs, h, h);
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[0].manifest,
            support
                .join("Microsoft Edge/NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json"))
        );
        assert_eq!(
            t[1].manifest,
            support
                .join("Mozilla/NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json"))
        );

        // Install and uninstall round trip.
        let host = h.join("bin/keyroam-native-host");
        let mut no_registry = |_: &str, _: Option<&Path>| -> io::Result<()> { panic!("no registry on macOS") };
        install_to(&t, &host, &[], &mut no_registry).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&t[1].manifest).unwrap()).unwrap();
        assert_eq!(written["allowed_extensions"][0], FIREFOX_EXTENSION_ID);
        assert!(install_to(&t, Path::new("relative/host"), &[], &mut no_registry).is_err());
        uninstall_from(&t, &mut no_registry).unwrap();
        assert!(!t[0].manifest.exists() && !t[1].manifest.exists());
        uninstall_from(&t, &mut no_registry).unwrap();
    }

    #[test]
    fn windows_uses_registry_values() {
        let dir = tempfile::tempdir().unwrap();
        let t = targets(Platform::Windows, Path::new("C:/Users/x"), dir.path());
        let keys: Vec<_> = t.iter().map(|t| t.registry_key.clone().unwrap()).collect();
        assert_eq!(
            keys,
            vec![
                format!(r"HKCU\Software\Google\Chrome\NativeMessagingHosts\{HOST_NAME}"),
                format!(r"HKCU\Software\Chromium\NativeMessagingHosts\{HOST_NAME}"),
                format!(r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\{HOST_NAME}"),
                format!(r"HKCU\Software\Mozilla\NativeMessagingHosts\{HOST_NAME}"),
            ]
        );
        let mut calls = Vec::new();
        let host = dir.path().join("keyroam-native-host.exe");
        install_to(&t, &host, &[], &mut |k: &str, m: Option<&Path>| {
            calls.push((k.to_string(), m.map(Path::to_path_buf)));
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0].1.as_deref(), Some(dir.path().join("chromium.json").as_path()));
        assert_eq!(calls[3].1.as_deref(), Some(dir.path().join("firefox.json").as_path()));
        assert!(dir.path().join("chromium.json").exists());
    }
}
