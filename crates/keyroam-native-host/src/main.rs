//! `keyroam-native-host`: the native-messaging host for the KeyRoam browser
//! extension.
//!
//! Browsers start it with the extension's identity as arguments and exchange
//! length-prefixed JSON messages over stdin and stdout. Each message is relayed
//! to the running KeyRoam desktop app over the authenticated loopback channel
//! described in `keyroam_autofill::ipc`; the host itself never opens the vault.
//!
//! ```text
//! keyroam-native-host install [--extension-id <id>]...   register with every browser
//! keyroam-native-host uninstall                          remove the registrations
//! keyroam-native-host status                             show the registrations
//! ```

#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use keyroam_autofill::install::{self, InstallOptions};
use keyroam_autofill::ipc::{self, ClientError};
use keyroam_autofill::protocol::{
    codes, error_response, read_frame, write_json_frame, ProtocolError, MAX_MESSAGE_BYTES,
};
use serde_json::Value;
use zeroize::Zeroize;

const USAGE: &str = "\
keyroam-native-host: connects the KeyRoam browser extension to the KeyRoam app.

Browsers start this program by themselves. To set it up, run:

  keyroam-native-host install [--extension-id <id>]...
      Register with Chrome, Chromium, Edge, Brave and Firefox for this user.
      --extension-id also admits another Chromium extension id (for example
      one assigned by a browser's extension store).
  keyroam-native-host uninstall
      Remove those registrations.
  keyroam-native-host status
      Show which browsers have it registered.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("install") => cli(install(&args[1..])),
        Some("uninstall") => cli(install::uninstall().map(|()| println!("Removed the KeyRoam browser registrations."))),
        Some("status") => {
            print_status(&install::status());
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("keyroam-native-host {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ if args.is_empty() && io::stdin().is_terminal() => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        _ => {
            let caller = caller(&args);
            match serve(
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
                &caller,
                &ipc::candidate_dirs(),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
    }
}

fn cli(result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn install(args: &[String]) -> io::Result<()> {
    let mut extra = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--extension-id" => extra.push(
                it.next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--extension-id needs a value"))?
                    .clone(),
            ),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unexpected argument {other:?}"),
                ))
            }
        }
    }
    let host_path = std::env::current_exe()?.canonicalize()?;
    let regs = install::install(&InstallOptions {
        host_path: strip_verbatim(host_path),
        extra_chromium_ids: extra,
    })?;
    print_status(&regs);
    println!("\nInstall the KeyRoam extension in your browser, then restart the browser.");
    Ok(())
}

/// `canonicalize` on Windows yields `\\?\C:\...`, which browsers do not accept.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn print_status(regs: &[install::Registration]) {
    for r in regs {
        let mark = if r.installed { "registered" } else { "not registered" };
        println!("{:<15} {:<15} {}", r.browser.display_name(), mark, r.location);
    }
}

/// The calling extension, as identified by the browser's launch arguments:
/// Chromium passes its origin (`chrome-extension://<id>/`); Firefox passes the
/// manifest path and then the extension id.
fn caller(args: &[String]) -> String {
    if let Some(origin) = args.iter().find(|a| a.starts_with("chrome-extension://")) {
        return origin.clone();
    }
    match args {
        [_manifest, id, ..] => format!("firefox:{id}"),
        _ => String::from("unknown"),
    }
}

/// Relay messages until the browser closes stdin.
fn serve(input: &mut impl Read, output: &mut impl Write, caller: &str, dirs: &[PathBuf]) -> io::Result<()> {
    loop {
        let mut frame = match read_frame(input, MAX_MESSAGE_BYTES) {
            Ok(Some(f)) => f,
            Ok(None) => return Ok(()),
            Err(e) => {
                // The stream cannot be resynchronised after a bad frame.
                let err = ProtocolError::new(codes::INVALID_REQUEST, e.to_string());
                let _ = write_json_frame(output, &error_response(&Value::Null, &err));
                return Err(e);
            }
        };
        let response = relay(&frame, caller, dirs);
        frame.zeroize();
        write_json_frame(output, &response)?;
    }
}

fn relay(frame: &[u8], caller: &str, dirs: &[PathBuf]) -> Value {
    let request = match serde_json::from_slice::<Value>(frame) {
        Ok(v) if v.is_object() => v,
        _ => {
            return error_response(
                &Value::Null,
                &ProtocolError::new(codes::INVALID_REQUEST, "expected a JSON object"),
            )
        }
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let not_running = || {
        error_response(
            &id,
            &ProtocolError::new(codes::APP_NOT_RUNNING, "open the KeyRoam app to use autofill"),
        )
    };
    let Some(endpoint) = ipc::find_endpoint(dirs) else {
        return not_running();
    };
    match ipc::send(&endpoint, caller, &request) {
        Ok(response) => response,
        Err(ClientError::NotRunning) => not_running(),
        Err(ClientError::Io(e)) => error_response(
            &id,
            &ProtocolError::new(codes::APP_NOT_RUNNING, format!("could not reach the KeyRoam app: {e}")),
        ),
        Err(ClientError::BadResponse) => error_response(
            &id,
            &ProtocolError::new(codes::INTERNAL, "unexpected answer from the KeyRoam app"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_callers() {
        let s = |v: &[&str]| caller(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["chrome-extension://abc/"]), "chrome-extension://abc/");
        assert_eq!(
            s(&["chrome-extension://abc/", "--parent-window=0"]),
            "chrome-extension://abc/"
        );
        assert_eq!(s(&["/path/manifest.json", "keyroam@x"]), "firefox:keyroam@x");
        assert_eq!(s(&[]), "unknown");
    }

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\a\b.exe")),
            PathBuf::from(r"C:\a\b.exe")
        );
        assert_eq!(strip_verbatim(PathBuf::from("/usr/bin/x")), PathBuf::from("/usr/bin/x"));
    }
}
