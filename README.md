# KeyRoam

**Your keys. Wherever you go.**

KeyRoam is a privacy-focused, local-first password manager for Windows, macOS,
Android and iOS. One app, one encrypted vault, one cloud sync service (your own
Google Drive), every device.

- **Local-first.** The vault lives encrypted on each device and works fully offline.
- **End-to-end encrypted.** Everything is encrypted on the device before it is
  uploaded. Google only ever stores ciphertext.
- **No KeyRoam servers.** Devices talk directly to Google Drive's hidden,
  app-specific `appDataFolder`, using the least-privilege `drive.appdata` scope.
- **No lost updates.** If two devices change the same entry while apart, both
  versions are kept and you choose which to keep. Nothing is silently overwritten.

> **Status: pre-release.** Use only synthetic (made-up) passwords until KeyRoam
> has passed a security review. See [SECURITY.md](SECURITY.md).

## Progress

| # | Milestone | Status |
|---|-----------|--------|
| 1 | Encrypted local vault (Rust) | ✅ Done: `keyroam-core`, with tests |
| 2 | Google OAuth 2.0 + PKCE | ✅ Done: `keyroam-sync::oauth`, with tests |
| 3 | Encrypted Google Drive sync | ✅ Done: `keyroam-sync::engine` / `drive`, with tests |
| 4 | Windows and macOS desktop apps (Flutter) | ✅ Done: `app/` + `keyroam-ffi`, with tests (macOS built in CI only) |
| 5 | Desktop browser autofill extension | ✅ Done: `browser-extension/` + `keyroam-native-host`, with unit and real-browser tests |
| 6 | Android app + Credential Manager provider | ✅ Done: autofill and Credential Manager, mobile sign-in, biometric unlock, background sync |
| 7 | iOS app + AutoFill Credential Provider extension | ✅ Done: AutoFill extension, QuickType suggestions, Face ID, sign-in, background sync (unsigned CI build) |
| 8 | Cross-device tests, security review, packaging | 🔨 Key rotation, permanent purge and inter-process lock done; review and packaging pending |

The desktop app (`app/`), the browser extension and the command-line client
(`keyroam`) share the same Rust engine. See [ROADMAP.md](ROADMAP.md).

## Repository layout

```
crates/
  keyroam-core/   Vault engine: Argon2id, XChaCha20-Poly1305, vault model,
                  conflict-free merge, backups, local storage. No network code.
  keyroam-sync/   Google OAuth (PKCE, loopback redirect), Drive v3 appDataFolder
                  client, sync protocol, onboarding flows.
  keyroam-autofill/  Browser autofill: public-suffix-aware site matching, the
                  extension protocol, the authenticated app bridge.
  keyroam-native-host/  Native-messaging host for the browser extension.
  keyroam-cli/    `keyroam` command-line client.
  keyroam-ffi/    C ABI (JSON request/response) used by the Flutter app.
app/              Flutter app (Windows, macOS; Android and iOS builds). See app/README.md.
browser-extension/  Manifest V3 extension for Chromium browsers and Firefox.
tools/            Icon generator.
docs/
  architecture.md       Components and how the apps fit together
  security.md           Cryptographic design, threat model, limitations
  sync-protocol.md      The Drive file layout and merge protocol
  google-cloud-setup.md Creating the Google OAuth client
```

## Downloads (CI builds)

The [Build apps](.github/workflows/build.yml) workflow builds every app on
each push to `main`. You can download them from the run page under
**Actions → Build apps → Artifacts**:

| Artifact | Contents |
|---|---|
| `KeyRoam-windows-x64` | Windows app (zip; run `keyroam.exe`) |
| `KeyRoam-macos-universal` | macOS app, Apple silicon and Intel (unsigned) |
| `KeyRoam-android` | `KeyRoam-android.apk` and `.aab` (signed with a debug key) |
| `KeyRoam-ios-unsigned` | Unsigned `.ipa` with the AutoFill extension. It needs re-signing (with the App Group and AutoFill capabilities) before it can be installed |
| `KeyRoam-extension` | Browser extension: `KeyRoam-extension-chromium.zip` and `-firefox.zip` |
| `keyroam-cli-*` | Command-line client and `keyroam-native-host` for Linux, Windows and macOS |

Pushing a tag such as `v0.2.0` publishes the same files as a GitHub
pre-release. To embed a Google OAuth client into the desktop apps and the
CLI, add the repository secrets `KEYROAM_GOOGLE_CLIENT_ID` and
`KEYROAM_GOOGLE_CLIENT_SECRET`.

## Build and test

You need a stable Rust toolchain (<https://rustup.rs>).

```sh
cargo test --workspace      # 153 tests: crypto, merge properties, sync, OAuth, autofill, import, FFI, CLI, host
cargo build --release -p keyroam-cli
```

The browser extension has its own tests (`cd browser-extension && npm test`)
and a real-Chromium end-to-end test; see
[browser-extension/README.md](browser-extension/README.md).

The tests never contact Google. OAuth and Drive code is exercised against local
HTTP stand-ins for Google's endpoints. The tests use only synthetic secrets.

## Desktop app

```sh
cd app
flutter run -d windows      # or -d macos; needs Flutter 3.47+
flutter test                # real-engine FFI tests + UI flow tests
```

See [app/README.md](app/README.md) for the full details.

## Moving your passwords in

Export your passwords as CSV from your browser or password manager, then use
**Menu → Import passwords from CSV…** in the app (or
`keyroam import-csv <file> --delete`). KeyRoam reads the exports of Chrome,
Edge, Brave and other Chromium browsers, Firefox, Safari, Bitwarden, LastPass,
1Password, KeePass and KeePassXC. Logins already in KeyRoam are skipped and
nothing is overwritten. The CSV file is unencrypted: let KeyRoam delete it
afterwards, and empty the trash.

| Source | How to export |
|---|---|
| Chrome, Edge, Brave, Vivaldi | Settings → Passwords / Password Manager → Export passwords |
| Firefox | `about:logins` → ••• → Export Passwords |
| Safari (macOS) | File → Export → Passwords |
| Bitwarden, LastPass, 1Password, KeePass(XC) | Their "Export → CSV" option |

## Browser extension

1. In the desktop app, open **Menu → Browser integration…**, turn it on and
   click **Register with browsers**.
2. Load the extension from the `KeyRoam-extension` download (or build it with
   `node browser-extension/build.mjs`) in Chrome, Edge, Brave, Vivaldi or
   Firefox. In Firefox, use `KeyRoam-extension-firefox.zip` (or
   `dist/firefox`) under `about:debugging` → **Load Temporary Add-on…**.
3. On a login page, click the KeyRoam toolbar button and pick a login, or
   press Ctrl/⌘+Shift+L. After you sign in with a new or changed password,
   KeyRoam offers to save it.

The extension never sees the vault. It only gets the login you pick, for the
site you are on, while the app is unlocked. See
[browser-extension/README.md](browser-extension/README.md).

## Using the CLI

1. Create a Google OAuth client and set its ID. See
   [docs/google-cloud-setup.md](docs/google-cloud-setup.md).

   ```sh
   export KEYROAM_GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
   export KEYROAM_GOOGLE_CLIENT_SECRET=...   # issued for "Desktop app" clients
   ```

2. **New user:**

   ```sh
   keyroam init       # master password -> Google sign-in -> encrypted vault synced to Drive
   ```

   **Existing user on another device:**

   ```sh
   keyroam join       # Google sign-in -> download vault -> master password -> unlocked
   ```

3. Everyday use (each command syncs automatically; pass `--no-sync` to skip):

   ```sh
   keyroam add --title "Example" --username alice@example.com --url https://example.com
   keyroam list [--search term]
   keyroam show <id> [--reveal] [--history]
   keyroam edit <id> --generate
   keyroam rm <id>  /  keyroam restore <id>
   keyroam purge <id> | --all   # erase deleted entries on every device
   keyroam conflicts  /  keyroam resolve <id> <n>
   keyroam sync       # Sync Now
   keyroam status     # sync status, no password needed
   keyroam export backup.krbackup   # independent encrypted backup
   keyroam import backup.krbackup   # merge a backup back in (never deletes data)
   keyroam import-csv passwords.csv --delete   # import a browser's / password manager's CSV export
   keyroam passwd     # change master password on all devices
   keyroam rotate-key # replace the vault key (other devices ask for the password once)
   keyroam generate --length 24
   ```

The vault is stored in the platform data directory (`%APPDATA%\KeyRoam`,
`~/Library/Application Support/KeyRoam`, `~/.local/share/KeyRoam`). Override it
with `--data-dir` or `KEYROAM_HOME`.

## License

No license has been chosen yet. All rights reserved until one is added.
