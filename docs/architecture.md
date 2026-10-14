# KeyRoam architecture

```
┌──────────────────────────── Flutter UI (Windows, macOS, Android, iOS) ───────────────────────────┐
│ onboarding · dashboard · entry editor · conflicts · Sync Now + status · backup export/restore   │
└───────────────▲──────────────────────────────────────────────────────────────▲──────────────────┘
                │ keyroam-ffi: C ABI, JSON req/resp, background isolates      │ platform channels
┌───────────────┴────────────────────────────┐   ┌───────────────────────────────┴─────────────────┐
│ keyroam-sync                               │   │ Platform integrations (native code)             │
│  oauth   PKCE, loopback redirect, refresh  │   │  Desktop: native-messaging host ◄─► browser ext │
│  drive   Drive v3 appDataFolder REST       │   │  Android: CredentialProviderService             │
│  engine  sync protocol, onboarding         │   │  iOS: ASCredentialProviderViewController ext    │
└───────────────▲────────────────────────────┘   └─────────────────────────────────────────────────┘
┌───────────────┴────────────────────────────┐
│ keyroam-core (no network, no unsafe)       │
│  kdf · crypto · header · envelope · model  │
│  vault · files · storage · generator       │
└────────────────────────────────────────────┘
```

## Principles

- **One engine.** All security-critical logic (cryptography, the vault format,
  merging, the sync protocol) lives in Rust and is shared by every platform.
  The UI layer never handles keys or ciphertext formats.
- **One provider.** Google Drive's `appDataFolder` is the only sync target.
  `RemoteStore` is a small internal seam that keeps the engine testable. It is
  not a provider-plugin system, and no other providers are planned.
- **No server.** Devices call Google APIs directly.

## Crates

### `keyroam-core`

| Module | Responsibility |
|---|---|
| `kdf` | Argon2id parameters, validation policy and key derivation |
| `crypto` | XChaCha20-Poly1305, HKDF, HMAC, length-hiding padding |
| `header` | Vault-key wrapping, header MAC, master-password change |
| `envelope` | Purpose-, vault- and device-bound encrypted JSON payloads |
| `model` | Entries, version vectors, CRDT merge, conflicts and history |
| `vault` | The unlocked `Vault` API used by every front-end |
| `files` | Local vault, replica, backup and secrets file formats |
| `storage` | Atomic local persistence and rotating backups |
| `generator` | CSPRNG password generator |
| `import` | CSV import from browsers and other password managers |

### `keyroam-sync`

| Module | Responsibility |
|---|---|
| `oauth` | PKCE, authorisation URL, loopback receiver, token exchange, refresh and revoke, `GoogleSession` |
| `drive` | Drive v3 client: list, download, multipart create, media update. Retries and token refresh |
| `engine` | Sync rounds, sync status, `create_vault` and `join_vault` onboarding |

### `keyroam-autofill`

| Module | Responsibility |
|---|---|
| `origin` | Web origins, registrable domains (Public Suffix List), page matching |
| `matching` | Logins for a page, save/update plans |
| `protocol` | The extension protocol, answered by the app |
| `bridge` | Authenticated loopback bridge between the host and the app |
| `framing` | Length-prefixed messages (native messaging and the bridge) |
| `install` | Registering the host with Chrome, Chromium, Edge, Brave, Vivaldi and Firefox |

### `keyroam-native-host`

The executable the browser starts for the extension. It relays each message
to the running app over the bridge and holds no secrets. `install` and
`uninstall` register it for the current user.

### `browser-extension/`

Manifest V3 extension (Chromium service worker, Firefox background scripts).
It detects login forms, fills a login the user picks, and offers to save
logins after sign-in. See `browser-extension/README.md`.

### `keyroam-ffi`

A `cdylib` that exports two functions: `keyroam_call(json) -> json` and
`keyroam_free_string`. The latter wipes each response before freeing it. A
process-wide `Engine` holds the open vault. Long Google sign-in waits run
without holding the session lock, so they can be cancelled. The Flutter app
builds and bundles this library through `app/hook/build.dart`.

### Android (`app/android`)

Kotlin code in the app module calls the engine through a JNI entry point in
`keyroam-ffi` (`android.rs`). It is the same native library and the same
process-wide `Engine` the Flutter UI uses through Dart FFI, so the system
services and the UI share one session. The services are the autofill service,
the Credential Manager provider (Android 14+), the unlock activity for both,
biometric unlock, WorkManager sync and the sign-in redirect activity. The
`android_*` engine methods answer them, with the matching and field rules of
`keyroam_autofill::android`. When the vault changes outside the UI, Kotlin
tells the UI over the `keyroam/platform` channel, and the UI refreshes.

### `keyroam-cli`

The reference front-end and the runnable deliverable for milestones 1–3. Every
operation the GUI apps will offer is available here first.

## Planned components

| Milestone | Component | Notes |
|---|---|---|
| 7 | iOS `ASCredentialProviderViewController` extension | A shared App Group container for the encrypted vault, unlocked with the master password or Face ID. The key is wrapped in a Keychain item limited to biometry. Sync through BGAppRefreshTask. |
| 8 | Cross-device test matrix, external security review, signed installers (MSIX, notarised DMG, Play, App Store) | |
