import 'package:flutter/material.dart';

import 'src/app/controller.dart';
import 'src/engine/api.dart';
import 'src/ui/app.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final controller = AppController(NativeKeyRoamApi());
  runApp(KeyRoamApp(controller: controller));
  // Data directory: KEYROAM_HOME if set, otherwise the platform default.
  await controller.init();
}
