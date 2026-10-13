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
- **Auto-lock** after 5 min of inactivity.
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

Not yet implemented on mobile: Google sign-in with the mobile OAuth client
types, system autofill providers, and OS-scheduled background sync. These are
milestones 6 and 7 in [../ROADMAP.md](../ROADMAP.md).
