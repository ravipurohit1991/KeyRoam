// Build hook: compiles the Rust engine (crates/keyroam-ffi) with cargo and
// bundles it with the app as a native code asset. Runs automatically for
// `flutter run`, `flutter build` and `flutter test` on every platform:
// Windows, macOS, Linux (tests only), Android and iOS.
//
// Release builds can embed the Google OAuth client through pubspec.yaml
// user-defines:
//
//   hooks:
//     user_defines:
//       keyroam:
//         google_client_id: "1234-abc.apps.googleusercontent.com"
//         google_client_secret: "GOCSPX-..."

import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

const _crate = 'keyroam-ffi';
const _libBase = 'keyroam_ffi';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    final code = input.config.code;
    final target = _rustTarget(code);
    final workspace = Directory.fromUri(input.packageRoot.resolve('../'));
    final targetDir = input.outputDirectoryShared.resolve('cargo/');

    final env = <String, String>{..._platformEnv(code, target)};
    for (final key in ['google_client_id', 'google_client_secret']) {
      final value = input.userDefines[key];
      if (value is String && value.isNotEmpty) {
        env['KEYROAM_${key.toUpperCase()}'] = value;
      }
    }

    final result = await Process.run(
      _findCargo(),
      ['build', '--release', '--locked', '-p', _crate, '--target', target, '--target-dir', targetDir.toFilePath()],
      workingDirectory: workspace.path,
      environment: env,
    );
    if (result.exitCode != 0) {
      throw Exception(
        'cargo build for $target failed (exit ${result.exitCode}).\n'
        'If the target is missing, run: rustup target add $target\n'
        '${result.stderr}',
      );
    }

    final lib = targetDir.resolve('$target/release/${_libFileName(code.targetOS)}');
    output.assets.code.add(
      CodeAsset(package: input.packageName, name: _libBase, linkMode: DynamicLoadingBundled(), file: lib),
    );

    // Rebuild when any Rust source or manifest changes.
    output.dependencies.addAll([
      workspace.uri.resolve('Cargo.toml'),
      workspace.uri.resolve('Cargo.lock'),
      ...Directory.fromUri(workspace.uri.resolve('crates/'))
          .listSync(recursive: true)
          .whereType<File>()
          .where((f) => f.path.endsWith('.rs') || f.path.endsWith('Cargo.toml'))
          .map((f) => f.uri),
    ]);
  });
}

/// Toolchain environment for cross-compiling to [target].
Map<String, String> _platformEnv(CodeConfig code, String target) {
  switch (code.targetOS) {
    case OS.macOS:
      return {'MACOSX_DEPLOYMENT_TARGET': '${code.macOS.targetVersion}', ..._appleSdk('macosx')};
    case OS.iOS:
      final sdk = code.iOS.targetSdk == IOSSdk.iPhoneSimulator ? 'iphonesimulator' : 'iphoneos';
      return {'IPHONEOS_DEPLOYMENT_TARGET': '${code.iOS.targetVersion}', ..._appleSdk(sdk)};
    case OS.android:
      // Link with the NDK clang provided by Flutter; `ring` (TLS) also needs
      // it to compile its C/assembly parts.
      final cc = code.cCompiler;
      if (cc == null) {
        throw StateError('No Android NDK toolchain was provided. Install the NDK via Android Studio.');
      }
      final clangTarget =
          '${target == 'armv7-linux-androideabi' ? 'armv7a-linux-androideabi' : target}'
          '${code.android.targetNdkApi}';
      final envTarget = target.replaceAll('-', '_');
      final compiler = cc.compiler.toFilePath();
      // 16 KB page alignment, required for 64-bit apps on newer Android versions.
      final pageSize = target.startsWith('aarch64') || target.startsWith('x86_64')
          ? ' -C link-arg=-Wl,-z,max-page-size=16384'
          : '';
      return {
        'CARGO_TARGET_${envTarget.toUpperCase()}_LINKER': compiler,
        'CARGO_TARGET_${envTarget.toUpperCase()}_RUSTFLAGS': '-C link-arg=--target=$clangTarget$pageSize',
        'CC_$envTarget': compiler,
        'CFLAGS_$envTarget': '--target=$clangTarget',
        'AR_$envTarget': cc.archiver.toFilePath(),
      };
    default:
      return {};
  }
}

/// The hook runner does not pass Xcode's environment through, so C code
/// (compiled by `ring`) would not find the SDK headers. Locate the SDK
/// explicitly.
Map<String, String> _appleSdk(String sdk) {
  final r = Process.runSync('xcrun', ['--sdk', sdk, '--show-sdk-path']);
  if (r.exitCode != 0) {
    throw StateError('xcrun could not find the $sdk SDK: ${r.stderr}');
  }
  return {'SDKROOT': (r.stdout as String).trim()};
}

String _findCargo() {
  final home = Platform.environment['USERPROFILE'] ?? Platform.environment['HOME'];
  final exe = Platform.isWindows ? 'cargo.exe' : 'cargo';
  if (home != null) {
    final candidate = File(
      '$home${Platform.pathSeparator}.cargo${Platform.pathSeparator}bin'
      '${Platform.pathSeparator}$exe',
    );
    if (candidate.existsSync()) return candidate.path;
  }
  return 'cargo';
}

String _libFileName(OS os) => switch (os) {
  OS.windows => '$_libBase.dll',
  OS.macOS || OS.iOS => 'lib$_libBase.dylib',
  _ => 'lib$_libBase.so',
};

String _rustTarget(CodeConfig code) {
  final os = code.targetOS;
  final arch = code.targetArchitecture;
  final simulator = os == OS.iOS && code.iOS.targetSdk == IOSSdk.iPhoneSimulator;
  final key = '$os-$arch${simulator ? '-sim' : ''}';
  const targets = {
    'windows-x64': 'x86_64-pc-windows-msvc',
    'windows-arm64': 'aarch64-pc-windows-msvc',
    'macos-x64': 'x86_64-apple-darwin',
    'macos-arm64': 'aarch64-apple-darwin',
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
    'android-arm64': 'aarch64-linux-android',
    'android-arm': 'armv7-linux-androideabi',
    'android-x64': 'x86_64-linux-android',
    'android-ia32': 'i686-linux-android',
    'ios-arm64': 'aarch64-apple-ios',
    'ios-arm64-sim': 'aarch64-apple-ios-sim',
    'ios-x64-sim': 'x86_64-apple-ios',
  };
  final t = targets[key];
  if (t == null) {
    throw UnsupportedError('KeyRoam does not support $key yet');
  }
  return t;
}
