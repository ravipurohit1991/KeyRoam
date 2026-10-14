// Widget tests of the Android-only flows: biometric unlock, background sync
// scheduling, autofill settings and vault changes made by the autofill
// service. They run against in-memory fakes of the engine and the platform.

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

Future<void> openMenu(WidgetTester tester, String item) async {
  await tester.tap(find.byKey(const Key('menu')));
  await tester.pumpAndSettle();
  await tester.tap(find.text(item));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('turn on biometric unlock, then unlock with it', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)..signedIn = true;
    final mobile = FakeMobilePlatform(api);
    final c = await start(tester, api, mobile);
    expect(find.byKey(const Key('biometricUnlock')), findsNothing);
    await unlock(tester);
    expect(mobile.calls, contains('scheduleSync'));

    await openMenu(tester, 'Biometric unlock…');
    final field = find.descendant(of: find.byType(AlertDialog), matching: find.byType(TextField));
    await tester.enterText(field, 'not-the-password');
    await tester.tap(find.text('Turn on'));
    await tester.pumpAndSettle();
    expect(mobile.enabled, isFalse);
    expect(find.byType(AlertDialog), findsOneWidget, reason: 'a wrong password keeps the dialog open');
    await tester.enterText(field, pw);
    await tester.tap(find.text('Turn on'));
    await tester.pumpAndSettle();
    expect(mobile.enabled, isTrue);
    expect(find.text('Biometric unlock is on'), findsWidgets);

    await c.lock();
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('biometricUnlock')));
    await tester.pumpAndSettle();
    expect(mobile.calls, contains('biometricUnlock'));
    expect(find.byKey(const Key('addEntry')), findsOneWidget);

    // Turning it off again.
    await openMenu(tester, 'Biometric unlock…');
    await tester.tap(find.text('Turn off'));
    await tester.pumpAndSettle();
    expect(mobile.enabled, isFalse);
    await finish(tester, c);
  });

  testWidgets('a stale biometric key falls back to the master password', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api)..enabled = true;
    final c = await start(tester, api, mobile);
    // The platform turned it off (e.g. the master password changed).
    mobile.enabled = false;
    await tester.tap(find.byKey(const Key('biometricUnlock')));
    await tester.pumpAndSettle();
    expect(find.text('Biometric unlock is off.'), findsOneWidget);
    expect(find.byKey(const Key('biometricUnlock')), findsNothing);
    await unlock(tester);
    expect(find.byKey(const Key('addEntry')), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('autofill changes reach the UI: unlock, saved login, lock', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api);
    final c = await start(tester, api, mobile);
    expect(find.text('Your vault is locked'), findsOneWidget);

    // The autofill service's unlock prompt opened the vault.
    await api.unlock(pw);
    mobile.vaultChanged();
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('addEntry')), findsOneWidget);

    // A login saved through autofill shows up.
    api.saveFromBrowser(const EntryData(title: 'Example app', username: 'bob'));
    mobile.vaultChanged();
    await tester.pumpAndSettle();
    expect(find.text('Example app'), findsOneWidget);

    // The autofill unlock timed out and locked the vault.
    await api.lock();
    mobile.vaultChanged();
    await tester.pumpAndSettle();
    expect(find.text('Your vault is locked'), findsOneWidget);
    await finish(tester, c);
  });

  testWidgets('the UI opens unlocked when autofill unlocked first', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)..signedIn = true;
    await api.unlock(pw);
    final mobile = FakeMobilePlatform(api);
    final c = await start(tester, api, mobile);
    expect(find.byKey(const Key('addEntry')), findsOneWidget);
    expect(mobile.calls, contains('scheduleSync'));
    await finish(tester, c);
  });

  testWidgets('offline sync waits for connectivity', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true)..signedIn = true;
    final mobile = FakeMobilePlatform(api);
    final c = await start(tester, api, mobile);
    api.offline = true;
    await unlock(tester);
    expect(mobile.calls, contains('syncWhenOnline'));
    api.offline = false;
    final before = api.syncCount;
    mobile.online();
    await tester.pumpAndSettle();
    expect(api.syncCount, before + 1);
    await finish(tester, c);
  });

  testWidgets('autofill settings dialog', (tester) async {
    final api = FakeKeyRoamApi(hasVault: true);
    final mobile = FakeMobilePlatform(api);
    final c = await start(tester, api, mobile);
    await unlock(tester);
    await openMenu(tester, 'Autofill…');
    expect(find.text('Choose KeyRoam as your autofill service'), findsOneWidget);
    await tester.tap(find.descendant(of: find.byKey(const Key('autofillService')), matching: find.text('Settings')));
    await tester.pumpAndSettle();
    expect(mobile.calls, contains('openAutofillSettings'));
    expect(find.text('KeyRoam is your autofill service'), findsOneWidget);
    final credentials = find.descendant(
      of: find.byKey(const Key('credentialProvider')),
      matching: find.text('Settings'),
    );
    await tester.ensureVisible(credentials);
    await tester.pumpAndSettle();
    await tester.tap(credentials);
    expect(mobile.calls, contains('openCredentialSettings'));
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
    await finish(tester, c);
  });

  testWidgets('desktop has no Android menu items', (tester) async {
    tester.view.physicalSize = const Size(1280, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final api = FakeKeyRoamApi(hasVault: true);
    final c = AppController(api, timings: timings);
    await c.init();
    await tester.pumpWidget(KeyRoamApp(controller: c));
    await tester.pumpAndSettle();
    await unlock(tester);
    await tester.tap(find.byKey(const Key('menu')));
    await tester.pumpAndSettle();
    expect(find.text('Autofill…'), findsNothing);
    expect(find.text('Biometric unlock…'), findsNothing);
    await tester.tapAt(const Offset(5, 5));
    await tester.pumpAndSettle();
    await finish(tester, c);
  });
}
