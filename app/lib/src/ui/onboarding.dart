import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../app/controller.dart';
import '../app/platform_paths.dart';
import '../engine/api.dart';
import '../engine/models.dart';
import 'common.dart';

enum _Step {
  welcome,
  // New user: password → Google → create + sync.
  newPassword,
  newSignIn,
  creating,
  // Existing user: Google → download → password → unlock.
  existingSignIn,
  downloading,
  existingPassword,
  joining,
}

class OnboardingFlow extends StatefulWidget {
  const OnboardingFlow({super.key, required this.controller});
  final AppController controller;

  @override
  State<OnboardingFlow> createState() => _OnboardingFlowState();
}

class _OnboardingFlowState extends State<OnboardingFlow> {
  _Step _step = _Step.welcome;
  String? _error;
  bool _signedIn = false;
  final _password = TextEditingController();
  final _confirm = TextEditingController();

  KeyRoamApi get _api => widget.controller.api;

  @override
  void dispose() {
    _password.dispose();
    _confirm.dispose();
    super.dispose();
  }

  void _go(_Step step, {String? error}) => setState(() {
    _step = step;
    _error = error;
  });

  Future<void> _create() async {
    _go(_Step.creating);
    try {
      final result = await _api.createVault(_password.text);
      _password.clear();
      _confirm.clear();
      await widget.controller.completeOnboarding(result);
    } on KeyRoamException catch (e) {
      if (!mounted) return;
      _go(_Step.creating, error: e.code == 'remote_vault_exists' ? 'remote_vault_exists' : describeError(e));
    }
  }

  Future<void> _download() async {
    _go(_Step.downloading);
    try {
      await _api.fetchRemoteVault();
      if (mounted) _go(_Step.existingPassword);
    } on KeyRoamException catch (e) {
      if (!mounted) return;
      _go(_Step.downloading, error: e.code == 'no_remote_vault' ? 'no_remote_vault' : describeError(e));
    }
  }

  Future<void> _join() async {
    _go(_Step.joining);
    try {
      final result = await _api.joinVault(_password.text);
      _password.clear();
      await widget.controller.completeOnboarding(result);
    } on KeyRoamException catch (e) {
      if (!mounted) return;
      _go(_Step.existingPassword, error: describeError(e));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: CenteredCard(
        maxWidth: 520,
        child: AnimatedSize(
          duration: const Duration(milliseconds: 200),
          child: switch (_step) {
            _Step.welcome => _welcome(context),
            _Step.newPassword => _newPassword(context),
            _Step.newSignIn => GoogleSignInStep(
              api: _api,
              stepLabel: 'Step 2 of 3',
              onSignedIn: (_) {
                _signedIn = true;
                _create();
              },
              onBack: () => _go(_Step.newPassword),
            ),
            _Step.creating => _progress(
              context,
              'Step 3 of 3',
              'Creating your encrypted vault and synchronising it to Google Drive…',
            ),
            _Step.existingSignIn => GoogleSignInStep(
              api: _api,
              stepLabel: 'Step 1 of 3',
              onSignedIn: (_) {
                _signedIn = true;
                _download();
              },
              onBack: () => _go(_Step.welcome),
            ),
            _Step.downloading => _progress(context, 'Step 2 of 3', 'Downloading your encrypted vault…'),
            _Step.existingPassword => _existingPassword(context),
            _Step.joining => _progress(context, 'Step 3 of 3', 'Unlocking and synchronising your vault…'),
          },
        ),
      ),
    );
  }

  Widget _welcome(BuildContext context) {
    return Column(
      key: const ValueKey('welcome'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const BrandHeader(subtitle: 'Welcome to KeyRoam'),
        Text(
          'One encrypted vault on all your devices, synchronised through your own Google Drive. '
          'Everything is encrypted on this device before it leaves it.',
          textAlign: TextAlign.center,
          style: Theme.of(context).textTheme.bodyLarge,
        ),
        const SizedBox(height: 28),
        FilledButton(
          onPressed: () => _go(_Step.newPassword),
          child: const Padding(padding: EdgeInsets.all(12), child: Text("I'm new to KeyRoam")),
        ),
        const SizedBox(height: 12),
        OutlinedButton(
          onPressed: () => _go(_Step.existingSignIn),
          child: const Padding(padding: EdgeInsets.all(12), child: Text('I already use KeyRoam on another device')),
        ),
      ],
    );
  }

  Widget _newPassword(BuildContext context) {
    return ListenableBuilder(
      key: const ValueKey('newPassword'),
      listenable: Listenable.merge([_password, _confirm]),
      builder: (context, _) {
        final strength = passwordStrength(_password.text);
        final mismatch = _confirm.text.isNotEmpty && _confirm.text != _password.text;
        final canContinue = _password.text.length >= 12 && _confirm.text == _password.text;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('Step 1 of 3', style: Theme.of(context).textTheme.labelLarge),
            const SizedBox(height: 4),
            Text('Create your master password', style: Theme.of(context).textTheme.headlineSmall),
            const SizedBox(height: 8),
            const Text(
              'It encrypts your vault and is never stored or sent anywhere. '
              'If you forget it, your data cannot be recovered. Use at least 12 characters; '
              'a few unrelated words make a strong, memorable password.',
            ),
            const SizedBox(height: 20),
            PasswordField(
              key: const Key('masterPassword'),
              controller: _password,
              label: 'Master password',
              autofocus: true,
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                Expanded(child: LinearProgressIndicator(value: strength.score)),
                const SizedBox(width: 12),
                SizedBox(width: 70, child: Text(strength.label)),
              ],
            ),
            const SizedBox(height: 16),
            PasswordField(
              key: const Key('confirmPassword'),
              controller: _confirm,
              label: 'Confirm master password',
              errorText: mismatch ? 'Passwords do not match' : _error,
              onSubmitted: (_) => canContinue ? _validateAndContinue() : null,
            ),
            const SizedBox(height: 24),
            Row(
              children: [
                TextButton(onPressed: () => _go(_Step.welcome), child: const Text('Back')),
                const Spacer(),
                FilledButton(
                  key: const Key('continue'),
                  onPressed: canContinue ? _validateAndContinue : null,
                  child: const Text('Continue'),
                ),
              ],
            ),
          ],
        );
      },
    );
  }

  Future<void> _validateAndContinue() async {
    try {
      await _api.checkPassword(_password.text);
    } on KeyRoamException catch (e) {
      setState(() => _error = e.message);
      return;
    }
    if (_signedIn) {
      _create();
    } else {
      _go(_Step.newSignIn);
    }
  }

  Widget _existingPassword(BuildContext context) {
    return Column(
      key: const ValueKey('existingPassword'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('Step 3 of 3', style: Theme.of(context).textTheme.labelLarge),
        const SizedBox(height: 4),
        Text('Enter your master password', style: Theme.of(context).textTheme.headlineSmall),
        const SizedBox(height: 8),
        const Text('Your encrypted vault was found. Enter the master password you use on your other devices.'),
        const SizedBox(height: 20),
        PasswordField(
          key: const Key('joinPassword'),
          controller: _password,
          label: 'Master password',
          autofocus: true,
          errorText: _error,
          onSubmitted: (_) => _join(),
        ),
        const SizedBox(height: 24),
        Align(
          alignment: Alignment.centerRight,
          child: FilledButton(onPressed: _join, child: const Text('Unlock')),
        ),
      ],
    );
  }

  Widget _progress(BuildContext context, String step, String message) {
    final error = _error;
    return Column(
      key: ValueKey(message),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(step, style: Theme.of(context).textTheme.labelLarge),
        const SizedBox(height: 16),
        if (error == null) ...[
          const Center(child: CircularProgressIndicator()),
          const SizedBox(height: 16),
          Text(message, textAlign: TextAlign.center),
        ] else if (error == 'remote_vault_exists') ...[
          const Text(
            'This Google account already has a KeyRoam vault. To use it on this device, '
            'continue with your existing master password instead of creating a new vault.',
          ),
          const SizedBox(height: 20),
          FilledButton(
            onPressed: () {
              _password.clear();
              _confirm.clear();
              _download();
            },
            child: const Text('Use my existing vault'),
          ),
        ] else if (error == 'no_remote_vault') ...[
          const Text('No KeyRoam vault was found in this Google account.'),
          const SizedBox(height: 20),
          FilledButton(onPressed: () => _go(_Step.newPassword), child: const Text('Create a new vault')),
          TextButton(onPressed: _download, child: const Text('Try again')),
        ] else ...[
          Text(error, style: TextStyle(color: Theme.of(context).colorScheme.error)),
          const SizedBox(height: 20),
          FilledButton(onPressed: _step == _Step.creating ? _create : _download, child: const Text('Try again')),
        ],
      ],
    );
  }
}

/// Google sign-in using the system browser and a loopback redirect.
class GoogleSignInStep extends StatefulWidget {
  const GoogleSignInStep({super.key, required this.api, required this.onSignedIn, this.stepLabel, this.onBack});

  final KeyRoamApi api;
  final ValueChanged<AppState> onSignedIn;
  final String? stepLabel;
  final VoidCallback? onBack;

  @override
  State<GoogleSignInStep> createState() => _GoogleSignInStepState();
}

class _GoogleSignInStepState extends State<GoogleSignInStep> {
  SignInStart? _pending;
  String? _error;

  Future<void> _start() async {
    setState(() => _error = null);
    try {
      var start = await widget.api.beginSignIn();
      if (!start.browserOpened && await PlatformPaths.openUrl(start.url)) {
        start = SignInStart(start.url, true);
      }
      if (!mounted) return;
      setState(() => _pending = start);
      final state = await widget.api.completeSignIn();
      if (!mounted) return;
      widget.onSignedIn(state);
    } on KeyRoamException catch (e) {
      if (!mounted) return;
      setState(() {
        _pending = null;
        _error = e.code == 'cancelled' ? null : describeError(e);
      });
    }
  }

  Future<void> _cancel() => widget.api.cancelSignIn();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final pending = _pending;
    return Column(
      key: const ValueKey('signIn'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.stepLabel != null) Text(widget.stepLabel!, style: theme.textTheme.labelLarge),
        const SizedBox(height: 4),
        Text('Sign in with Google', style: theme.textTheme.headlineSmall),
        const SizedBox(height: 8),
        const Text(
          'KeyRoam stores your encrypted vault in a private, hidden app folder in your Google Drive. '
          'It asks only for access to that folder. It cannot see your other files.',
        ),
        const SizedBox(height: 24),
        if (pending == null) ...[
          FilledButton.icon(
            key: const Key('googleSignIn'),
            onPressed: _start,
            icon: const Icon(Icons.login),
            label: const Padding(padding: EdgeInsets.all(10), child: Text('Sign in with Google')),
          ),
          if (_error != null) ...[
            const SizedBox(height: 12),
            Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
          ],
          if (widget.onBack != null) ...[
            const SizedBox(height: 8),
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton(onPressed: widget.onBack, child: const Text('Back')),
            ),
          ],
        ] else ...[
          const LinearProgressIndicator(),
          const SizedBox(height: 16),
          Text(
            pending.browserOpened
                ? 'Finish signing in in your browser, then return here.'
                : 'Open this link in your browser to sign in:',
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(child: SelectableText(pending.url, maxLines: 2, style: theme.textTheme.bodySmall)),
              IconButton(
                tooltip: 'Copy link',
                icon: const Icon(Icons.copy),
                onPressed: () => Clipboard.setData(ClipboardData(text: pending.url)),
              ),
            ],
          ),
          const SizedBox(height: 16),
          Align(
            alignment: Alignment.centerRight,
            child: TextButton(onPressed: _cancel, child: const Text('Cancel')),
          ),
        ],
      ],
    );
  }
}
