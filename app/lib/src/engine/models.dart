// Data types exchanged with the engine. Field names mirror the Rust DTOs in
// crates/keyroam-ffi/src/api.rs.

class SyncInfo {
  const SyncInfo({required this.status, this.lastSuccessAt, this.message, this.pendingUpload = false});

  /// One of `never`, `up_to_date`, `pending`, `offline`, `error`.
  final String status;
  final DateTime? lastSuccessAt;
  final String? message;
  final bool pendingUpload;

  factory SyncInfo.fromJson(Map<String, dynamic> j) => SyncInfo(
    status: j['status'] as String,
    lastSuccessAt: _time(j['last_success_at']),
    message: j['message'] as String?,
    pendingUpload: j['pending_upload'] as bool? ?? false,
  );
}

class AppState {
  const AppState({
    this.dataDir,
    required this.hasVault,
    required this.unlocked,
    required this.signedIn,
    required this.googleConfigured,
    this.vaultId,
    this.deviceId,
    this.entryCount = 0,
    this.conflicts = 0,
    this.sync = const SyncInfo(status: 'never'),
    this.browserIntegration = false,
    this.revision = 0,
  });

  final String? dataDir;
  final bool hasVault;
  final bool unlocked;
  final bool signedIn;
  final bool googleConfigured;
  final String? vaultId;
  final String? deviceId;
  final int entryCount;
  final int conflicts;
  final SyncInfo sync;

  /// Browser integration is on: the KeyRoam extension can reach this app.
  final bool browserIntegration;

  /// Changes whenever the browser extension modified the vault.
  final int revision;

  factory AppState.fromJson(Map<String, dynamic> j) => AppState(
    dataDir: j['data_dir'] as String?,
    hasVault: j['has_vault'] as bool,
    unlocked: j['unlocked'] as bool,
    signedIn: j['signed_in'] as bool,
    googleConfigured: j['google_configured'] as bool,
    vaultId: j['vault_id'] as String?,
    deviceId: j['device_id'] as String?,
    entryCount: j['entry_count'] as int? ?? 0,
    conflicts: j['conflicts'] as int? ?? 0,
    sync: SyncInfo.fromJson(j['sync'] as Map<String, dynamic>),
    browserIntegration: j['browser_integration'] as bool? ?? false,
    revision: j['revision'] as int? ?? 0,
  );
}

class EntryData {
  const EntryData({
    required this.title,
    this.username = '',
    this.password = '',
    this.urls = const [],
    this.notes = '',
    this.tags = const [],
    this.favorite = false,
  });

  final String title;
  final String username;
  final String password;
  final List<String> urls;
  final String notes;
  final List<String> tags;
  final bool favorite;

  factory EntryData.fromJson(Map<String, dynamic> j) => EntryData(
    title: j['title'] as String? ?? '',
    username: j['username'] as String? ?? '',
    password: j['password'] as String? ?? '',
    urls: _strings(j['urls']),
    notes: j['notes'] as String? ?? '',
    tags: _strings(j['tags']),
    favorite: j['favorite'] as bool? ?? false,
  );

  Map<String, dynamic> toJson() => {
    'title': title,
    'username': username,
    'password': password,
    'urls': urls,
    'notes': notes,
    'tags': tags,
    'favorite': favorite,
  };

  bool sameContentAs(EntryData o) =>
      title == o.title &&
      username == o.username &&
      password == o.password &&
      notes == o.notes &&
      favorite == o.favorite &&
      _listEq(urls, o.urls) &&
      _listEq(tags, o.tags);

  @override
  String toString() => 'EntryData($title, $username, <redacted>)';
}

class EntrySummary {
  const EntrySummary({
    required this.id,
    required this.title,
    this.username = '',
    this.url = '',
    this.tags = const [],
    this.favorite = false,
    this.conflict = false,
    this.deleted = false,
    required this.modifiedAt,
  });

  final String id;
  final String title;
  final String username;
  final String url;
  final List<String> tags;
  final bool favorite;
  final bool conflict;
  final bool deleted;
  final DateTime modifiedAt;

  factory EntrySummary.fromJson(Map<String, dynamic> j) => EntrySummary(
    id: j['id'] as String,
    title: j['title'] as String,
    username: j['username'] as String? ?? '',
    url: j['url'] as String? ?? '',
    tags: _strings(j['tags']),
    favorite: j['favorite'] as bool? ?? false,
    conflict: j['conflict'] as bool? ?? false,
    deleted: j['deleted'] as bool? ?? false,
    modifiedAt: _time(j['modified_at'])!,
  );
}

class EntryVersion {
  const EntryVersion({required this.modifiedAt, required this.modifiedBy, this.deleted = false, this.data});

  final DateTime modifiedAt;
  final String modifiedBy;
  final bool deleted;
  final EntryData? data;

  factory EntryVersion.fromJson(Map<String, dynamic> j) => EntryVersion(
    modifiedAt: _time(j['modified_at'])!,
    modifiedBy: j['modified_by'] as String,
    deleted: j['deleted'] as bool? ?? false,
    data: j['data'] == null ? null : EntryData.fromJson(j['data'] as Map<String, dynamic>),
  );
}

class EntryDetail {
  const EntryDetail({
    required this.id,
    required this.conflict,
    required this.deleted,
    required this.versions,
    this.history = const [],
  });

  final String id;
  final bool conflict;
  final bool deleted;

  /// Concurrent versions; more than one with different content is a conflict.
  final List<EntryVersion> versions;
  final List<EntryVersion> history;

  /// The content shown by default.
  EntryData? get data =>
      versions.map((v) => v.data).followedBy(history.map((v) => v.data)).whereType<EntryData>().firstOrNull;

  factory EntryDetail.fromJson(Map<String, dynamic> j) => EntryDetail(
    id: j['id'] as String,
    conflict: j['conflict'] as bool,
    deleted: j['deleted'] as bool,
    versions: (j['versions'] as List).map((v) => EntryVersion.fromJson(v as Map<String, dynamic>)).toList(),
    history: (j['history'] as List? ?? []).map((v) => EntryVersion.fromJson(v as Map<String, dynamic>)).toList(),
  );
}

class SyncReport {
  const SyncReport({
    this.replicasMerged = 0,
    this.localChanged = false,
    this.uploaded = false,
    this.headerAdopted = false,
    this.conflicts = 0,
    this.warnings = const [],
  });

  final int replicasMerged;
  final bool localChanged;
  final bool uploaded;
  final bool headerAdopted;
  final int conflicts;
  final List<String> warnings;

  factory SyncReport.fromJson(Map<String, dynamic> j) => SyncReport(
    replicasMerged: j['replicas_merged'] as int? ?? 0,
    localChanged: j['local_changed'] as bool? ?? false,
    uploaded: j['uploaded'] as bool? ?? false,
    headerAdopted: j['header_adopted'] as bool? ?? false,
    conflicts: j['conflicts'] as int? ?? 0,
    warnings: _strings(j['warnings']),
  );
}

class SyncResult {
  const SyncResult(this.report, this.state);
  final SyncReport report;
  final AppState state;
}

/// Result of creating or joining a vault. The vault exists locally even if
/// the first sync failed ([syncError] set).
class OnboardingResult {
  const OnboardingResult(this.state, {this.report, this.syncError});
  final AppState state;
  final SyncReport? report;
  final String? syncError;

  factory OnboardingResult.fromJson(Map<String, dynamic> j) {
    final sync = j['sync'] as Map<String, dynamic>;
    return OnboardingResult(
      AppState.fromJson(j['state'] as Map<String, dynamic>),
      report: sync['report'] == null ? null : SyncReport.fromJson(sync['report'] as Map<String, dynamic>),
      syncError: sync['error'] as String?,
    );
  }
}

/// Outcome of importing a CSV export of another password manager.
class CsvImportResult {
  const CsvImportResult({
    required this.format,
    required this.added,
    required this.duplicates,
    required this.skipped,
    required this.deleted,
  });

  /// Detected source: Chrome, Firefox, Safari, Bitwarden, LastPass,
  /// 1Password, KeePass or CSV.
  final String format;
  final int added;
  final int duplicates;
  final int skipped;
  final bool deleted;

  factory CsvImportResult.fromJson(Map<String, dynamic> j) => CsvImportResult(
    format: j['format'] as String,
    added: j['added'] as int,
    duplicates: j['duplicates'] as int,
    skipped: j['skipped'] as int,
    deleted: j['deleted'] as bool,
  );
}

class SignInStart {
  const SignInStart(this.url, this.browserOpened);
  final String url;
  final bool browserOpened;
}

class ImportResult {
  const ImportResult({required this.before, required this.after, required this.changed});
  final int before;
  final int after;
  final bool changed;
}

class GeneratorOptions {
  const GeneratorOptions({
    this.length = 20,
    this.lowercase = true,
    this.uppercase = true,
    this.digits = true,
    this.symbols = true,
    this.avoidAmbiguous = true,
  });

  final int length;
  final bool lowercase;
  final bool uppercase;
  final bool digits;
  final bool symbols;
  final bool avoidAmbiguous;

  GeneratorOptions copyWith({
    int? length,
    bool? lowercase,
    bool? uppercase,
    bool? digits,
    bool? symbols,
    bool? avoidAmbiguous,
  }) => GeneratorOptions(
    length: length ?? this.length,
    lowercase: lowercase ?? this.lowercase,
    uppercase: uppercase ?? this.uppercase,
    digits: digits ?? this.digits,
    symbols: symbols ?? this.symbols,
    avoidAmbiguous: avoidAmbiguous ?? this.avoidAmbiguous,
  );

  Map<String, dynamic> toJson() => {
    'length': length,
    'lowercase': lowercase,
    'uppercase': uppercase,
    'digits': digits,
    'symbols': symbols,
    'avoid_ambiguous': avoidAmbiguous,
  };
}

DateTime? _time(Object? ms) => ms == null ? null : DateTime.fromMillisecondsSinceEpoch(ms as int);

List<String> _strings(Object? v) => (v as List? ?? const []).cast<String>().toList();

bool _listEq(List<String> a, List<String> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}
