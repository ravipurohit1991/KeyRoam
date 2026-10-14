import 'dart:io';

import 'package:flutter/services.dart';

/// Native open and save dialogs for backups and CSV imports, served by the
/// Windows and macOS runners on the `keyroam/files` channel. On the macOS
/// App Sandbox they are also what gives the app access to a file outside its
/// container. Mobile platforms keep the path field (Files app, USB folder).
class FilePicker {
  const FilePicker();

  static const _channel = MethodChannel('keyroam/files');

  bool get supported => Platform.isWindows || Platform.isMacOS;

  /// Ask where to save; starts at [suggestedPath] (folder and file name).
  Future<String?> save({
    required String title,
    String? suggestedPath,
    required String typeLabel,
    required List<String> extensions,
  }) {
    final path = suggestedPath?.trim() ?? '';
    final cut = path.lastIndexOf(Platform.pathSeparator);
    return _channel.invokeMethod<String>('save', {
      'title': title,
      'initialDirectory': cut > 0 ? path.substring(0, cut) : '',
      'suggestedName': cut >= 0 ? path.substring(cut + 1) : path,
      'typeLabel': typeLabel,
      'extensions': extensions,
    });
  }

  /// Ask for an existing file.
  Future<String?> open({required String title, required String typeLabel, required List<String> extensions}) =>
      _channel.invokeMethod<String>('open', {'title': title, 'typeLabel': typeLabel, 'extensions': extensions});
}
