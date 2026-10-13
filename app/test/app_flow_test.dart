import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keyroam/src/app/controller.dart';
import 'package:keyroam/src/engine/models.dart';
import 'package:keyroam/src/ui/app.dart';

import 'support/fake_api.dart';

const pw = 'synthetic-master-pw-1';

const fastTimings = AppTimings(
  periodicSync: Duration(minutes: 10),
  offlineRetry: Duration(seconds: 3),
  syncDebounce: Duration(milliseconds: 200),
  autoLock: Duration(seconds: 30),
  clipboardClear: Duration(seconds: 1),
);

Future<AppController> start(WidgetTester tester, FakeKeyRoamApi api) async {
  tester.view.physicalSize = const Size(1280, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final c = AppController(api, timings: fastTimings);
  await c.init();
  await tester.pumpWidget(KeyRoamApp(controller: c));
  await tester.pumpAndSettle();
  return c;
}

Future<void> finish(WidgetTester tester, AppController c) async {
  c.dispose();
  await tester.pumpWidget(const SizedBox());
}

void main() {
  testWidgets('new user: password, then Google, then vault created and synced', (tester) async {
    final api = FakeKeyRoamApi();
    final c = await start(tester, api);
    expect(find.text('Welcome to KeyRoam'), findsOneWidget);

    await tester.tap(find.text("I'm new to KeyRoam"));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('masterPassword')).first, pw);
    await tester.enterText(find.byKey(const Key('confirmPassword')).first, 'different-password');
    await tester.pump();
    expect(find.text('Passwords do not match'), findsOneWidget);
    expect(tester.widget<FilledButton>(find.byKey(const Key('continue'))).onPressed, isNull);

    await tester.enterText(find.byKey(const Key('confirmPassword')).first, pw);
    await tester.pump();
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pumpAndSettle();
    expect(find.text('Sign in with Google'), findsWidgets);
    expect(api.calls, isEmpty, reason: 'nothing happens before Google sign-in');

    await tester.tap(find.byKey(const Key('googleSignIn')));
    await tester.pumpAndSettle();
    expect(api.calls, ['beginSignIn', 'completeSignIn', 'createVault']);
    expect(api.password, pw);
    expect(find.byKey(const Key('addEntry')), findsOneWidget);
    expect(find.text('Synced'), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('existing user: Google, download, then master password', (tester) async {
    final api = FakeKeyRoamApi(remoteVaultExists: true);
    final c = await start(tester, api);
    await tester.tap(find.text('I already use KeyRoam on another device'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('googleSignIn')));
    await tester.pumpAndSettle();
    expect(api.calls, ['beginSignIn', 'completeSignIn', 'fetchRemoteVault']);
    expect(find.text('Enter your master password'), findsOneWidget);

    await tester.enterText(find.byKey(const Key('joinPassword')).first, 'wrong-password-xyz');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(find.text('Incorrect master password.'), findsOneWidget);

    await tester.enterText(find.byKey(const Key('joinPassword')).first, pw);
    await tester.tap(find.text('Unlock'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('addEntry')), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('new user whose Google account already has a vault is redirected', (tester) async {
    final api = FakeKeyRoamApi(remoteVaultExists: true);
    final c = await start(tester, api);
    await tester.tap(find.text("I'm new to KeyRoam"));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('masterPassword')).first, 'some-other-password');
    await tester.enterText(find.byKey(const Key('confirmPassword')).first, 'some-other-password');
    await tester.pump();
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('googleSignIn')));
    await tester.pumpAndSettle();
    expect(find.text('Use my existing vault'), findsOneWidget);
    await tester.tap(find.text('Use my existing vault'));
    await tester.pumpAndSettle();
    expect(find.text('Enter your master password'), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('unlock syncs on open; add, reveal, edit, delete; sync after edits', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)..signedIn = true;
    final c = await start(tester, api);
    expect(find.text('Your vault is locked'), findsOneWidget);
    await tester.enterText(find.byKey(const Key('unlockPassword')).first, 'wrong-password-xyz');
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    expect(find.text('Incorrect master password.'), findsOneWidget);

    await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    expect(api.syncCount, 1, reason: 'sync when the vault is opened');

    // Add (the editor pre-fills a generated password).
    await tester.tap(find.byKey(const Key('addEntry')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('entryTitle')), 'Example Mail');
    await tester.enterText(find.byKey(const Key('entryUsername')), 'alice@example.test');
    await tester.tap(find.byKey(const Key('saveEntry')));
    await tester.pumpAndSettle();
    expect(find.text('Example Mail'), findsWidgets);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    expect(api.syncCount, 2, reason: 'sync after a local modification');

    // Detail view: password hidden until revealed.
    await tester.tap(find.text('Example Mail').first);
    await tester.pumpAndSettle();
    expect(find.text('G' * 20), findsNothing);
    await tester.tap(find.byKey(const Key('revealPassword')));
    await tester.pumpAndSettle();
    expect(find.text('G' * 20), findsOneWidget);

    // Edit.
    await tester.tap(find.byTooltip('Edit'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('entryTitle')), 'Example Mail (work)');
    await tester.tap(find.byKey(const Key('saveEntry')));
    await tester.pumpAndSettle();
    expect(find.text('Example Mail (work)'), findsWidgets);
    expect(find.textContaining('Password history (1)'), findsOneWidget);

    // Delete.
    await tester.tap(find.byTooltip('Delete'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Delete'));
    await tester.pumpAndSettle();
    expect(find.text('No logins yet'), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('conflicts are shown and resolved by choosing a version', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)..signedIn = true;
    api.seed(
      const EntryData(title: 'Shared', password: 'from-a'),
      conflictWith: const EntryData(title: 'Shared', password: 'from-b'),
    );
    final c = await start(tester, api);
    await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    expect(find.textContaining('edited on two devices'), findsOneWidget);
    expect(find.byTooltip('Edited on two devices'), findsOneWidget);

    await tester.tap(find.text('Shared'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('conflictVersion0')), findsOneWidget);
    expect(find.byKey(const Key('conflictVersion1')), findsOneWidget);
    await tester.tap(find.byKey(const Key('keepVersion1')));
    await tester.pumpAndSettle();
    expect(api.calls, contains('resolveConflict:1'));
    expect(find.byKey(const Key('conflictVersion0')), findsNothing);
    expect(find.textContaining('edited on two devices'), findsNothing);
    await finish(tester, c);
  });

  testWidgets('offline status, retry when connectivity returns, Sync Now', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)
      ..signedIn = true
      ..offline = true;
    final c = await start(tester, api);
    await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    expect(find.text('Offline'), findsOneWidget);
    expect(api.syncCount, 1);

    api.offline = false;
    await tester.pump(fastTimings.offlineRetry);
    await tester.pumpAndSettle();
    expect(api.syncCount, 2);
    expect(find.text('Synced'), findsOneWidget);

    await tester.tap(find.byKey(const Key('syncNow')));
    await tester.pumpAndSettle();
    expect(api.syncCount, 3);
    expect(find.text('Synchronised'), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('auto-locks after inactivity', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final c = await start(tester, api);
    await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('addEntry')), findsOneWidget);
    await tester.pump(fastTimings.autoLock + const Duration(seconds: 1));
    await tester.pumpAndSettle();
    expect(find.text('Your vault is locked'), findsOneWidget);
    expect(api.unlocked, isFalse);
    await finish(tester, c);
  });

  testWidgets('export backup dialog requires matching passwords', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final c = await start(tester, api);
    await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
    await tester.tap(find.byKey(const Key('unlock')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('menu')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Export encrypted backup…'));
    await tester.pumpAndSettle();
    final fields = find.descendant(of: find.byType(AlertDialog), matching: find.byType(TextField));
    await tester.enterText(fields.at(1), 'backup-synthetic-pw');
    await tester.enterText(fields.at(2), 'mismatch-synthetic');
    await tester.tap(find.text('Export'));
    await tester.pumpAndSettle();
    expect(find.text('Passwords do not match'), findsOneWidget);
    await tester.enterText(fields.at(2), 'backup-synthetic-pw');
    await tester.tap(find.text('Export'));
    await tester.pumpAndSettle();
    expect(api.calls, contains('export:backup.krbackup'));
    expect(find.textContaining('Exported 0 entries'), findsOneWidget);
    await finish(tester, c);
  });
}
