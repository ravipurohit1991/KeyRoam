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
                      trailing: TextButton(
                        onPressed: () async {
                          await c.restoreEntry(e.id);
                          if (context.mounted) Navigator.pop(context);
                        },
                        child: const Text('Restore'),
                      ),
                    ),
                ],
              ),
      ),
      actions: [TextButton(onPressed: () => Navigator.pop(context), child: const Text('Close'))],
    ),
  );
}

Future<void> showSignInDialog(BuildContext context, AppController c, Widget signInStep) {
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => AlertDialog(content: SizedBox(width: 480, child: signInStep)),
  );
}

/// Browser autofill: shows whether the browsers know about KeyRoam's
/// native-messaging host and registers it on request.
Future<void> showAutofillDialog(BuildContext context, AppController c) async {
  await c.refreshAutofillStatus();
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (_) => _AutofillDialog(controller: c),
  );
}

class _AutofillDialog extends StatefulWidget {
  const _AutofillDialog({required this.controller});
  final AppController controller;

  @override
  State<_AutofillDialog> createState() => _AutofillDialogState();
}

class _AutofillDialogState extends State<_AutofillDialog> {
  String? _error;
  bool _busy = false;

  Future<void> _install() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.controller.installBrowserIntegration();
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final s = c.autofill;
    final scheme = Theme.of(context).colorScheme;
    return AlertDialog(
      title: const Text('Browser autofill'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const Text(
              'The KeyRoam browser extension fills and saves logins for the site you are on. '
              'It talks only to this app, and only while KeyRoam is unlocked. Passwords are never '
              'filled on insecure (HTTP) pages or into another site\'s embedded form without asking you first.',
            ),
            const SizedBox(height: 16),
            if (s == null || !s.running)
              Text(
                'Browser integration is not running${c.autofillError == null ? '' : ': ${c.autofillError}'}',
                style: TextStyle(color: scheme.error),
              )
            else ...[
              for (final b in s.browsers)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(
                    b.installed ? Icons.check_circle : Icons.radio_button_unchecked,
                    color: b.installed ? scheme.primary : scheme.outline,
                  ),
                  title: Text(b.displayName),
                  subtitle: Text(b.installed ? 'Connected to KeyRoam' : 'Not set up'),
                ),
              if (!s.hostFound)
                Text(
                  'This build does not include the browser helper (keyroam-native-host). '
                  'Expected at: ${s.hostPath ?? 'unknown'}',
                  style: TextStyle(color: scheme.error),
                ),
              const SizedBox(height: 8),
              Text(
                'After setting up, install the KeyRoam extension in your browser and restart the browser.',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
            if (_error != null) ...[
              const SizedBox(height: 12),
              SelectableText(_error!, style: TextStyle(color: scheme.error)),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Close')),
        if (s != null && s.running)
          FilledButton(
            key: const Key('setUpBrowsers'),
            onPressed: _busy || !s.hostFound ? null : _install,
            child: _busy
                ? const SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2))
                : Text(s.anyBrowserRegistered ? 'Set up again' : 'Set up browsers'),
          ),
      ],
    );
  }
}
