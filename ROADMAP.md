# KeyRoam roadmap

**Product principle:** one app, one encrypted vault, one cloud sync service
(Google Drive), every device.

**Not planned:** Dropbox, GitHub sync, peer-to-peer or LAN sync, device
discovery or pairing, self-hosted servers, provider selection, or optional sync
architectures.

Each milestone must ship runnable software with automated tests. No mock
security behaviour may be used for real secrets, and only synthetic passwords
may be used until the security review is complete.

## ✅ 1. Encrypted local vault

`keyroam-core`: Argon2id with downgrade protection, XChaCha20-Poly1305 with
context-bound associated data, a wrapped vault key, an authenticated header,
the entry model with CRDT merge, history, deletion and restore, encrypted
backup export and import, atomic storage with rotating backups, and a password
generator. It has 42 unit and property tests.

## ✅ 2. Google OAuth

`keyroam-sync::oauth`: PKCE (S256, checked against the RFC 7636 test vector), a
loopback redirect receiver with constant-time `state` checking, code exchange,
refresh, revocation, scope verification, HTTPS-only endpoints, and a refresh
token stored encrypted under the vault key.

## ✅ 3. Encrypted Google Drive sync

`keyroam-sync::engine` and `drive`: per-device replicas in `appDataFolder`,
header adoption for password changes, offline-first operation, sync status,
retries with backoff, and new-user and existing-user onboarding. It has 27 tests,
including an end-to-end run of the real HTTP client against a local Drive
stand-in. The `keyroam` CLI exposes all of it and has 6 integration tests.

## ✅ 4. Windows and macOS desktop apps

`app/` (Flutter 3.47) together with `crates/keyroam-ffi`. The FFI layer is a
two-function C ABI that carries JSON requests and responses; a Dart build hook
compiles and bundles it, and calls run on background isolates.

- [x] Onboarding follows the required flows exactly: new user (password,
      Google, create and sync) and existing user (Google, download, password).
- [x] Dashboard, search, entry editor, password generator, password history,
      conflict resolver, recently deleted, Sync Now with a status chip,
      encrypted backup export and restore, master-password change, and
      Google sign-in and sign-out.
- [x] Auto-lock after inactivity. The clipboard is cleared after 30 s.
- [x] Sync on unlock, after edits (debounced), periodically, and by retry when
      connectivity returns.
- [x] Tests: 9 Rust FFI tests; native Dart tests that run against the real
      engine; 8 widget tests of the UI flows. CI builds on Windows and macOS.
- [ ] Native file picker for backups (`file_selector`). Flutter plugins need
      Windows Developer Mode on the build machine, so this is deferred.
      Under the macOS sandbox, exports currently go to the app container.
- [ ] Advisory file lock on the data directory.
- [ ] Lock when the OS locks or sleeps. A KeyRoam app icon.
- [ ] Verified on a real macOS machine (so far it is only built in CI).

## ✅ 5. Desktop browser autofill

`browser-extension/`, `crates/keyroam-autofill` and `crates/keyroam-native-host`.
The extension talks to `keyroam-native-host` over native messaging. The host
relays each message to the running desktop app over a loopback channel that
requires a token, and the app answers from its unlocked vault.

- [x] Manifest V3 extension for Chromium-based browsers (Chrome, Edge, Brave)
      and Firefox, built from one source with `node build.mjs`. A pinned
      public key gives the Chromium extension a stable id.
- [x] Native-messaging host bundled with the desktop app (Windows zip and
      `KeyRoam.app/Contents/MacOS`). **Browser autofill… → Set up browsers**
      in the app, or `keyroam-native-host install`, registers it with each
      browser. The extension asks only about the active tab, KeyRoam re-checks
      every fill against that page's site, and nothing is answered while the
      app is locked.
- [x] Public-suffix-aware origin matching (registrable domain from the Public
      Suffix List, including private suffixes such as `github.io`). Logins
      saved for https are never offered on http. Filling on an HTTP page, or
      into a form framed from another origin, needs the user to tick
      "Fill anyway" each time.
- [x] Save and update prompts after form submission, in a closed shadow root
      that only responds to real clicks. Saved logins show up in the app and
      sync.
- [x] Tests: 38 Rust tests (origin matching, protocol and framing, the
      service, the loopback channel, host registration), 4 integration tests
      that drive the real host executable, FFI and Flutter tests for the app
      side, and 24 Node tests for the extension's form, frame, prompt,
      protocol and manifest logic.
- [ ] Verified by hand in each browser (so far only automated tests and CI
      builds).
- [ ] Published to the Chrome Web Store, Edge Add-ons and Firefox Add-ons
      (milestone 8).

## 6. Android

- [x] Flutter Android target. The Rust engine is cross-compiled for arm64,
      armv7 and x86_64 with the NDK through the build hook. CI builds the APK
      and AAB. Storage paths come from `MainActivity`.
- [ ] Mobile Google sign-in: an Android OAuth client with a custom-scheme redirect and PKCE.
      For now the app opens the desktop loopback flow in the browser.
- [ ] `CredentialProviderService` (Android 14+) with an `AutofillService`
      fallback.
- [ ] WorkManager periodic sync and a connectivity callback.
- [ ] Biometric unlock (Keystore key wrapping a device-local copy of the KEK).

## 7. iOS

- [x] Flutter iOS target. The Rust engine is built for device and simulator.
      CI builds an unsigned app. The Files app can reach backups.
- [ ] Mobile Google sign-in through ASWebAuthenticationSession with PKCE (iOS OAuth client).
- [ ] AutoFill Credential Provider extension sharing an App Group container.
- [ ] BGAppRefreshTask for sync. Face ID unlock.

## 8. Release preparation

- [ ] Cross-device test matrix: Windows, macOS, Android and iOS editing the
      same vault, including offline periods and password changes.
- [ ] Vault-key rotation, permanent purge, and an inter-process lock (see
      docs/security.md).
- [ ] External security review and fixes.
- [ ] Google OAuth app verification.
- [ ] Signed packages: MSIX, notarised DMG, Play Store, App Store.
- [ ] Choose a license.
