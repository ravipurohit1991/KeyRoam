// Exercises the real Rust engine through FFI (built by hook/build.dart).
// Uses only synthetic passwords and a committed synthetic fixture vault.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:keyroam/src/engine/api.dart';
import 'package:keyroam/src/engine/models.dart';

const fixturePassword = 'synthetic-master-pw-1';

void main() {
  final api = NativeKeyRoamApi();
  late Directory dir;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('keyroam_test');
  });

  tearDown(() async {
    await api.lock();
    await dir.delete(recursive: true);
  });

  test('generator and password policy', () async {
    final pw = await api.generatePassword(const GeneratorOptions(length: 24, symbols: false));
    expect(pw.length, 24);
    expect(RegExp(r'^[A-Za-z0-9]+$').hasMatch(pw), isTrue);
    await expectLater(api.checkPassword('short'), throwsA(isA<KeyRoamException>()));
    await api.checkPassword('long-enough-synthetic');
  });

  test('empty data directory needs onboarding', () async {
    final st = await api.open(dataDir: dir.path);
    expect(st.hasVault, isFalse);
    expect(st.unlocked, isFalse);
    expect(st.signedIn, isFalse);
    await expectLater(
      api.createVault(fixturePassword),
      throwsA(isA<KeyRoamException>().having((e) => e.code, 'code', 'not_signed_in')),
    );
  });

  test('fixture vault: unlock, edit, conflicts-free history, backup round trip', () async {
    File('test/fixtures/vault.krv').copySync('${dir.path}${Platform.pathSeparator}vault.krv');
    var st = await api.open(dataDir: dir.path);
    expect(st.hasVault, isTrue);
    await expectLater(
      api.unlock('wrong-password-xyz'),
      throwsA(isA<KeyRoamException>().having((e) => e.code, 'code', 'wrong_password')),
    );
    st = await api.unlock(fixturePassword);
    expect(st.unlocked, isTrue);
    expect(st.entryCount, 2);

    final list = await api.listEntries();
    expect(list.map((e) => e.title), ['Example Bank', 'Example Mail']);
    final id = await api.addEntry(const EntryData(title: 'Added from Dart', password: 'synthetic-dart'));
    await api.updateEntry(id, const EntryData(title: 'Added from Dart', password: 'synthetic-dart-2'));
    final detail = await api.getEntry(id);
    expect(detail.data!.password, 'synthetic-dart-2');
    expect(detail.history.single.data!.password, 'synthetic-dart');
    expect((await api.listEntries(query: 'dart')).length, 1);

    final backup = '${dir.path}${Platform.pathSeparator}b.krbackup';
    expect(await api.exportBackup(backup, 'backup-synthetic-pw'), 3);
    await api.deleteEntry(id);
    expect((await api.listEntries()).length, 2);
    final r = await api.importBackup(backup, 'backup-synthetic-pw');
    // Restore merges: the newer deletion wins over the older backup copy.
    expect(r.changed, isFalse);
    await api.restoreEntry(id);
    expect((await api.listEntries()).length, 3);

    // Not signed in: sync reports it clearly; status shows pending changes.
    await expectLater(api.sync(), throwsA(isA<KeyRoamException>().having((e) => e.code, 'code', 'not_signed_in')));
    expect((await api.state()).sync.pendingUpload, isTrue);

    await api.lock();
    await expectLater(api.listEntries(), throwsA(isA<KeyRoamException>()));
  });

  test('browser autofill channel starts and publishes its endpoint', () async {
    await api.open(dataDir: dir.path);
    expect((await api.startAutofill()).running, isTrue);
    final st = await api.autofillStatus();
    expect(st.running, isTrue);
    expect(st.browsers.map((b) => b.browser), containsAll(['chrome', 'edge', 'firefox']));
    expect(st.chromiumExtensionId, hasLength(32));
    expect(File('${dir.path}${Platform.pathSeparator}autofill-endpoint.json').existsSync(), isTrue);
    expect(await api.autofillChanges(), 0);
    if (!st.hostFound) {
      // The test runner has no keyroam-native-host next to it.
      await expectLater(
        api.installAutofill(),
        throwsA(isA<KeyRoamException>().having((e) => e.code, 'code', 'host_missing')),
      );
    }
  });
}
