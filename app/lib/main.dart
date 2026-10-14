import 'dart:io';

import 'package:flutter/material.dart';

import 'src/app/controller.dart';
import 'src/app/mobile_platform.dart';
import 'src/app/platform_paths.dart';
import 'src/engine/api.dart';
import 'src/ui/app.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final mobile = Platform.isAndroid || Platform.isIOS ? NativeMobilePlatform() : null;
  final controller = AppController(NativeKeyRoamApi(), mobile: mobile);
  PlatformPaths.onSystemLock(controller.lockForSystem);
  runApp(KeyRoamApp(controller: controller));
  // Data directory: KEYROAM_HOME if set, otherwise the platform default
  // (reported by the host app on Android, and on iOS the App Group folder
  // that the AutoFill extension shares).
  final paths = await PlatformPaths.load();
  await controller.init(dataDir: paths.data, backupDir: paths.backups);
}
