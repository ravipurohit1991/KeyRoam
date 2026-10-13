// In-memory stand-in for the engine, used only by widget tests to exercise UI
// flows. It holds synthetic data and performs no cryptography; the real
// engine is covered by native_engine_test.dart and the Rust test-suite.

import 'package:keyroam/src/engine/api.dart';
import 'package:keyroam/src/engine/models.dart';

class FakeKeyRoamApi implements KeyRoamApi {
  FakeKeyRoamApi({this.hasVault = false, this.remoteVaultExists = false, this.password = 'synthetic-master-pw-1'});

  bool hasVault;
  bool remoteVaultExists;
  String password;
  bool unlocked = false;
  bool signedIn = false;
  bool offline = false;
  String syncStatus = 'never';
  int syncCount = 0;
  bool browserIntegration = false;
  int revision = 0;
  String? installedHost;
  final calls = <String>[];
  final _entries = <String, EntryDetail>{};
  final _deleted = <String>{};
  int _next = 0;

  void _log(String c) => calls.add(c);

  /// Insert an entry, optionally with conflicting versions.
  String seed(EntryData data, {EntryData? conflictWith}) {
    final id = 'id-${_next++}'.padRight(12, '0');
    final now = DateTime.now();
    _entries[id] = EntryDetail(
      id: id,
      conflict: conflictWith != null,
      deleted: false,
      versions: [
        EntryVersion(modifiedAt: now, modifiedBy: 'device-aaaaaaaa', data: data),
        if (conflictWith != null) EntryVersion(modifiedAt: now, modifiedBy: 'device-bbbbbbbb', data: conflictWith),
      ],
    );
    return id;
  }

  AppState get _state => AppState(
    hasVault: hasVault,
    unlocked: unlocked,
    signedIn: signedIn,
    googleConfigured: true,
    entryCount: _visible.length,
    conflicts: _visible.where((e) => e.conflict).length,
    sync: SyncInfo(status: syncStatus),
    browserIntegration: browserIntegration,
    revision: revision,
  );

  Iterable<EntryDetail> get _visible => _entries.values.where((e) => !_deleted.contains(e.id));

  void _requireUnlocked() {
    if (!unlocked) throw const KeyRoamException('locked', 'the vault is locked');
  }

  @override
  Future<AppState> open({String? dataDir}) async => _state;
  @override
  Future<AppState> state() async => _state;
  @override
  Future<void> checkPassword(String password) async {
    if (password.length < 12) throw const KeyRoamException('invalid_input', 'too short');
  }

  @override
  Future<String> generatePassword(GeneratorOptions options) async => 'G' * options.length;
  @override
  Future<String> defaultBackupPath({String? dir}) async => 'backup.krbackup';

  @override
  Future<SignInStart> beginSignIn() async {
    _log('beginSignIn');
    return const SignInStart('https://accounts.google.com/o/oauth2/v2/auth?synthetic', true);
  }

  @override
  Future<AppState> completeSignIn() async {
    _log('completeSignIn');
    signedIn = true;
    return _state;
  }

  @override
  Future<void> cancelSignIn() async => _log('cancelSignIn');
  @override
  Future<AppState> signOut() async {
    signedIn = false;
    return _state;
  }

  @override
  Future<OnboardingResult> createVault(String password) async {
    _log('createVault');
    if (!signedIn) throw const KeyRoamException('not_signed_in', 'sign in first');
    if (remoteVaultExists) throw const KeyRoamException('remote_vault_exists', 'exists');
    this.password = password;
    hasVault = true;
    unlocked = true;
    remoteVaultExists = true;
    syncStatus = 'up_to_date';
    return OnboardingResult(_state, report: const SyncReport(uploaded: true));
  }

  @override
  Future<void> fetchRemoteVault() async {
    _log('fetchRemoteVault');
    if (!remoteVaultExists) throw const KeyRoamException('no_remote_vault', 'none');
  }

  @override
  Future<OnboardingResult> joinVault(String password) async {
    _log('joinVault');
    if (password != this.password) throw const KeyRoamException('wrong_password', 'wrong');
    hasVault = true;
    unlocked = true;
    syncStatus = 'up_to_date';
    return OnboardingResult(_state, report: const SyncReport());
  }

  @override
  Future<AppState> unlock(String password) async {
    if (password != this.password) throw const KeyRoamException('wrong_password', 'wrong');
    unlocked = true;
    return _state;
  }

  @override
  Future<void> lock() async => unlocked = false;

  @override
  Future<List<EntrySummary>> listEntries({String? query, bool deleted = false}) async {
    _requireUnlocked();
    final source = deleted ? _entries.values.where((e) => _deleted.contains(e.id)) : _visible;
    return source
        .where((e) => query == null || e.data!.title.toLowerCase().contains(query.toLowerCase()))
        .map(
          (e) => EntrySummary(
            id: e.id,
            title: e.data!.title,
            username: e.data!.username,
            conflict: e.conflict,
            deleted: deleted,
            modifiedAt: e.versions.first.modifiedAt,
          ),
        )
        .toList();
  }

  @override
  Future<EntryDetail> getEntry(String id) async {
    _requireUnlocked();
    return _entries[id] ?? (throw const KeyRoamException('not_found', 'entry not found'));
  }

  @override
  Future<String> addEntry(EntryData data) async {
    _requireUnlocked();
    _log('addEntry');
    return seed(data);
  }

  @override
  Future<void> updateEntry(String id, EntryData data) async {
    _log('updateEntry');
    final old = _entries[id]!;
    _entries[id] = EntryDetail(
      id: id,
      conflict: false,
      deleted: false,
      versions: [EntryVersion(modifiedAt: DateTime.now(), modifiedBy: 'device-aaaaaaaa', data: data)],
      history: [...old.versions, ...old.history],
    );
  }

  @override
  Future<void> deleteEntry(String id) async {
    _log('deleteEntry');
    _deleted.add(id);
  }

  @override
  Future<void> restoreEntry(String id) async => _deleted.remove(id);

  @override
  Future<void> purgeEntry(String id) async {
    _log('purge:$id');
    if (!_deleted.remove(id)) throw const KeyRoamException('invalid_input', 'only deleted entries');
    _entries.remove(id);
  }

  @override
  Future<int> purgeDeleted() async {
    final n = _deleted.length;
    _deleted.toList().forEach(purgeEntry);
    return n;
  }

  @override
  Future<void> resolveConflict(String id, int choice) async {
    _log('resolveConflict:$choice');
    await updateEntry(id, _entries[id]!.versions[choice].data!);
  }

  @override
  Future<SyncResult> sync() async {
    syncCount++;
    if (remoteKeyRotated) throw const KeyRoamException('key_rotated', 'the vault key was replaced');
    if (offline) {
      syncStatus = 'offline';
      throw const KeyRoamException('offline', 'network unavailable');
    }
    syncStatus = 'up_to_date';
    return SyncResult(const SyncReport(), _state);
  }

  @override
  Future<int> exportBackup(String path, String password, {bool overwrite = false}) async {
    _log('export:$path');
    return _visible.length;
  }

  @override
  Future<ImportResult> importBackup(String path, String password) async =>
      ImportResult(before: _visible.length, after: _visible.length, changed: false);

  /// Simulate the browser extension saving a login through the engine.
  String saveFromBrowser(EntryData data) {
    final id = seed(data);
    revision++;
    return id;
  }

  bool remoteKeyRotated = false;

  @override
  Future<CsvImportResult> importCsv(String path, {bool deleteFile = false}) async {
    _log('importCsv:$path:$deleteFile');
    if (!path.endsWith('.csv')) throw const KeyRoamException('invalid_input', 'no password column found');
    seed(const EntryData(title: 'imported.example', username: 'csv-user'));
    return CsvImportResult(format: 'Chrome', added: 1, duplicates: 2, skipped: 0, deleted: deleteFile);
  }

  @override
  Future<AppState> rotateVaultKey(String password) async {
    if (password != this.password) throw const KeyRoamException('wrong_password', 'wrong');
    _log('rotateVaultKey');
    return _state;
  }

  @override
  Future<OnboardingResult> adoptRotatedKey(String password) async {
    if (password != this.password) throw const KeyRoamException('wrong_password', 'wrong');
    _log('adoptRotatedKey');
    remoteKeyRotated = false;
    return OnboardingResult(_state, report: const SyncReport());
  }

  @override
  Future<AppState> setBrowserIntegration(bool enabled) async {
    _log('browserIntegration:$enabled');
    browserIntegration = enabled;
    return _state;
  }

  @override
  Future<List<String>> installBrowserHost(String hostPath) async {
    installedHost = hostPath;
    return ['Google Chrome', 'Firefox'];
  }

  @override
  Future<void> changePassword(String current, String newPassword) async {
    if (current != password) throw const KeyRoamException('wrong_password', 'wrong');
    password = newPassword;
  }
}
