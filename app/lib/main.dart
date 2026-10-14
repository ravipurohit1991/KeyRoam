import 'dart:io';

import 'package:flutter/material.dart';

import 'src/app/controller.dart';
import 'src/app/platform_paths.dart';
import 'src/engine/api.dart';
import 'src/ui/app.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // The browser extension is served on desktop only.
  final desktop = Platform.isWindows || Platform.isMacOS || Platform.isLinux;
  final controller = AppController(NativeKeyRoamApi(), browserAutofill: desktop);
  runApp(KeyRoamApp(controller: controller));
  // Data directory: KEYROAM_HOME if set, otherwise the platform default
  // (reported by the host app on Android).
  final paths = await PlatformPaths.load();
  await controller.init(dataDir: paths.data, backupDir: paths.backups);
}
