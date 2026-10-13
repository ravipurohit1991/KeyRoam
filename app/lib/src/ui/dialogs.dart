import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../engine/api.dart';
import '../engine/models.dart';
import 'common.dart';

/// Add or edit an entry. Returns the new content, or null if cancelled.
Future<EntryData?> showEntryEditor(BuildContext context, AppController controller, {EntryData? initial}) {
  return showDialog<EntryData>(
    context: context,
    builder: (_) => _EntryEditor(controller: controller, initial: initial),
  );
}

class _EntryEditor extends StatefulWidget {
  const _EntryEditor({required this.controller, this.initial});
  final AppController controller;
  final EntryData? initial;

  @override
  State<_EntryEditor> createState() => _EntryEditorState();
}

class _EntryEditorState extends State<_EntryEditor> {
  final _form = GlobalKey<FormState>();
  late final _title = TextEditingController(text: widget.initial?.title);
  late final _username = TextEditingController(text: widget.initial?.username);
  late final _password = TextEditingController(text: widget.initial?.password);
  late final _urls = TextEditingController(text: widget.initial?.urls.join('\n'));
  late final _notes = TextEditingController(text: widget.initial?.notes);
  late final _tags = TextEditingController(text: widget.initial?.tags.join(', '));
  late bool _favorite = widget.initial?.favorite ?? false;

  @override
  void initState() {
    super.initState();
    if (widget.initial == null) _generate(const GeneratorOptions());
  }

  @override
  void dispose() {
    for (final c in [_title, _username, _password, _urls, _notes, _tags]) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> _generate(GeneratorOptions options) async {
    final pw = await widget.controller.api.generatePassword(options);
    if (mounted) _password.text = pw;
  }

  Future<void> _openGenerator() async {
    final pw = await showGeneratorDialog(context, widget.controller.api);
    if (pw != null) _password.text = pw;
  }

  void _save() {
    if (!_form.currentState!.validate()) return;
    List<String> split(String s, Pattern p) => s.split(p).map((e) => e.trim()).where((e) => e.isNotEmpty).toList();
    Navigator.pop(
      context,
      EntryData(
        title: _title.text.trim(),
        username: _username.text.trim(),
        password: _password.text,
        urls: split(_urls.text, RegExp(r'[\n ]')),
        notes: _notes.text,
        tags: split(_tags.text, ','),
        favorite: _favorite,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.initial == null ? 'New login' : 'Edit login'),
      content: SizedBox(
        width: 520,
        child: Form(
          key: _form,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextFormField(
                  key: const Key('entryTitle'),
                  controller: _title,
                  autofocus: true,
                  decoration: const InputDecoration(labelText: 'Title'),
                  validator: (v) => (v ?? '').trim().isEmpty ? 'A title is required' : null,
                ),
                const SizedBox(height: 12),
                TextFormField(
                  key: const Key('entryUsername'),
                  controller: _username,
                  decoration: const InputDecoration(labelText: 'Username or email'),
                ),
                const SizedBox(height: 12),
                PasswordField(
                  key: const Key('entryPassword'),
                  controller: _password,
                  label: 'Password',
                  suffix: IconButton(
                    tooltip: 'Generate password',
                    icon: const Icon(Icons.casino_outlined),
                    onPressed: _openGenerator,
                  ),
                ),
                const SizedBox(height: 12),
                TextFormField(
                  key: const Key('entryUrls'),
                  controller: _urls,
                  minLines: 1,
                  maxLines: 3,
                  decoration: const InputDecoration(labelText: 'Websites (one per line)'),
                ),
                const SizedBox(height: 12),
                TextFormField(
                  controller: _tags,
                  decoration: const InputDecoration(labelText: 'Tags (comma separated)'),
                ),
                const SizedBox(height: 12),
                TextFormField(
                  controller: _notes,
                  minLines: 2,
                  maxLines: 6,
                  decoration: const InputDecoration(labelText: 'Notes'),
                ),
                CheckboxListTile(
                  contentPadding: EdgeInsets.zero,
                  value: _favorite,
                  onChanged: (v) => setState(() => _favorite = v ?? false),
                  title: const Text('Favourite'),
                ),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
        FilledButton(key: const Key('saveEntry'), onPressed: _save, child: const Text('Save')),
      ],
    );
  }
}

/// Password generator. Returns the chosen password, or null.
Future<String?> showGeneratorDialog(BuildContext context, KeyRoamApi api) {
  return showDialog<String>(
    context: context,
    builder: (_) => _GeneratorDialog(api: api),
  );
}

class _GeneratorDialog extends StatefulWidget {
  const _GeneratorDialog({required this.api});
  final KeyRoamApi api;

  @override
  State<_GeneratorDialog> createState() => _GeneratorDialogState();
}

class _GeneratorDialogState extends State<_GeneratorDialog> {
  GeneratorOptions _options = const GeneratorOptions();
  String _value = '';
  String? _error;

  @override
  void initState() {
    super.initState();
    _regenerate();
  }

  Future<void> _regenerate() async {
    try {
      final v = await widget.api.generatePassword(_options);
      if (mounted) {
        setState(() {
          _value = v;
          _error = null;
        });
      }
    } on KeyRoamException catch (e) {
      if (mounted) setState(() => _error = e.message);
    }
  }

  void _set(GeneratorOptions o) {
    setState(() => _options = o);
    _regenerate();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Generate password'),
      content: SizedBox(
        width: 460,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(8),
              ),
              child: Row(
                children: [
                  Expanded(
                    child: SelectableText(_value, style: const TextStyle(fontFamily: 'monospace', fontSize: 16)),
                  ),
                  IconButton(tooltip: 'Regenerate', onPressed: _regenerate, icon: const Icon(Icons.refresh)),
                ],
              ),
            ),
            if (_error != null) Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
            const SizedBox(height: 12),
            Text('Length: ${_options.length}'),
            Slider(
              min: 8,
              max: 64,
              divisions: 56,
              value: _options.length.toDouble(),
              onChanged: (v) => _set(_options.copyWith(length: v.round())),
            ),
            SwitchListTile(
              title: const Text('Uppercase (A–Z)'),
              value: _options.uppercase,
              onChanged: (v) => _set(_options.copyWith(uppercase: v)),
            ),
            SwitchListTile(
              title: const Text('Digits (0–9)'),
              value: _options.digits,
              onChanged: (v) => _set(_options.copyWith(digits: v)),
            ),
            SwitchListTile(
              title: const Text('Symbols (!@#…)'),
              value: _options.symbols,
              onChanged: (v) => _set(_options.copyWith(symbols: v)),
            ),
            SwitchListTile(
              title: const Text('Avoid look-alike characters'),
              value: _options.avoidAmbiguous,
              onChanged: (v) => _set(_options.copyWith(avoidAmbiguous: v)),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
        FilledButton(onPressed: _value.isEmpty ? null : () => Navigator.pop(context, _value), child: const Text('Use')),
      ],
    );
  }
}

/// Generic form dialog for actions needing a path and/or passwords.
class _ActionDialog extends StatefulWidget {
  const _ActionDialog({
    required this.title,
    required this.description,
    required this.action,
    required this.run,
    this.initialPath,
    this.passwordLabels = const [],
    this.confirmLast = false,
  });

  final String title;
  final String description;
  final String action;
  final String? initialPath;
  final List<String> passwordLabels;

  /// Require the last password to be typed twice.
  final bool confirmLast;
  final Future<String> Function(String? path, List<String> passwords) run;

  @override
  State<_ActionDialog> createState() => _ActionDialogState();
}

class _ActionDialogState extends State<_ActionDialog> {
  late final _path = widget.initialPath == null ? null : TextEditingController(text: widget.initialPath);
  late final _passwords = [
    for (var i = 0; i < widget.passwordLabels.length + (widget.confirmLast ? 1 : 0); i++) TextEditingController(),
  ];
  String? _error;
  bool _busy = false;

  @override
  void dispose() {
    _path?.dispose();
    for (final c in _passwords) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> _submit() async {
    if (widget.confirmLast && _passwords[_passwords.length - 1].text != _passwords[_passwords.length - 2].text) {
      setState(() => _error = 'Passwords do not match');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final message = await widget.run(_path?.text.trim(), _passwords.map((c) => c.text).toList());
      if (mounted) Navigator.pop(context, message);
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final labels = [
      ...widget.passwordLabels,
      if (widget.confirmLast) 'Confirm ${widget.passwordLabels.last.toLowerCase()}',
    ];
    return AlertDialog(
      title: Text(widget.title),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(widget.description),
            if (_path != null) ...[
              const SizedBox(height: 16),
              TextField(
                controller: _path,
                decoration: const InputDecoration(labelText: 'File'),
              ),
            ],
            for (var i = 0; i < labels.length; i++) ...[
              const SizedBox(height: 12),
              PasswordField(controller: _passwords[i], label: labels[i], autofocus: i == 0 && _path == null),
            ],
            if (_error != null) ...[
              const SizedBox(height: 12),
              Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.pop(context), child: const Text('Cancel')),
        FilledButton(
          onPressed: _busy ? null : _submit,
          child: _busy
              ? const SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2))
              : Text(widget.action),
        ),
      ],
    );
  }
}

Future<String?> showExportDialog(BuildContext context, AppController c) async {
  final path = await c.api.defaultBackupPath(dir: c.backupDir);
  if (!context.mounted) return null;
  return showDialog<String>(
    context: context,
    builder: (_) => _ActionDialog(
      title: 'Export encrypted backup',
      description:
          'Google Drive\'s hidden app folder is not a backup you control. Save an encrypted copy of '
          'your vault somewhere safe (for example an external drive). Choose a backup password; you will '
          'need it to restore.',
      action: 'Export',
      initialPath: path,
      passwordLabels: const ['Backup password'],
      confirmLast: true,
      run: (path, pw) async {
        final n = await c.api.exportBackup(path!, pw[0]);
        return 'Exported $n entries to $path';
      },
    ),
  );
}

Future<String?> showImportDialog(BuildContext context, AppController c) {
  return showDialog<String>(
    context: context,
    builder: (_) => _ActionDialog(
      title: 'Restore from backup',
      description:
          'The backup is merged into your vault: entries are added or revived, and nothing newer '
          'is overwritten. Accepts .krbackup exports and KeyRoam\'s automatic local backups.',
      action: 'Restore',
      initialPath: '',
      passwordLabels: const ['Backup password'],
      run: (path, pw) async {
        final r = await c.importBackup(path!, pw[0]);
        return r.changed ? 'Restored: ${r.before} → ${r.after} entries' : 'Nothing new in that backup';
      },
    ),
  );
}

Future<String?> showRotateKeyDialog(BuildContext context, AppController c) {
  return showDialog<String>(
    context: context,
    builder: (_) => _ActionDialog(
      title: 'Replace vault key',
      description:
          'Creates a new encryption key for your vault. Use it if an old master password or device may be '
          'compromised: data you save from now on cannot be read with the old key. Each of your other devices '
          'asks for the master password once.',
      action: 'Replace key',
      passwordLabels: const ['Master password'],
      run: (_, pw) async {
        await c.rotateVaultKey(pw[0]);
        return 'Vault key replaced';
      },
    ),
  );
}

Future<String?> showAdoptKeyDialog(BuildContext context, AppController c) {
  return showDialog<String>(
    context: context,
    builder: (_) => _ActionDialog(
      title: 'New vault key',
      description:
          'The vault key was replaced on another device. Enter your master password to switch this device '
          'to the new key.',
      action: 'Continue',
      passwordLabels: const ['Master password'],
      run: (_, pw) async {
        await c.adoptRotatedKey(pw[0]);
        return 'Now using the new vault key';
      },
    ),
  );
}

Future<String?> showChangePasswordDialog(BuildContext context, AppController c) {
  return showDialog<String>(
    context: context,
    builder: (_) => _ActionDialog(
      title: 'Change master password',
      description: 'Your other devices will ask for the new password after they synchronise.',
      action: 'Change',
      passwordLabels: const ['Current master password', 'New master password'],
      confirmLast: true,
      run: (_, pw) async {
        await c.changePassword(pw[0], pw[1]);
        return 'Master password changed';
      },
    ),
  );
}

Future<void> showDeletedDialog(BuildContext context, AppController c) async {
  final deleted = await c.deletedEntries();
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: const Text('Recently deleted'),
      content: SizedBox(
        width: 480,
        child: deleted.isEmpty
            ? const Text('No deleted items.')
            : ListView(
                shrinkWrap: true,
                children: [
                  for (final e in deleted)
                    ListTile(
                      title: Text(e.title),
                      subtitle: Text('Deleted ${timeAgo(e.modifiedAt)}'),
                      trailing: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          TextButton(
                            onPressed: () async {
                              await c.restoreEntry(e.id);
                              if (context.mounted) Navigator.pop(context);
                            },
                            child: const Text('Restore'),
                          ),
                          IconButton(
                            tooltip: 'Delete permanently',
                            icon: const Icon(Icons.delete_forever_outlined),
                            onPressed: () async {
                              if (!await _confirmPurge(context, e.title)) return;
                              await c.purgeEntry(e.id);
                              if (context.mounted) Navigator.pop(context);
                            },
                          ),
                        ],
                      ),
                    ),
                ],
              ),
      ),
      actions: [
        if (deleted.isNotEmpty)
          TextButton(
            key: const Key('purgeAll'),
            onPressed: () async {
              if (!await _confirmPurge(context, null)) return;
              await c.purgeDeleted();
              if (context.mounted) Navigator.pop(context);
            },
            child: const Text('Delete all permanently'),
          ),
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Close')),
      ],
    ),
  );
}

Future<bool> _confirmPurge(BuildContext context, String? title) async {
  final confirmed = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title == null ? 'Delete all permanently?' : 'Delete “$title” permanently?'),
      content: const SizedBox(
        width: 420,
        child: Text(
          'The login and its password history are erased from this device and, after syncing, '
          'from all your devices. This cannot be undone. Encrypted backups you exported earlier '
          'still contain it.',
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('Cancel')),
        FilledButton(
          key: const Key('confirmPurge'),
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Delete permanently'),
        ),
      ],
    ),
  );
  return confirmed ?? false;
}

Future<void> showSignInDialog(BuildContext context, AppController c, Widget signInStep) {
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => AlertDialog(content: SizedBox(width: 480, child: signInStep)),
  );
}

/// Turn browser integration on or off and register KeyRoam with browsers.
Future<void> showBrowserIntegrationDialog(BuildContext context, AppController c) {
  return showDialog<void>(
    context: context,
    builder: (_) => _BrowserIntegrationDialog(controller: c),
  );
}

class _BrowserIntegrationDialog extends StatefulWidget {
  const _BrowserIntegrationDialog({required this.controller});
  final AppController controller;

  @override
  State<_BrowserIntegrationDialog> createState() => _BrowserIntegrationDialogState();
}

class _BrowserIntegrationDialogState extends State<_BrowserIntegrationDialog> {
  bool _busy = false;
  String? _message;

  AppController get _c => widget.controller;

  Future<void> _run(Future<String?> Function() action) async {
    setState(() {
      _busy = true;
      _message = null;
    });
    String? message;
    try {
      message = await action();
    } on KeyRoamException catch (e) {
      message = e.message;
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _message = message;
    });
  }

  Future<String?> _toggle(bool enabled) async {
    await _c.setBrowserIntegration(enabled);
    return null;
  }

  Future<String?> _register() async {
    final browsers = await _c.installBrowserHost();
    if (browsers.isEmpty) return 'No supported browser was found.';
    return 'Registered with ${browsers.join(', ')}. Restart the browser if it is open.';
  }

  @override
  Widget build(BuildContext context) {
    final enabled = _c.state?.browserIntegration ?? false;
    return AlertDialog(
      title: const Text('Browser integration'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SwitchListTile(
              key: const Key('browserIntegration'),
              contentPadding: EdgeInsets.zero,
              title: const Text('Fill passwords with the KeyRoam browser extension'),
              subtitle: const Text(
                'Only while KeyRoam is unlocked, only on the website a login was saved for, '
                'and only when you choose a login.',
              ),
              value: enabled,
              onChanged: _busy ? null : (v) => _run(() => _toggle(v)),
            ),
            const SizedBox(height: 12),
            const Text(
              '1. Add the KeyRoam extension to Chrome, Edge, Brave, Vivaldi or Firefox.\n'
              '2. Register KeyRoam with your browsers (once per computer):',
            ),
            const SizedBox(height: 8),
            OutlinedButton.icon(
              key: const Key('registerBrowsers'),
              onPressed: _busy ? null : () => _run(_register),
              icon: const Icon(Icons.extension_outlined),
              label: const Text('Register with browsers'),
            ),
            if (_message != null) ...[const SizedBox(height: 12), SelectableText(_message!)],
          ],
        ),
      ),
      actions: [TextButton(onPressed: () => Navigator.pop(context), child: const Text('Close'))],
    );
  }
}

/// Import logins from another browser's or password manager's CSV export.
Future<String?> showCsvImportDialog(BuildContext context, AppController c) {
  return showDialog<String>(
    context: context,
    builder: (_) => _CsvImportDialog(controller: c),
  );
}

class _CsvImportDialog extends StatefulWidget {
  const _CsvImportDialog({required this.controller});
  final AppController controller;

  @override
  State<_CsvImportDialog> createState() => _CsvImportDialogState();
}

class _CsvImportDialogState extends State<_CsvImportDialog> {
  final _path = TextEditingController();
  bool _delete = true;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  Future<void> _import() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final r = await widget.controller.importCsv(_path.text.trim(), deleteFile: _delete);
      if (!mounted) return;
      final parts = [
        'Imported ${r.added} login${r.added == 1 ? '' : 's'} from ${r.format}',
        if (r.duplicates > 0) '${r.duplicates} already in KeyRoam',
        if (r.skipped > 0) '${r.skipped} without a username or password',
      ];
      final tail = r.deleted ? ' The CSV file was deleted.' : ' Delete the CSV file: it is not encrypted.';
      Navigator.pop(context, '${parts.join(', ')}.$tail');
    } on KeyRoamException catch (e) {
      setState(() {
        _busy = false;
        _error = e.message;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Import passwords from CSV'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Export your passwords as a CSV file, then enter its path:\n'
              '• Chrome, Edge, Brave: Settings → Passwords (Password Manager) → Export passwords\n'
              '• Firefox: about:logins → ••• menu → Export Passwords\n'
              '• Safari: File → Export → Passwords\n'
              '• Bitwarden, LastPass, 1Password, KeePass: their CSV export\n\n'
              'Logins already in KeyRoam are skipped; nothing is overwritten.',
            ),
            const SizedBox(height: 12),
            TextField(
              key: const Key('csvPath'),
              controller: _path,
              decoration: const InputDecoration(
                labelText: 'CSV file',
                hintText: 'C:\\Users\\me\\Downloads\\passwords.csv',
              ),
              onChanged: (_) => setState(() {}),
            ),
            CheckboxListTile(
              key: const Key('csvDelete'),
              contentPadding: EdgeInsets.zero,
              value: _delete,
              onChanged: (v) => setState(() => _delete = v ?? false),
              title: const Text('Delete the CSV file after importing'),
              subtitle: const Text('It holds your passwords unencrypted.'),
            ),
            if (_error != null) Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.pop(context), child: const Text('Cancel')),
        FilledButton(
          key: const Key('csvImport'),
          onPressed: _busy || _path.text.trim().isEmpty ? null : _import,
          child: const Text('Import'),
        ),
      ],
    );
  }
}
