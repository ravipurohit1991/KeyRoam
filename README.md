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
| 5 | Desktop browser autofill extension | ✅ Done: `browser-extension/` + `keyroam-native-host`, with tests |
| 6 | Android app + Credential Manager provider | 🔨 App builds in CI (APK/AAB); sign-in & autofill pending |
| 7 | iOS app + AutoFill Credential Provider extension | 🔨 App builds in CI (unsigned); sign-in & autofill pending |
| 8 | Cross-device tests, security review, packaging | Planned |

The desktop app (`app/`) and the command-line client (`keyroam`) share the
same Rust engine. See [ROADMAP.md](ROADMAP.md).

## Repository layout

```
crates/
  keyroam-core/   Vault engine: Argon2id, XChaCha20-Poly1305, vault model,
                  conflict-free merge, backups, local storage. No network code.
  keyroam-sync/   Google OAuth (PKCE, loopback redirect), Drive v3 appDataFolder
                  client, sync protocol, onboarding flows.
  keyroam-cli/    `keyroam` command-line client.
  keyroam-ffi/    C ABI (JSON request/response) used by the Flutter app.
  keyroam-autofill/    Browser autofill: public-suffix-aware origin matching,
                       the native-messaging protocol, the app's local channel.
  keyroam-native-host/ Native-messaging host started by the browsers.
app/              Flutter desktop app (Windows, macOS). See app/README.md.
browser-extension/ Manifest V3 extension for Chromium browsers and Firefox.
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
| `KeyRoam-ios-unsigned` | Unsigned `.ipa`. It needs re-signing before it can be installed |
| `KeyRoam-extension` | Browser extension for Chrome, Edge and Brave (`-chromium.zip`) and Firefox (`-firefox.zip`) |
| `keyroam-cli-*` | Command-line client for Linux, Windows and macOS |

Pushing a tag such as `v0.2.0` publishes the same files as a GitHub
pre-release. To embed a Google OAuth client into the desktop apps and the
CLI, add the repository secrets `KEYROAM_GOOGLE_CLIENT_ID` and
`KEYROAM_GOOGLE_CLIENT_SECRET`.

## Build and test

You need a stable Rust toolchain (<https://rustup.rs>).

```sh
cargo test --workspace      # 131 tests: crypto, merge, sync, OAuth, FFI, CLI, autofill, native host
cargo build --release -p keyroam-cli
```

The tests never contact Google. OAuth and Drive code is exercised against local
HTTP stand-ins for Google's endpoints. The tests use only synthetic secrets.

## Desktop app

```sh
cd app
flutter run -d windows      # or -d macos; needs Flutter 3.47+
flutter test                # real-engine FFI tests + UI flow tests
```

See [app/README.md](app/README.md) for the full details.

## Browser autofill

The KeyRoam extension fills and saves logins in Chrome, Edge, Brave and
Firefox. It talks only to the desktop app on the same computer, and only
while the app is unlocked. See
[browser-extension/README.md](browser-extension/README.md).

```sh
cd browser-extension
npm test                    # form, frame, prompt, protocol and manifest tests
npm run build               # dist/chromium and dist/firefox
```

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
   keyroam conflicts  /  keyroam resolve <id> <n>
   keyroam sync       # Sync Now
   keyroam status     # sync status, no password needed
   keyroam export backup.krbackup   # independent encrypted backup
   keyroam import backup.krbackup   # merge a backup back in (never deletes data)
   keyroam passwd     # change master password on all devices
   keyroam generate --length 24
   ```

The vault is stored in the platform data directory (`%APPDATA%\KeyRoam`,
`~/Library/Application Support/KeyRoam`, `~/.local/share/KeyRoam`). Override it
with `--data-dir` or `KEYROAM_HOME`.

## License

No license has been chosen yet. All rights reserved until one is added.
