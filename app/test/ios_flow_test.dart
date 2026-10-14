// Widget tests of the iOS flows: the AutoFill settings dialog, QuickType
// suggestions kept up to date, and locking when the device locks. They run
// against in-memory fakes of the engine and the platform.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keyroam/src/app/controller.dart';
import 'package:keyroam/src/engine/models.dart';
import 'package:keyroam/src/ui/app.dart';

import 'support/fake_api.dart';
import 'support/fake_mobile.dart';

const pw = 'synthetic-master-pw-1';

const timings = AppTimings(
  periodicSync: Duration(minutes: 10),
  offlineRetry: Duration(seconds: 3),
  syncDebounce: Duration(milliseconds: 200),
  autoLock: Duration(seconds: 30),
  clipboardClear: Duration(seconds: 1),
);

Future<AppController> start(WidgetTester tester, FakeKeyRoamApi api, FakeMobilePlatform mobile) async {
  tester.view.physicalSize = const Size(400, 860);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final c = AppController(api, timings: timings, mobile: mobile);
  await c.init();
  await tester.pumpWidget(KeyRoamApp(controller: c));
  await tester.pumpAndSettle();
  return c;
}

Future<void> finish(WidgetTester tester, AppController c) async {
  c.dispose();
  await tester.pumpWidget(const SizedBox());
}

Future<void> unlock(WidgetTester tester) async {
  await tester.enterText(find.byKey(const Key('unlockPassword')).first, pw);
  await tester.tap(find.byKey(const Key('unlock')));
  await tester.pumpAndSettle();
}

int count(FakeMobilePlatform m, String call) => m.calls.where((c) => c == call).length;

void main() {
  testWidgets('iOS AutoFill dialog points to the Settings app', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api)..ios = true;
    final c = await start(tester, api, mobile);
    await unlock(tester);
    await tester.tap(find.byKey(const Key('menu')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Autofill…'));
    await tester.pumpAndSettle();

    expect(find.text('In Settings, open General → AutoFill & Passwords and turn on KeyRoam'), findsOneWidget);
    expect(find.byKey(const Key('credentialProvider')), findsNothing, reason: 'Android 14+ only');
    await tester.tap(find.descendant(of: find.byKey(const Key('autofillService')), matching: find.text('Settings')));
    await tester.pumpAndSettle();
    expect(mobile.calls, contains('openAutofillSettings'));
    expect(find.text('KeyRoam is turned on in AutoFill & Passwords'), findsOneWidget);
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
    await finish(tester, c);
  });

  testWidgets('QuickType suggestions follow unlocks, edits and outside changes', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api)..ios = true;
    final c = await start(tester, api, mobile);
    expect(count(mobile, 'credentialsChanged'), 0, reason: 'nothing to suggest while locked');

    await unlock(tester);
    expect(count(mobile, 'credentialsChanged'), 1);

    await c.addEntry(const EntryData(title: 'Shop', username: 'carol', urls: ['https://shop.example.com']));
    await tester.pumpAndSettle();
    expect(count(mobile, 'credentialsChanged'), 2);

    // Searching does not change the logins.
    await c.search('shop');
    await tester.pumpAndSettle();
    expect(count(mobile, 'credentialsChanged'), 2);

    // A background sync brought in a login.
    api.saveFromBrowser(const EntryData(title: 'Synced', username: 'dave'));
    mobile.vaultChanged();
    await tester.pumpAndSettle();
    expect(count(mobile, 'credentialsChanged'), 3);
    await finish(tester, c);
  });

  testWidgets('locking the device locks the vault', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api)..ios = true;
    final c = await start(tester, api, mobile);
    await unlock(tester);
    expect(find.byKey(const Key('addEntry')), findsOneWidget);

    await c.lockForSystem();
    await tester.pumpAndSettle();
    expect(find.text('Your vault is locked'), findsOneWidget);
    expect(find.text('Locked because the device was locked'), findsWidgets);
    await finish(tester, c);
  });
}
