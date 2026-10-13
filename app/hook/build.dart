// Build hook: compiles the Rust engine (crates/keyroam-ffi) with cargo and
// bundles it with the app as a native code asset. Runs automatically for
// `flutter run`, `flutter build` and `flutter test`.
//
// Release builds can embed the Google OAuth desktop client through
// pubspec.yaml user-defines:
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
    final triple = _rustTarget(code.targetOS, code.targetArchitecture);
    final workspace = Directory.fromUri(input.packageRoot.resolve('../'));
    final targetDir = input.outputDirectoryShared.resolve('cargo/');

    final env = <String, String>{};
    for (final key in ['google_client_id', 'google_client_secret']) {
      final value = input.userDefines[key];
      if (value is String && value.isNotEmpty) {
        env['KEYROAM_${key.toUpperCase()}'] = value;
      }
    }
    if (code.targetOS == OS.macOS) {
      env['MACOSX_DEPLOYMENT_TARGET'] = '${code.macOS.targetVersion}';
    }

    final cargo = _findCargo();
    final result = await Process.run(
      cargo,
      ['build', '--release', '--locked', '-p', _crate, '--target', triple, '--target-dir', targetDir.toFilePath()],
      workingDirectory: workspace.path,
      environment: env,
    );
    if (result.exitCode != 0) {
      throw Exception(
        'cargo build for $triple failed (exit ${result.exitCode}).\n'
        'If the target is missing, run: rustup target add $triple\n'
        '${result.stderr}',
      );
    }

    final lib = targetDir.resolve('$triple/release/${_libFileName(code.targetOS)}');
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

String _rustTarget(OS os, Architecture arch) {
  final key = '$os-$arch';
  const targets = {
    'windows-x64': 'x86_64-pc-windows-msvc',
    'windows-arm64': 'aarch64-pc-windows-msvc',
    'macos-x64': 'x86_64-apple-darwin',
    'macos-arm64': 'aarch64-apple-darwin',
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
  };
  final t = targets[key];
  if (t == null) {
    throw UnsupportedError('KeyRoam does not support $key yet');
  }
  return t;
}
