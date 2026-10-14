// Widget tests of the native file dialogs (Windows and macOS): the Browse
// buttons of the backup and CSV dialogs fill in the picked path. The dialogs
// themselves run in the runners; here a fake answers.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keyroam/src/app/controller.dart';
import 'package:keyroam/src/app/file_picker.dart';
import 'package:keyroam/src/ui/app.dart';

import 'support/fake_api.dart';

const pw = 'synthetic-master-pw-1';

class FakeFilePicker extends FilePicker {
  FakeFilePicker({this.supported = true});

  @override
  final bool supported;
  final requests = <String>[];
  String? answer;

  @override
  Future<String?> save({
    required String title,
    String? suggestedPath,
    required String typeLabel,
    required List<String> extensions,
  }) async {
    requests.add('save:$suggestedPath:${extensions.join(',')}');
    return answer;
  }

  @override
  Future<String?> open({required String title, required String typeLabel, required List<String> extensions}) async {
    requests.add('open:${extensions.join(',')}');
    return answer;
  }
}

Future<AppController> start(WidgetTester tester, FakeKeyRoamApi api, FilePicker files) async {
  tester.view.physicalSize = const Size(1280, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final c = AppController(api, files: files);
  await c.init();
  await tester.pumpWidget(KeyRoamApp(controller: c));
  await tester.pumpAndSettle();
  await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
  await tester.tap(find.byKey(const Key('unlock')));
  await tester.pumpAndSettle();
  return c;
}

Future<void> finish(WidgetTester tester, AppController c) async {
  c.dispose();
  await tester.pumpWidget(const SizedBox());
}

Future<void> openMenu(WidgetTester tester, String item) async {
  await tester.tap(find.byKey(const Key('menu')));
  await tester.pumpAndSettle();
  await tester.tap(find.text(item));
  await tester.pumpAndSettle();
}

String pathText(WidgetTester tester, String key) => tester.widget<TextField>(find.byKey(Key(key))).controller!.text;

void main() {
  testWidgets('export: Browse picks where to save; cancelling keeps the path', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final files = FakeFilePicker();
    final c = await start(tester, api, files);
    await openMenu(tester, 'Export encrypted backup…');
    final suggested = pathText(tester, 'actionPath');

    await tester.tap(find.byKey(const Key('browse')));
    await tester.pumpAndSettle();
    expect(files.requests, ['save:$suggested:krbackup']);
    expect(pathText(tester, 'actionPath'), suggested, reason: 'cancelled');

    files.answer = '/Volumes/USB/vault.krbackup';
    await tester.tap(find.byKey(const Key('browse')));
    await tester.pumpAndSettle();
    expect(pathText(tester, 'actionPath'), '/Volumes/USB/vault.krbackup');

    final fields = find.descendant(of: find.byType(AlertDialog), matching: find.byType(TextField));
    await tester.enterText(fields.at(1), 'backup-synthetic-pw');
    await tester.enterText(fields.at(2), 'backup-synthetic-pw');
    await tester.tap(find.text('Export'));
    await tester.pumpAndSettle();
    expect(api.calls, contains('export:/Volumes/USB/vault.krbackup'));
    await finish(tester, c);
  });

  testWidgets('restore and CSV import: Browse picks an existing file', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final files = FakeFilePicker()..answer = r'C:\Users\me\Downloads\vault.krbackup';
    final c = await start(tester, api, files);
    await openMenu(tester, 'Restore from backup…');
    await tester.tap(find.byKey(const Key('browse')));
    await tester.pumpAndSettle();
    expect(files.requests.last, 'open:krbackup,krv');
    expect(pathText(tester, 'actionPath'), r'C:\Users\me\Downloads\vault.krbackup');
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();

    files.answer = r'C:\Users\me\Downloads\passwords.csv';
    await openMenu(tester, 'Import passwords from CSV…');
    await tester.tap(find.byKey(const Key('browse')));
    await tester.pumpAndSettle();
    expect(files.requests.last, 'open:csv');
    expect(pathText(tester, 'csvPath'), r'C:\Users\me\Downloads\passwords.csv');
    await tester.tap(find.byKey(const Key('csvImport')));
    await tester.pumpAndSettle();
    expect(api.calls, contains(r'importCsv:C:\Users\me\Downloads\passwords.csv:true'));
    await finish(tester, c);
  });

  testWidgets('no Browse button where there are no native dialogs', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final c = await start(tester, api, FakeFilePicker(supported: false));
    await openMenu(tester, 'Export encrypted backup…');
    expect(find.byKey(const Key('browse')), findsNothing);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    await finish(tester, c);
  });
}
