import 'dart:convert';
import 'dart:isolate';

import 'models.dart';
import 'native.dart';

/// Error reported by the engine. [code] is stable and machine-readable
/// (e.g. `wrong_password`, `offline`, `auth_required`).
class KeyRoamException implements Exception {
  const KeyRoamException(this.code, this.message);
  final String code;
  final String message;

  bool get isOffline => code == 'offline';

  @override
  String toString() => message;
}

/// Everything the UI can ask of the vault and sync engine.
abstract class KeyRoamApi {
  Future<AppState> open({String? dataDir});
  Future<AppState> state();
  Future<void> checkPassword(String password);
  Future<String> generatePassword(GeneratorOptions options);
  Future<String> defaultBackupPath({String? dir});

  // Google sign-in (desktop loopback flow).
  Future<SignInStart> beginSignIn();
  Future<AppState> completeSignIn();
  Future<void> cancelSignIn();
  Future<AppState> signOut();

  // Onboarding.
  Future<OnboardingResult> createVault(String password);
  Future<void> fetchRemoteVault();
  Future<OnboardingResult> joinVault(String password);

  Future<AppState> unlock(String password);
  Future<void> lock();

  Future<List<EntrySummary>> listEntries({String? query, bool deleted = false});
  Future<EntryDetail> getEntry(String id);
  Future<String> addEntry(EntryData data);
  Future<void> updateEntry(String id, EntryData data);
  Future<void> deleteEntry(String id);
  Future<void> restoreEntry(String id);
  Future<void> resolveConflict(String id, int choice);

  Future<SyncResult> sync();
  Future<int> exportBackup(String path, String password, {bool overwrite = false});
  Future<ImportResult> importBackup(String path, String password);
  Future<void> changePassword(String current, String newPassword);

  // Browser autofill (desktop only): the local channel used by the extension's
  // native-messaging host, its registration with the browsers, and a revision
  // counter that changes when the extension saves a login.
  Future<AutofillStatus> startAutofill();
  Future<AutofillStatus> autofillStatus();
  Future<AutofillStatus> installAutofill();
  Future<int> autofillChanges();
}

/// [KeyRoamApi] backed by the Rust engine. Every call runs on a short-lived
/// background isolate so Argon2 and network work never block the UI.
class NativeKeyRoamApi implements KeyRoamApi {
  static Future<String> _invoke(String request) => Isolate.run(() => invokeEngine(request));

  Future<dynamic> _call(String method, [Map<String, dynamic> params = const {}]) async {
    final response =
        jsonDecode(await _invoke(jsonEncode({'method': method, 'params': params}))) as Map<String, dynamic>;
    final error = response['error'] as Map<String, dynamic>?;
    if (error != null) {
      throw KeyRoamException(error['code'] as String, error['message'] as String);
    }
    return response['ok'];
  }

  Future<AppState> _state(String method, [Map<String, dynamic> params = const {}]) async =>
      AppState.fromJson(await _call(method, params) as Map<String, dynamic>);

  @override
  Future<AppState> open({String? dataDir}) => _state('open', {'data_dir': dataDir});
  @override
  Future<AppState> state() => _state('state');
  @override
  Future<void> checkPassword(String password) => _call('check_password', {'password': password});
  @override
  Future<String> generatePassword(GeneratorOptions options) async =>
      await _call('generate_password', options.toJson()) as String;
  @override
  Future<String> defaultBackupPath({String? dir}) async => await _call('default_backup_path', {'dir': dir}) as String;

  @override
  Future<SignInStart> beginSignIn() async {
    final r = await _call('begin_sign_in') as Map<String, dynamic>;
    return SignInStart(r['url'] as String, r['browser_opened'] as bool);
  }

  @override
  Future<AppState> completeSignIn() => _state('complete_sign_in');
  @override
  Future<void> cancelSignIn() => _call('cancel_sign_in');
  @override
  Future<AppState> signOut() => _state('sign_out');

  @override
  Future<OnboardingResult> createVault(String password) async =>
      OnboardingResult.fromJson(await _call('create_vault', {'password': password}) as Map<String, dynamic>);
  @override
  Future<void> fetchRemoteVault() => _call('fetch_remote_vault');
  @override
  Future<OnboardingResult> joinVault(String password) async =>
      OnboardingResult.fromJson(await _call('join_vault', {'password': password}) as Map<String, dynamic>);

  @override
  Future<AppState> unlock(String password) => _state('unlock', {'password': password});
  @override
  Future<void> lock() => _call('lock');

  @override
  Future<List<EntrySummary>> listEntries({String? query, bool deleted = false}) async {
    final r = await _call('list_entries', {'query': query, 'deleted': deleted}) as List;
    return r.map((e) => EntrySummary.fromJson(e as Map<String, dynamic>)).toList();
  }

  @override
  Future<EntryDetail> getEntry(String id) async =>
      EntryDetail.fromJson(await _call('get_entry', {'id': id}) as Map<String, dynamic>);
  @override
  Future<String> addEntry(EntryData data) async =>
      (await _call('add_entry', {'data': data.toJson()}) as Map<String, dynamic>)['id'] as String;
  @override
  Future<void> updateEntry(String id, EntryData data) => _call('update_entry', {'id': id, 'data': data.toJson()});
  @override
  Future<void> deleteEntry(String id) => _call('delete_entry', {'id': id});
  @override
  Future<void> restoreEntry(String id) => _call('restore_entry', {'id': id});
  @override
  Future<void> resolveConflict(String id, int choice) => _call('resolve_conflict', {'id': id, 'choice': choice});

  @override
  Future<SyncResult> sync() async {
    final r = await _call('sync') as Map<String, dynamic>;
    return SyncResult(
      SyncReport.fromJson(r['report'] as Map<String, dynamic>),
      AppState.fromJson(r['state'] as Map<String, dynamic>),
    );
  }

  @override
  Future<int> exportBackup(String path, String password, {bool overwrite = false}) async {
    final r = await _call('export_backup', {'path': path, 'password': password, 'overwrite': overwrite});
    return (r as Map<String, dynamic>)['entries'] as int;
  }

  @override
  Future<ImportResult> importBackup(String path, String password) async {
    final r = await _call('import_backup', {'path': path, 'password': password}) as Map<String, dynamic>;
    return ImportResult(before: r['before'] as int, after: r['after'] as int, changed: r['changed'] as bool);
  }

  @override
  Future<void> changePassword(String current, String newPassword) =>
      _call('change_password', {'current': current, 'new': newPassword});

  Future<AutofillStatus> _autofill(String method) async =>
      AutofillStatus.fromJson(await _call(method) as Map<String, dynamic>);

  @override
  Future<AutofillStatus> startAutofill() => _autofill('autofill_start');
  @override
  Future<AutofillStatus> autofillStatus() => _autofill('autofill_status');
  @override
  Future<AutofillStatus> installAutofill() => _autofill('autofill_install');
  @override
  Future<int> autofillChanges() async => (await _call('autofill_changes') as Map<String, dynamic>)['revision'] as int;
}
