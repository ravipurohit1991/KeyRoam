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
- [x] Native open and save dialogs for backups and CSV imports, served by
      the Windows (`IFileDialog`) and macOS (`NSSavePanel`) runners without
      Flutter plugins. Under the macOS sandbox the picked file is what the
      app may access (`files.user-selected.read-write`).
- [x] Advisory lock on the data directory. Every write merges what another
      KeyRoam process (such as the CLI) saved first, so no edit is lost.
- [x] Lock when Windows locks or suspends, and when macOS locks the screen,
      sleeps or switches user. A KeyRoam app icon (`tools/make_icons.py`).
- [x] Import logins from the CSV exports of Chrome/Edge/Brave, Firefox,
      Safari, Bitwarden, LastPass, 1Password and KeePass (app and CLI);
      duplicates are skipped and the plaintext file can be deleted afterwards.
- [ ] Verified on a real macOS machine (so far it is only built in CI).

## ✅ 5. Desktop browser autofill

`browser-extension/`, `crates/keyroam-autofill` and `crates/keyroam-native-host`.

- [x] Manifest V3 extension for Chrome, Edge, Brave, Vivaldi (service worker)
      and Firefox 115+ (background scripts), built by `browser-extension/build.mjs`.
      The manifest declares both background forms, so the source folder and
      either package load in Firefox; the popup asks for website access when
      Firefox has withheld it.
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

## ✅ 6. Android

`app/android` (Kotlin), `crates/keyroam-autofill::android` and the JNI entry
point in `crates/keyroam-ffi`. Kotlin calls the same process-wide Rust engine
as the Flutter UI, so the system services and the UI share one session and
one unlocked vault.

- [x] Flutter Android target. The Rust engine is cross-compiled for arm64,
      armv7 and x86_64 with the NDK through the build hook. CI builds the APK
      and AAB. Storage paths come from `MainActivity`.
- [x] Mobile Google sign-in: a mobile OAuth client (no secret), with PKCE and
      a redirect to its reversed client id, a private-use URI scheme
      (`com.googleusercontent.apps.<id>:/oauth2redirect`) that only KeyRoam
      registers. `OAuthRedirectActivity` hands the redirect to the engine,
      which checks `state` in constant time and redeems the code. Without a
      mobile client the app falls back to the loopback flow.
- [x] `CredentialProviderService` (Android 14+) for passwords, with an
      `AutofillService` (Android 8+) for apps and browsers. Apps are matched
      by package name (`androidapp://` and Google Password Manager's
      `android://…@package` URLs). A browser's reported web domain is
      believed only from known browsers and matched by registrable domain,
      as on the desktop. Any other app that claims a web domain sees only its
      own logins. While KeyRoam is locked, both services offer an unlock
      prompt; unlocking there locks again after 5 minutes unless the app is
      in use. Saves go through the system's save prompt.
- [x] WorkManager periodic sync (every 15 minutes, online) and a sync as
      soon as connectivity returns, through a WorkManager network constraint
      and, while the app is open, a `ConnectivityManager` callback. Sync
      needs the vault key, so it runs only while the vault is unlocked.
- [x] Biometric unlock: the KEK (the Argon2 output of the master password)
      is encrypted with an AES-GCM Android Keystore key that needs a strong
      biometric and is destroyed when biometrics change. A master-password
      change or vault-key rotation makes the stored KEK stop working, and
      biometric unlock turns itself off.
- [x] Tests: 9 Rust tests for app/browser targets, field classification and
      saving; 4 OAuth tests for the custom-scheme flow (including an
      end-to-end exchange against a local token endpoint); a core test for
      KEK unlock; 6 FFI tests; 7 widget tests of the Android flows.
- [ ] Verified on physical devices. So far: unit and widget tests, CI
      builds, and an emulator run (see the pull request).
- [ ] Play Store release (milestone 8).

## ✅ 7. iOS

`app/ios` (Swift), `crates/keyroam-autofill::ios` and the `ios_*` engine
methods. Swift opens the engine framework the build hook bundles, so the app's
Swift code shares the Flutter UI's engine and the AutoFill extension runs its
own over the shared vault folder.

- [x] Flutter iOS target. The Rust engine is built for device and simulator.
      CI builds an unsigned app with the extension. The Files app can reach
      backups.
- [x] Mobile Google sign-in in an `ASWebAuthenticationSession` (private
      session) with PKCE and the reversed-client-id redirect of an iOS
      OAuth client; the engine checks `state` and redeems the code.
- [x] AutoFill Credential Provider extension. The vault lives in an App Group
      container (an existing vault is moved there once). Each request
      unlocks with Face ID or the master password, lists the logins of the
      requesting site or app (matched by registrable domain from iOS's
      service identifiers) and releases only the chosen one. QuickType
      suggestions come from credential identities (entry id, username,
      host; never passwords) refreshed whenever the vault changes.
- [x] Face ID / Touch ID unlock: the KEK in a keychain item limited to the
      current biometrics, shared with the extension. A password change or
      key rotation turns it off.
- [x] `BGAppRefreshTask` sync while the vault is unlocked; sync as soon as the
      network returns (`NWPathMonitor`); lock when the device locks.
- [x] Tests: 4 Rust tests for service identifiers, matching and identities;
      an FFI test of the extension as a second engine over the same folder;
      3 widget tests of the iOS flows.
- [ ] Verified on physical devices (needs a signing team with the App Group
      and AutoFill capabilities). So far: unit and widget tests and CI builds.
- [ ] App Store release (milestone 8).

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
