import 'package:flutter/material.dart';

import '../engine/api.dart';

/// Brand colours and theme.
ThemeData keyroamTheme(Brightness brightness) {
  final scheme = ColorScheme.fromSeed(seedColor: const Color(0xFF0F766E), brightness: brightness);
  return ThemeData(
    colorScheme: scheme,
    useMaterial3: true,
    inputDecorationTheme: const InputDecorationTheme(border: OutlineInputBorder()),
    visualDensity: VisualDensity.standard,
  );
}

/// Human-friendly message for engine errors.
String describeError(Object e) {
  if (e is KeyRoamException) {
    return switch (e.code) {
      'wrong_password' => 'Incorrect master password.',
      'offline' => 'You appear to be offline.',
      'not_configured' => 'Google sign-in is not configured for this build. Set KEYROAM_GOOGLE_CLIENT_ID (see docs/google-cloud-setup.md).',
      'cancelled' => 'Sign-in cancelled.',
      _ => e.message,
    };
  }
  return e.toString();
}

String timeAgo(DateTime t) {
  final d = DateTime.now().difference(t);
  if (d.inSeconds < 60) return 'just now';
  if (d.inMinutes < 60) return '${d.inMinutes} min ago';
  if (d.inHours < 24) return '${d.inHours} h ago';
  return '${d.inDays} d ago';
}

/// Password field with a visibility toggle.
class PasswordField extends StatefulWidget {
  const PasswordField({
    super.key,
    required this.controller,
    required this.label,
    this.autofocus = false,
    this.onSubmitted,
    this.validator,
    this.errorText,
    this.suffix,
  });

  final TextEditingController controller;
  final String label;
  final bool autofocus;
  final ValueChanged<String>? onSubmitted;
  final FormFieldValidator<String>? validator;
  final String? errorText;
  final Widget? suffix;

  @override
  State<PasswordField> createState() => _PasswordFieldState();
}

class _PasswordFieldState extends State<PasswordField> {
  bool _obscure = true;

  @override
  Widget build(BuildContext context) {
    return TextFormField(
      controller: widget.controller,
      obscureText: _obscure,
      autofocus: widget.autofocus,
      enableSuggestions: false,
      autocorrect: false,
      onFieldSubmitted: widget.onSubmitted,
      validator: widget.validator,
      decoration: InputDecoration(
        labelText: widget.label,
        errorText: widget.errorText,
        suffixIcon: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            ?widget.suffix,
            IconButton(
              tooltip: _obscure ? 'Show' : 'Hide',
              icon: Icon(_obscure ? Icons.visibility_outlined : Icons.visibility_off_outlined),
              onPressed: () => setState(() => _obscure = !_obscure),
            ),
          ],
        ),
      ),
    );
  }
}

/// Very rough strength estimate for UI feedback only (the engine enforces the
/// minimum length).
({double score, String label}) passwordStrength(String pw) {
  if (pw.isEmpty) return (score: 0, label: '');
  var classes = 0;
  if (RegExp(r'[a-z]').hasMatch(pw)) classes++;
  if (RegExp(r'[A-Z]').hasMatch(pw)) classes++;
  if (RegExp(r'[0-9]').hasMatch(pw)) classes++;
  if (RegExp(r'[^A-Za-z0-9]').hasMatch(pw)) classes++;
  final bits =
      pw.length *
      (classes <= 1
          ? 4
          : classes == 2
          ? 5
          : 6);
  if (pw.length < 12) return (score: 0.15, label: 'Too short');
  if (bits < 70) return (score: 0.4, label: 'Fair');
  if (bits < 100) return (score: 0.7, label: 'Good');
  return (score: 1, label: 'Strong');
}

/// Centered card layout used by onboarding and lock screens.
class CenteredCard extends StatelessWidget {
  const CenteredCard({super.key, required this.child, this.maxWidth = 460});
  final Widget child;
  final double maxWidth;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(24),
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: maxWidth),
          child: Card(
            child: Padding(padding: const EdgeInsets.all(32), child: child),
          ),
        ),
      ),
    );
  }
}

class BrandHeader extends StatelessWidget {
  const BrandHeader({super.key, this.subtitle});
  final String? subtitle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      children: [
        Icon(Icons.key_rounded, size: 48, color: theme.colorScheme.primary),
        const SizedBox(height: 8),
        Text('KeyRoam', style: theme.textTheme.headlineMedium?.copyWith(fontWeight: FontWeight.w600)),
        Text(subtitle ?? 'Your keys. Wherever you go.', style: theme.textTheme.bodyMedium),
        const SizedBox(height: 24),
      ],
    );
  }
}
