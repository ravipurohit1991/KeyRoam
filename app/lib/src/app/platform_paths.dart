import 'dart:io';

import 'package:flutter/services.dart';

/// Storage locations that the Rust engine cannot discover by itself.
///
/// Desktop platforms use the engine defaults (null). On Android,
/// `MainActivity` reports the app's private data folder and an app-specific
/// folder that is reachable over USB, for backups. On iOS, the plugin reports
/// the App Group folder the AutoFill extension can read, and Documents (shown
/// in the Files app) for backups.
class PlatformPaths {
  const PlatformPaths({this.data, this.backups});

  final String? data;
  final String? backups;

  static const _channel = MethodChannel('keyroam/platform');

  static bool get _mobile => Platform.isAndroid || Platform.isIOS;

  /// Open the sign-in page [url] where the engine cannot: in the browser on
  /// Android, in an `ASWebAuthenticationSession` on iOS. Returns false on
  /// other platforms or on failure.
  static Future<bool> openUrl(String url) async {
    if (!_mobile) return false;
    return await _channel.invokeMethod<bool>('openUrl', {'url': url}) ?? false;
  }

  /// Dismiss the iOS sign-in sheet once sign-in finished or failed.
  static Future<void> closeSignIn() async {
    if (Platform.isIOS) await _channel.invokeMethod<void>('closeSignIn');
  }

  /// Desktop platforms, where the browser extension can be used.
  static bool get isDesktop => Platform.isWindows || Platform.isMacOS || Platform.isLinux;

  /// The native-messaging helper shipped next to the app executable
  /// (`KeyRoam.app/Contents/MacOS` on macOS).
  static String browserHostPath() {
    final dir = File(Platform.resolvedExecutable).parent.path;
    final name = Platform.isWindows ? 'keyroam-native-host.exe' : 'keyroam-native-host';
    return '$dir${Platform.pathSeparator}$name';
  }

  /// Run [lock] when the operating system locks the screen, sleeps or
  /// switches user (reported by the Windows and macOS runners).
  static void onSystemLock(Future<void> Function() lock) {
    const MethodChannel('keyroam/system').setMethodCallHandler((call) async {
      if (call.method == 'lock') await lock();
    });
  }

  static Future<PlatformPaths> load() async {
    if (!_mobile) return const PlatformPaths();
    final m = await _channel.invokeMapMethod<String, String>('paths');
    return PlatformPaths(data: m?['data'], backups: m?['backups']);
  }
}
