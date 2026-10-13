import 'dart:io';

import 'package:flutter/services.dart';

/// Storage locations that the Rust engine cannot discover by itself.
///
/// Desktop and iOS use the engine defaults (null). On Android, `MainActivity`
/// reports the app's private data folder and an app-specific folder that is
/// reachable over USB, for backups.
class PlatformPaths {
  const PlatformPaths({this.data, this.backups});

  final String? data;
  final String? backups;

  static const _channel = MethodChannel('keyroam/platform');

  /// Open [url] in the browser on Android, where the engine cannot. Returns
  /// false on other platforms or on failure.
  static Future<bool> openUrl(String url) async {
    if (!Platform.isAndroid) return false;
    return await _channel.invokeMethod<bool>('openUrl', {'url': url}) ?? false;
  }

  static Future<PlatformPaths> load() async {
    if (!Platform.isAndroid) return const PlatformPaths();
    final m = await _channel.invokeMapMethod<String, String>('paths');
    return PlatformPaths(data: m?['data'], backups: m?['backups']);
  }
}
