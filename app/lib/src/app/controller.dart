import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../engine/api.dart';
import '../engine/models.dart';

enum Phase { loading, onboarding, locked, unlocked }

/// Timing policy. Mobile OSes restrict background work, so these timers only
/// apply while the app is running; nothing here promises real-time sync.
class AppTimings {
  const AppTimings({
    this.periodicSync = const Duration(minutes: 5),
    this.offlineRetry = const Duration(seconds: 30),
    this.syncDebounce = const Duration(seconds: 2),
    this.autoLock = const Duration(minutes: 5),
    this.clipboardClear = const Duration(seconds: 30),
  });

  final Duration periodicSync;
  final Duration offlineRetry;
  final Duration syncDebounce;
  final Duration autoLock;
  final Duration clipboardClear;
}

/// Application state and behaviour shared by all screens.
class AppController extends ChangeNotifier {
  AppController(this.api, {this.timings = const AppTimings()});

  final KeyRoamApi api;
  final AppTimings timings;

  AppState? state;
  String? fatalError;
  List<EntrySummary> entries = const [];
  String query = '';
  String? selectedId;
  EntryDetail? selected;
  bool syncing = false;

  /// One-shot message for a snackbar; consumed by the UI.
  String? notice;

  Timer? _periodic;
  Timer? _debounce;
  Timer? _retry;
  Timer? _idle;
  Timer? _clipboard;
  bool _disposed = false;

  Phase get phase {
    final s = state;
    if (s == null) return Phase.loading;
    if (!s.hasVault) return Phase.onboarding;
    return s.unlocked ? Phase.unlocked : Phase.locked;
  }

  /// Folder suggested for backup exports (null: engine default).
  String? backupDir;

  Future<void> init({String? dataDir, String? backupDir}) async {
    this.backupDir = backupDir;
    try {
      state = await api.open(dataDir: dataDir);
    } on KeyRoamException catch (e) {
      fatalError = e.message;
    }
    _notify();
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  void _say(String message) {
    notice = message;
    _notify();
  }

  String? takeNotice() {
    final n = notice;
    notice = null;
    return n;
  }

  // ----- lock / unlock -----

  Future<void> unlock(String password) async {
    state = await api.unlock(password);
    await _afterUnlock();
  }

  /// Called after onboarding created or joined the vault.
  Future<void> completeOnboarding(OnboardingResult result) async {
    state = result.state;
    if (result.syncError != null) {
      notice = 'Saved on this device. Sync will retry: ${result.syncError}';
    }
    await _afterUnlock(syncNow: false);
  }

  Future<void> _afterUnlock({bool syncNow = true}) async {
    await refreshEntries();
    _periodic?.cancel();
    _periodic = Timer.periodic(timings.periodicSync, (_) => this.syncNow(quiet: true));
    userActivity();
    if (syncNow) {
      // Sync when the vault is opened.
      unawaited(this.syncNow(quiet: true));
    }
  }

  Future<void> lock() async {
    for (final t in [_periodic, _debounce, _retry, _idle]) {
      t?.cancel();
    }
    await api.lock();
    entries = const [];
    selected = null;
    selectedId = null;
    query = '';
    state = await api.state();
    _notify();
  }

  /// Reset the auto-lock timer; called on any pointer or key activity.
  void userActivity() {
    if (phase != Phase.unlocked) return;
    _idle?.cancel();
    _idle = Timer(timings.autoLock, () {
      if (phase == Phase.unlocked) {
        notice = 'Locked after inactivity';
        unawaited(lock());
      }
    });
  }

  // ----- entries -----

  Future<void> refreshEntries() async {
    if (state?.unlocked != true) return;
    entries = await api.listEntries(query: query.isEmpty ? null : query);
    if (selectedId != null) {
      try {
        selected = await api.getEntry(selectedId!);
      } on KeyRoamException {
        selected = null;
        selectedId = null;
      }
    }
    state = await api.state();
    _notify();
  }

  Future<void> search(String q) async {
    query = q;
    await refreshEntries();
  }

  Future<void> select(String? id) async {
    selectedId = id;
    selected = id == null ? null : await api.getEntry(id);
    _notify();
  }

  Future<void> _modified() async {
    await refreshEntries();
    // Sync after local modifications (debounced to batch quick edits).
    _debounce?.cancel();
    _debounce = Timer(timings.syncDebounce, () => syncNow(quiet: true));
  }

  Future<String> addEntry(EntryData data) async {
    final id = await api.addEntry(data);
    selectedId = id;
    await _modified();
    return id;
  }

  Future<void> updateEntry(String id, EntryData data) async {
    await api.updateEntry(id, data);
    await _modified();
  }

  Future<void> deleteEntry(String id) async {
    await api.deleteEntry(id);
    if (selectedId == id) {
      selectedId = null;
      selected = null;
    }
    await _modified();
  }

  Future<void> restoreEntry(String id) async {
    await api.restoreEntry(id);
    await _modified();
  }

  Future<void> resolveConflict(String id, int choice) async {
    await api.resolveConflict(id, choice);
    await _modified();
  }

  Future<List<EntrySummary>> deletedEntries() => api.listEntries(deleted: true);

  // ----- sync -----

  /// "Sync Now". With [quiet], success is not announced.
  Future<void> syncNow({bool quiet = false}) async {
    if (syncing || phase != Phase.unlocked) return;
    if (state?.signedIn != true) {
      if (!quiet) _say('Sign in to Google to synchronise this device.');
      return;
    }
    syncing = true;
    _retry?.cancel();
    _notify();
    try {
      final r = await api.sync();
      state = r.state;
      if (r.report.localChanged) await refreshEntries();
      if (r.report.headerAdopted) {
        _say('Your master password was changed on another device. Use the new password next time.');
      } else if (r.report.conflicts > 0 && r.report.localChanged) {
        _say('${r.report.conflicts} item(s) were edited on two devices. Review the conflicts.');
      } else if (r.report.warnings.isNotEmpty) {
        _say('Sync warning: ${r.report.warnings.first}');
      } else if (!quiet) {
        _say('Synchronised');
      }
    } on KeyRoamException catch (e) {
      state = await api.state();
      if (e.isOffline) {
        // Retry when connectivity returns.
        _retry = Timer(timings.offlineRetry, () => syncNow(quiet: true));
        if (!quiet) _say('Offline. Changes are saved on this device and will sync later.');
      } else if (e.code == 'auth_required') {
        _say('Google sign-in expired. Sign in again from the menu.');
      } else if (!quiet || e.code != 'not_configured') {
        _say('Sync failed: ${e.message}');
      }
    } finally {
      syncing = false;
      _notify();
    }
  }

  // ----- clipboard -----

  /// Copy a secret and clear the clipboard after a delay if it is unchanged.
  Future<void> copySecret(String value, {String label = 'Copied'}) async {
    await Clipboard.setData(ClipboardData(text: value));
    _clipboard?.cancel();
    _clipboard = Timer(timings.clipboardClear, () async {
      final current = await Clipboard.getData(Clipboard.kTextPlain);
      if (current?.text == value) {
        await Clipboard.setData(const ClipboardData(text: ''));
      }
    });
    _say('$label. Clipboard clears in ${timings.clipboardClear.inSeconds} s.');
  }

  Future<void> copyPlain(String value, {String label = 'Copied'}) async {
    await Clipboard.setData(ClipboardData(text: value));
    _say(label);
  }

  // ----- account and backups -----

  Future<void> afterSignIn(AppState s) async {
    state = s;
    _notify();
    await syncNow();
  }

  Future<void> signOut() async {
    state = await api.signOut();
    _say('Signed out of Google. The vault stays available on this device.');
  }

  Future<void> changePassword(String current, String newPassword) async {
    await api.changePassword(current, newPassword);
    await _modified();
    _say('Master password changed. Other devices will use it after they sync.');
  }

  Future<ImportResult> importBackup(String path, String password) async {
    final r = await api.importBackup(path, password);
    await _modified();
    return r;
  }

  @override
  void dispose() {
    _disposed = true;
    for (final t in [_periodic, _debounce, _retry, _idle, _clipboard]) {
      t?.cancel();
    }
    super.dispose();
  }
}
