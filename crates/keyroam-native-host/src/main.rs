//! `keyroam-native-host`: the native-messaging host of the KeyRoam browser
//! extension.
//!
//! The browser starts it when the extension connects and talks to it over
//! stdin/stdout with length-prefixed JSON. The host holds no secrets and no
//! vault: it relays each message over the authenticated local bridge to the
//! running KeyRoam desktop app, which decides what to answer.
//!
//! ```text
//! keyroam-native-host install [--extension-id ID]...   register with this user's browsers
//! keyroam-native-host uninstall                         remove the registration
//! ```

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use keyroam_autofill::bridge::{candidate_data_dirs, connect_any, BridgeClient, ConnectError};
use keyroam_autofill::framing::{read_frame, write_frame};
use keyroam_autofill::install::{self, HOST_NAME};
use keyroam_autofill::protocol::{code, error_response, parse_request, request_id};

const USAGE: &str = "\
keyroam-native-host — connects the KeyRoam browser extension to the KeyRoam app.

Browsers start this program themselves. To register it with your browsers:

    keyroam-native-host install [--extension-id ID]...
    keyroam-native-host uninstall

--extension-id adds an extension allowed to use KeyRoam besides the built-in
ones (a Chromium ID, or a Firefox add-on ID containing '@').";

/// Relay messages from the extension to the app until the browser closes
/// stdin. Each message is answered, with `app_not_running` when no app is
/// reachable.
fn relay(input: &mut impl Read, output: &mut impl Write, dirs: &[PathBuf]) -> io::Result<()> {
    let mut client: Option<BridgeClient> = None;
    while let Some(mut message) = read_frame(input)? {
        let response = match parse_request(&message) {
            Err(invalid) => invalid,
            Ok((_, request)) => {
                // A read-only request can be retried once on a fresh connection
                // (the app may have restarted); a save is never sent twice.
                let attempts = if request.is_write() { 1 } else { 2 };
                forward(&mut client, &message, dirs, attempts)
            }
        };
        message.fill(0);
        write_frame(output, &response)?;
    }
    Ok(())
}

fn forward(client: &mut Option<BridgeClient>, message: &[u8], dirs: &[PathBuf], attempts: usize) -> Vec<u8> {
    let mut failure = ConnectError::NotRunning;
    for _ in 0..attempts {
        let c = match client {
            Some(c) => c,
            None => match connect_any(dirs) {
                Ok(c) => client.insert(c),
                Err(e) => {
                    failure = e;
                    break;
                }
            },
        };
        match c.call(message) {
            Ok(r) => return r,
            Err(e) => {
                *client = None;
                failure = ConnectError::Io(e);
            }
        }
    }
    let rid = request_id(message);
    match failure {
        ConnectError::NotRunning => error_response(
            &rid,
            code::APP_NOT_RUNNING,
            "Open the KeyRoam app and turn on browser integration",
        ),
        e => error_response(&rid, code::APP_NOT_RUNNING, &e.to_string()),
    }
}

fn run_install(args: &[String]) -> Result<(), String> {
    let mut extra = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--extension-id" => extra.push(it.next().ok_or("--extension-id needs a value")?.clone()),
            other => return Err(format!("unknown option {other}\n\n{USAGE}")),
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let targets = install::install(&exe, &extra).map_err(|e| e.to_string())?;
    if targets.is_empty() {
        println!("No supported browser found (Chrome, Chromium, Edge, Brave, Vivaldi, Firefox).");
    }
    for t in targets {
        println!("{}: {}", t.browser, t.manifest.display());
    }
    println!("Registered {HOST_NAME} → {}", exe.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("install") => run_install(&args[1..]),
        Some("uninstall") => install::uninstall()
            .map(|targets| {
                for t in targets {
                    println!("{}: removed {}", t.browser, t.manifest.display());
                }
            })
            .map_err(|e| e.to_string()),
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("keyroam-native-host {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // Started by a browser: Chromium passes the extension origin, Firefox
        // the manifest path and add-on ID. The browser already restricted the
        // caller to the extensions listed in the manifest.
        _ => relay(
            &mut io::stdin().lock(),
            &mut io::stdout().lock(),
            &candidate_data_dirs(),
        )
        .map_err(|e| e.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("keyroam-native-host: {e}");
            ExitCode::FAILURE
        }
    }
}
