import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/platform_paths.dart';
import '../engine/models.dart';
import 'common.dart';
import 'dialogs.dart';
import 'onboarding.dart';

class Dashboard extends StatelessWidget {
  const Dashboard({super.key, required this.controller});
  final AppController controller;

  Future<void> _add(BuildContext context) async {
    final data = await showEntryEditor(context, controller);
    if (data != null) await controller.addEntry(data);
  }

  Future<void> _menu(BuildContext context, String action) async {
    String? message;
    switch (action) {
      case 'export':
        message = await showExportDialog(context, controller);
      case 'import':
        message = await showImportDialog(context, controller);
      case 'importCsv':
        message = await showCsvImportDialog(context, controller);
      case 'password':
        message = await showChangePasswordDialog(context, controller);
      case 'rotate':
        message = await showRotateKeyDialog(context, controller);
      case 'adoptKey':
        message = await showAdoptKeyDialog(context, controller);
      case 'deleted':
        await showDeletedDialog(context, controller);
      case 'browser':
        await showBrowserIntegrationDialog(context, controller);
      case 'biometric':
        message = await showBiometricDialog(context, controller);
      case 'autofill':
        await showAndroidAutofillDialog(context, controller);
      case 'signin':
        await showSignInDialog(
          context,
          controller,
          Builder(
            builder: (dialogContext) => GoogleSignInStep(
              api: controller.api,
              onSignedIn: (s) {
                Navigator.pop(dialogContext);
                controller.afterSignIn(s);
              },
              onBack: () => Navigator.pop(dialogContext),
            ),
          ),
        );
      case 'signout':
        await controller.signOut();
      case 'lock':
        await controller.lock();
    }
    if (message != null && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = controller.state!;
    return Scaffold(
      appBar: AppBar(
        bottom: controller.keyRotated
            ? PreferredSize(
                preferredSize: const Size.fromHeight(48),
                child: Material(
                  color: Theme.of(context).colorScheme.tertiaryContainer,
                  child: ListTile(
                    dense: true,
                    leading: const Icon(Icons.key),
                    title: const Text('The vault key was replaced on another device.'),
                    trailing: TextButton(
                      key: const Key('adoptKey'),
                      onPressed: () => _menu(context, 'adoptKey'),
                      child: const Text('Enter master password'),
                    ),
                  ),
                ),
              )
            : null,
        title: const Row(
          children: [
            Icon(Icons.key_rounded),
            SizedBox(width: 8),
            Flexible(child: Text('KeyRoam', overflow: TextOverflow.ellipsis)),
          ],
        ),
        actions: [
          SyncStatusChip(controller: controller),
          const SizedBox(width: 4),
          IconButton(
            key: const Key('syncNow'),
            tooltip: 'Sync now',
            onPressed: controller.syncing ? null : () => controller.syncNow(),
            icon: controller.syncing
                ? const SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2))
                : const Icon(Icons.sync),
          ),
          IconButton(tooltip: 'Lock', onPressed: controller.lock, icon: const Icon(Icons.lock_outline)),
          PopupMenuButton<String>(
            key: const Key('menu'),
            onSelected: (a) => _menu(context, a),
            itemBuilder: (_) => [
              const PopupMenuItem(value: 'deleted', child: Text('Recently deleted')),
              const PopupMenuItem(value: 'export', child: Text('Export encrypted backup…')),
              const PopupMenuItem(value: 'import', child: Text('Restore from backup…')),
              const PopupMenuItem(value: 'importCsv', child: Text('Import passwords from CSV…')),
              const PopupMenuItem(value: 'password', child: Text('Change master password…')),
              const PopupMenuItem(value: 'rotate', child: Text('Replace vault key…')),
              if (PlatformPaths.isDesktop) const PopupMenuItem(value: 'browser', child: Text('Browser integration…')),
              if (controller.mobile != null) ...[
                const PopupMenuItem(value: 'autofill', child: Text('Autofill…')),
                const PopupMenuItem(value: 'biometric', child: Text('Biometric unlock…')),
              ],
              const PopupMenuDivider(),
              if (state.signedIn)
                const PopupMenuItem(value: 'signout', child: Text('Sign out of Google on this device'))
              else
                const PopupMenuItem(value: 'signin', child: Text('Sign in to Google…')),
            ],
          ),
          const SizedBox(width: 8),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        key: const Key('addEntry'),
        onPressed: () => _add(context),
        icon: const Icon(Icons.add),
        label: const Text('New login'),
      ),
      body: LayoutBuilder(
        builder: (context, box) {
          final list = _EntryList(controller: controller);
          final detail = controller.selected == null
              ? const _EmptyDetail()
              : EntryDetailView(controller: controller, entry: controller.selected!);
          if (box.maxWidth < 760) {
            return controller.selected == null
                ? list
                : Column(
                    children: [
                      Align(
                        alignment: Alignment.centerLeft,
                        child: TextButton.icon(
                          onPressed: () => controller.select(null),
                          icon: const Icon(Icons.arrow_back),
                          label: const Text('All logins'),
                        ),
                      ),
                      Expanded(child: detail),
                    ],
                  );
          }
          return Row(
            children: [
              SizedBox(width: 340, child: list),
              const VerticalDivider(width: 1),
              Expanded(child: detail),
            ],
          );
        },
      ),
    );
  }
}

class SyncStatusChip extends StatelessWidget {
  const SyncStatusChip({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final s = controller.state!;
    final scheme = Theme.of(context).colorScheme;
    final last = s.sync.lastSuccessAt == null ? 'never' : timeAgo(s.sync.lastSuccessAt!);
    final (IconData icon, String label, Color color, String tip) = controller.syncing
        ? (Icons.sync, 'Syncing…', scheme.primary, 'Synchronising with Google Drive')
        : !s.signedIn
        ? (Icons.cloud_off, 'Not signed in', scheme.outline, 'Sign in to Google from the menu to sync')
        : switch (s.sync.status) {
            'up_to_date' => (Icons.cloud_done_outlined, 'Synced', scheme.primary, 'Last sync $last'),
            'pending' => (Icons.cloud_upload_outlined, 'Changes pending', scheme.tertiary, 'Last sync $last'),
            'offline' => (
              Icons.cloud_off,
              'Offline',
              scheme.tertiary,
              'Changes are saved on this device and will sync when you are back online. Last sync $last',
            ),
            'error' => (Icons.error_outline, 'Sync error', scheme.error, s.sync.message ?? 'Sync failed'),
            _ => (Icons.cloud_queue, 'Not synced yet', scheme.outline, 'Never synchronised'),
          };
    if (MediaQuery.sizeOf(context).width < 600) {
      // Phones: icon only; details in the tooltip.
      return Tooltip(
        message: '$label. $tip',
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: Icon(icon, key: const Key('syncStatus'), color: color),
        ),
      );
    }
    return Tooltip(
      message: tip,
      child: Chip(
        key: const Key('syncStatus'),
        avatar: Icon(icon, size: 18, color: color),
        label: Text(label),
        visualDensity: VisualDensity.compact,
      ),
    );
  }
}

class _EntryList extends StatefulWidget {
  const _EntryList({required this.controller});
  final AppController controller;

  @override
  State<_EntryList> createState() => _EntryListState();
}

class _EntryListState extends State<_EntryList> {
  final _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _search.text = widget.controller.query;
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final conflicts = c.state!.conflicts;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.all(12),
          child: TextField(
            key: const Key('search'),
            controller: _search,
            onChanged: c.search,
            decoration: const InputDecoration(prefixIcon: Icon(Icons.search), hintText: 'Search logins', isDense: true),
          ),
        ),
        if (conflicts > 0)
          MaterialBanner(
            content: Text(
              '$conflicts item${conflicts == 1 ? ' was' : 's were'} edited on two devices. '
              'Open ${conflicts == 1 ? 'it' : 'them'} to choose which version to keep.',
            ),
            leading: const Icon(Icons.merge_type),
            actions: const [SizedBox.shrink()],
          ),
        Expanded(
          child: c.entries.isEmpty
              ? Center(child: Text(c.query.isEmpty ? 'No logins yet' : 'No matches'))
              : ListView.builder(
                  itemCount: c.entries.length,
                  itemBuilder: (context, i) {
                    final e = c.entries[i];
                    return ListTile(
                      selected: e.id == c.selectedId,
                      leading: CircleAvatar(child: Text(e.title.isEmpty ? '?' : e.title[0].toUpperCase())),
                      title: Text(e.title, overflow: TextOverflow.ellipsis),
                      subtitle: Text(e.username.isEmpty ? e.url : e.username, overflow: TextOverflow.ellipsis),
                      trailing: e.conflict
                          ? Tooltip(
                              message: 'Edited on two devices',
                              child: Icon(Icons.merge_type, color: Theme.of(context).colorScheme.error),
                            )
                          : e.favorite
                          ? const Icon(Icons.star, size: 18)
                          : null,
                      onTap: () => c.select(e.id),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _EmptyDetail extends StatelessWidget {
  const _EmptyDetail();
  @override
  Widget build(BuildContext context) =>
      Center(child: Text('Select a login', style: Theme.of(context).textTheme.bodyLarge));
}

class EntryDetailView extends StatefulWidget {
  const EntryDetailView({super.key, required this.controller, required this.entry});
  final AppController controller;
  final EntryDetail entry;

  @override
  State<EntryDetailView> createState() => _EntryDetailViewState();
}

class _EntryDetailViewState extends State<EntryDetailView> {
  bool _reveal = false;

  @override
  void didUpdateWidget(EntryDetailView old) {
    super.didUpdateWidget(old);
    if (old.entry.id != widget.entry.id) _reveal = false;
  }

  Future<void> _edit(EntryData data) async {
    final updated = await showEntryEditor(context, widget.controller, initial: data);
    if (updated != null) await widget.controller.updateEntry(widget.entry.id, updated);
  }

  Future<void> _delete() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Delete this login?'),
        content: const Text('You can restore it later from "Recently deleted".'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('Cancel')),
          FilledButton(onPressed: () => Navigator.pop(context, true), child: const Text('Delete')),
        ],
      ),
    );
    if (ok == true) await widget.controller.deleteEntry(widget.entry.id);
  }

  @override
  Widget build(BuildContext context) {
    final e = widget.entry;
    final theme = Theme.of(context);
    if (e.conflict) return ConflictResolver(controller: widget.controller, entry: e);
    final d = e.data!;
    final c = widget.controller;
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Row(
          children: [
            Expanded(child: Text(d.title, style: theme.textTheme.headlineSmall)),
            IconButton(tooltip: 'Edit', onPressed: () => _edit(d), icon: const Icon(Icons.edit_outlined)),
            IconButton(tooltip: 'Delete', onPressed: _delete, icon: const Icon(Icons.delete_outline)),
          ],
        ),
        const SizedBox(height: 16),
        _Field(
          label: 'Username',
          value: d.username,
          onCopy: d.username.isEmpty ? null : () => c.copyPlain(d.username, label: 'Username copied'),
        ),
        _Field(
          label: 'Password',
          value: _reveal ? d.password : '•' * 12,
          monospace: _reveal,
          actions: [
            IconButton(
              key: const Key('revealPassword'),
              tooltip: _reveal ? 'Hide' : 'Show',
              icon: Icon(_reveal ? Icons.visibility_off_outlined : Icons.visibility_outlined),
              onPressed: () => setState(() => _reveal = !_reveal),
            ),
          ],
          onCopy: d.password.isEmpty ? null : () => c.copySecret(d.password, label: 'Password copied'),
        ),
        for (final u in d.urls) _Field(label: 'Website', value: u, onCopy: () => c.copyPlain(u)),
        if (d.tags.isNotEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Wrap(spacing: 6, children: [for (final t in d.tags) Chip(label: Text(t))]),
          ),
        if (d.notes.isNotEmpty) _Field(label: 'Notes', value: d.notes),
        const SizedBox(height: 8),
        Text('Modified ${timeAgo(e.versions.first.modifiedAt)}', style: theme.textTheme.bodySmall),
        if (e.history.isNotEmpty)
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: Text('Password history (${e.history.length})'),
            children: [
              for (final v in e.history)
                if (v.data != null)
                  ListTile(
                    dense: true,
                    title: Text(_reveal ? v.data!.password : '•' * 12),
                    subtitle: Text('${v.data!.title} · ${timeAgo(v.modifiedAt)}'),
                    trailing: IconButton(
                      tooltip: 'Copy',
                      icon: const Icon(Icons.copy, size: 18),
                      onPressed: () => c.copySecret(v.data!.password, label: 'Old password copied'),
                    ),
                  ),
            ],
          ),
      ],
    );
  }
}

class _Field extends StatelessWidget {
  const _Field({
    required this.label,
    required this.value,
    this.onCopy,
    this.actions = const [],
    this.monospace = false,
  });
  final String label;
  final String value;
  final VoidCallback? onCopy;
  final List<Widget> actions;
  final bool monospace;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label, style: theme.textTheme.labelMedium?.copyWith(color: theme.colorScheme.outline)),
                const SizedBox(height: 2),
                SelectableText(
                  value.isEmpty ? '—' : value,
                  style: monospace ? const TextStyle(fontFamily: 'monospace', fontSize: 15) : theme.textTheme.bodyLarge,
                ),
              ],
            ),
          ),
          ...actions,
          if (onCopy != null) IconButton(tooltip: 'Copy $label', icon: const Icon(Icons.copy), onPressed: onCopy),
        ],
      ),
    );
  }
}

/// Shows concurrent versions side by side and lets the user keep one.
class ConflictResolver extends StatefulWidget {
  const ConflictResolver({super.key, required this.controller, required this.entry});
  final AppController controller;
  final EntryDetail entry;

  @override
  State<ConflictResolver> createState() => _ConflictResolverState();
}

class _ConflictResolverState extends State<ConflictResolver> {
  bool _reveal = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final e = widget.entry;
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Row(
          children: [
            Icon(Icons.merge_type, color: theme.colorScheme.error),
            const SizedBox(width: 8),
            Expanded(child: Text('Edited on two devices', style: theme.textTheme.headlineSmall)),
            TextButton.icon(
              onPressed: () => setState(() => _reveal = !_reveal),
              icon: Icon(_reveal ? Icons.visibility_off_outlined : Icons.visibility_outlined),
              label: Text(_reveal ? 'Hide passwords' : 'Show passwords'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        const Text(
          'These changes were made while the devices could not see each other. Nothing was overwritten. '
          'Choose the version to keep; the others stay in the password history.',
        ),
        const SizedBox(height: 16),
        for (var i = 0; i < e.versions.length; i++)
          Card(
            key: Key('conflictVersion$i'),
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Version ${i + 1} · ${timeAgo(e.versions[i].modifiedAt)} · device ${e.versions[i].modifiedBy.substring(0, 8)}',
                    style: theme.textTheme.labelLarge,
                  ),
                  const SizedBox(height: 8),
                  if (e.versions[i].data == null)
                    const Text('Deleted on this device')
                  else ...[
                    Text('Title: ${e.versions[i].data!.title}'),
                    Text('Username: ${e.versions[i].data!.username}'),
                    Text('Password: ${_reveal ? e.versions[i].data!.password : '•' * 12}'),
                    if (e.versions[i].data!.urls.isNotEmpty) Text('Website: ${e.versions[i].data!.urls.join(', ')}'),
                  ],
                  const SizedBox(height: 8),
                  Align(
                    alignment: Alignment.centerRight,
                    child: FilledButton.tonal(
                      key: Key('keepVersion$i'),
                      onPressed: () => widget.controller.resolveConflict(e.id, i),
                      child: Text(e.versions[i].data == null ? 'Keep deleted' : 'Keep this version'),
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}
