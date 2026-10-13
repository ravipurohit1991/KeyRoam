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
- [x] Advisory lock on the data directory. Every write merges what another
      KeyRoam process (such as the CLI) saved first, so no edit is lost.
- [x] Lock when Windows locks or suspends, and when macOS locks the screen,
      sleeps or switches user. A KeyRoam app icon (`tools/make_icons.py`).
- [ ] Verified on a real macOS machine (so far it is only built in CI).

## ✅ 5. Desktop browser autofill

`browser-extension/`, `crates/keyroam-autofill` and `crates/keyroam-native-host`.

- [x] Manifest V3 extension for Chrome, Edge, Brave, Vivaldi (service worker)
      and Firefox 115+ (background scripts), built by `browser-extension/build.mjs`.
- [x] Native-messaging host bundled with the desktop app. It relays to the
      running app over a loopback bridge authenticated by a mutual HMAC
      challenge-response. The app answers only while unlocked, and only with
      logins of the requesting page's site. Browser integration is an opt-in
      setting with a "Register with browsers" button
      (`keyroam-native-host install` does the same from a terminal).
- [x] Public-suffix-aware origin matching (private section included). https
      logins are never offered on http pages. Filling an http page or a form
      in a frame of another site needs explicit consent; the engine re-checks.
- [x] Save or update prompts after form submission, in an extension-page
      iframe the website cannot script. Nothing is filled without a user action.
- [x] Tests: origin matching, protocol and bridge (Rust); the host binary end to
      end; form detection, the native client and the background flows (Node);
      and a real-Chromium test that fills, submits and saves through the real
      host and bridge.

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
- [x] Vault-key rotation (other devices adopt the new key with the master
      password), permanent purge that reaches every device, and an
      inter-process lock (see docs/security.md).
- [ ] External security review and fixes.
- [ ] Google OAuth app verification.
- [ ] Signed packages: MSIX, notarised DMG, Play Store, App Store.
- [ ] Choose a license.
