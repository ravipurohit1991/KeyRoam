# KeyRoam desktop app (Flutter)

The Windows and macOS app. All cryptography, storage and sync run in the Rust
engine (`../crates`). This Flutter project contains only the UI.

## How the engine is linked

`hook/build.dart` is a Dart build hook. It runs automatically during
`flutter run`, `flutter build` and `flutter test`. It:

1. runs `cargo build --release -p keyroam-ffi` for the target platform, then
2. bundles the resulting `keyroam_ffi.dll` / `libkeyroam_ffi.dylib` as the code
   asset `package:keyroam/keyroam_ffi`.

`lib/src/engine/native.dart` binds the two C functions of that library. Every
call runs on a background isolate (`lib/src/engine/api.dart`), so Argon2 and
network work never freeze the UI.

## Requirements

- Flutter 3.47+ (stable) and a Rust toolchain (`rustup`).
- Windows: Visual Studio (or Build Tools) with the "Desktop development with
  C++" workload.
- macOS: Xcode, plus both Rust targets for universal builds:
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.

## Run

```sh
cd app
flutter run -d windows        # or -d macos
```

Google sign-in needs an OAuth "Desktop app" client. See
[../docs/google-cloud-setup.md](../docs/google-cloud-setup.md).

- **Development:** set `KEYROAM_GOOGLE_CLIENT_ID` and
  `KEYROAM_GOOGLE_CLIENT_SECRET` in the environment that launches the app.
- **Release builds:** embed the client through `pubspec.yaml`:

  ```yaml
  hooks:
    user_defines:
      keyroam:
        google_client_id: "1234-abc.apps.googleusercontent.com"
        google_client_secret: "GOCSPX-..."
  ```

By default the vault is stored in the platform data directory. Set
`KEYROAM_HOME` to use a different folder, for example a throwaway profile
during testing.

## Test

```sh
flutter test
```

- `test/native_engine_test.dart` drives the **real** Rust engine through FFI,
  using the synthetic fixture `test/fixtures/vault.krv` (password
  `synthetic-master-pw-1`). To regenerate the fixture, run
  `cargo run -p keyroam-core --example make_fixture -- app/test/fixtures/vault.krv`.
- `test/app_flow_test.dart` contains widget tests of the UI flows: both
  onboarding paths, unlock, entry editing, conflicts, offline handling and
  retry, auto-lock, and backup export. They run against an in-memory fake API.

## Behaviour

- **Sync** runs when the vault is unlocked, about 2 s after each change, every
  5 min while unlocked, every 30 s while offline (so it catches connectivity
  returning), and on **Sync Now**. The status chip in the toolbar shows the
  current state.
- **Auto-lock** after 5 min of inactivity, and when Windows locks or suspends,
  or macOS locks the screen, sleeps or switches user (the runners report these
  on the `keyroam/system` channel).
- **Browser integration** (Menu → Browser integration…) is off by default.
  When on, the engine serves the browser extension through
  `keyroam-native-host`, which the release builds place next to
  `keyroam.exe` and in `KeyRoam.app/Contents/MacOS`. **Register with
  browsers** writes the native-messaging manifests; under the macOS sandbox
  that fails and the app shows the terminal command to run instead. Logins
  saved from the browser appear within a few seconds.
- **Import passwords from CSV…** reads exports of Chrome, Edge, Firefox,
  Safari, Bitwarden, LastPass, 1Password and KeePass, skips logins already in
  the vault, and can delete the plaintext CSV afterwards.
- **Recently deleted** items can be restored or deleted permanently.
- **Replace vault key…** creates a new vault key; other devices show a banner
  asking for the master password once.
- **Copied passwords** are cleared from the clipboard after 30 s, provided the
  clipboard still holds them.

## Mobile builds (Android, iOS)

The same app builds for Android and iOS. The build hook cross-compiles the
Rust engine for each target. You need these Rust targets:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android   # Android
rustup target add aarch64-apple-ios aarch64-apple-ios-sim                              # iOS
flutter build apk --release        # or: flutter build ios --release --no-codesign
```

Android builds need the Android NDK. Gradle installs the version Flutter
pins on the first build.

### Android integrations

The Kotlin code in `android/app/src/main/kotlin` reaches the same Rust engine
as the Flutter UI through JNI (`KeyRoamEngine`, the native library's
`nativeCall`):

| Component | What it does |
|---|---|
| `KeyRoamAutofillService` | Android autofill (8+): fills logins for apps and browser pages, offers to save new ones, and shows an unlock prompt while KeyRoam is locked |
| `KeyRoamCredentialService` | Credential Manager provider (14+): offers saved passwords in apps' sign-in sheets and saves new ones |
| `UnlockActivity` | The unlock dialog for those requests (master password or biometrics) |
| `Biometrics` | Keeps the vault's key-encryption key behind an Android Keystore key that needs a strong biometric |
| `SyncWorker` | WorkManager sync every 15 minutes and when connectivity returns, while the vault is unlocked |
| `OAuthRedirectActivity` | Receives the Google sign-in redirect (mobile OAuth client, PKCE) |

In the app, **Menu → Autofill…** opens the system settings to choose KeyRoam,
and **Menu → Biometric unlock…** turns biometric unlock on or off. The rules
that decide which logins an app or page may see live in Rust
(`crates/keyroam-autofill/src/android.rs`) and have tests. iOS integrations are
milestone 7 in [../ROADMAP.md](../ROADMAP.md).
