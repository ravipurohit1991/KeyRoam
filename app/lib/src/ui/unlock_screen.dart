import 'package:flutter/material.dart';

import '../app/controller.dart';
import 'common.dart';

class UnlockScreen extends StatefulWidget {
  const UnlockScreen({super.key, required this.controller});
  final AppController controller;

  @override
  State<UnlockScreen> createState() => _UnlockScreenState();
}

class _UnlockScreenState extends State<UnlockScreen> {
  final _password = TextEditingController();
  String? _error;
  bool _busy = false;

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  Future<void> _unlock() async {
    if (_busy || _password.text.isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.controller.unlock(_password.text);
      _password.clear();
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: CenteredCard(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const BrandHeader(subtitle: 'Your vault is locked'),
            PasswordField(
              key: const Key('unlockPassword'),
              controller: _password,
              label: 'Master password',
              autofocus: true,
              errorText: _error,
              onSubmitted: (_) => _unlock(),
            ),
            const SizedBox(height: 20),
            FilledButton(
              key: const Key('unlock'),
              onPressed: _busy ? null : _unlock,
              child: Padding(
                padding: const EdgeInsets.all(10),
                child: _busy
                    ? const SizedBox(width: 20, height: 20, child: CircularProgressIndicator(strokeWidth: 2))
                    : const Text('Unlock'),
              ),
            ),
            const SizedBox(height: 16),
            Text(
              'Your master password cannot be recovered. If you have forgotten it, your vault can only be '
              'restored from an encrypted backup using that backup\'s password.',
              style: Theme.of(context).textTheme.bodySmall,
              textAlign: TextAlign.center,
            ),
          ],
        ),
      ),
    );
  }
}
