import 'dart:convert';

import 'package:flutter/services.dart';

import '../engine/api.dart';
import '../engine/models.dart';

/// Whether this device can use biometric unlock, and whether it is on.
class BiometricStatus {
  const BiometricStatus({this.available = false, this.enabled = false});

  /// A strong biometric (fingerprint, face) is enrolled on the device.
  final bool available;

  /// KeyRoam keeps the vault's key-encryption key behind it.
  final bool enabled;
}

/// The system autofill integrations KeyRoam offers.
class AutofillStatus {
  const AutofillStatus({
    this.autofillSupported = false,
    this.autofillEnabled = false,
    this.credentialProviderSupported = false,
    this.ios = false,
  });

  final bool autofillSupported;

  /// KeyRoam is the selected autofill service (Android) or AutoFill
  /// provider (iOS).
  final bool autofillEnabled;

  /// Credential Manager providers exist (Android 14+).
  final bool credentialProviderSupported;

  /// iOS: AutoFill providers are chosen in the Settings app.
  final bool ios;
}

/// What the Android and iOS apps add to the shared UI: biometric unlock,
/// background sync, the system autofill settings, and notifications when the
/// vault changed outside the UI (autofill, background sync).
abstract class MobilePlatform {
  Future<BiometricStatus> biometricStatus();

  /// Throws [KeyRoamException] (`wrong_password`, `cancelled`, …).
  Future<void> enableBiometric(String password);
  Future<void> disableBiometric();

  /// Unlocks the vault after a biometric check and returns the new state.
  Future<AppState> biometricUnlock();

  Future<void> scheduleBackgroundSync();
  Future<void> syncWhenOnline();

  Future<AutofillStatus> autofillStatus();
  Future<bool> openAutofillSettings();
  Future<bool> openCredentialSettings();

  /// The vault's logins changed or it was unlocked: iOS refreshes its
  /// QuickType suggestions (usernames and sites, never passwords).
  Future<void> credentialsChanged();

  void listen({required void Function() onVaultChanged, required void Function() onOnline});
}

/// [MobilePlatform] over the `keyroam/platform` channel of Android's
/// `MainActivity` and iOS's `KeyRoamPlatformPlugin`.
class NativeMobilePlatform implements MobilePlatform {
  static const _channel = MethodChannel('keyroam/platform');

  Future<T?> _invoke<T>(String method, [Map<String, Object?>? args]) async {
    try {
      return await _channel.invokeMethod<T>(method, args);
    } on PlatformException catch (e) {
      throw KeyRoamException(e.code, e.message ?? e.code);
    }
  }

  @override
  Future<BiometricStatus> biometricStatus() async {
    final m = await _channel.invokeMapMethod<String, bool>('biometricStatus') ?? const {};
    return BiometricStatus(available: m['available'] ?? false, enabled: m['enabled'] ?? false);
  }

  @override
  Future<void> enableBiometric(String password) => _invoke<bool>('enableBiometric', {'password': password});

  @override
  Future<void> disableBiometric() => _invoke<void>('disableBiometric');

  @override
  Future<AppState> biometricUnlock() async {
    final json = await _invoke<String>('biometricUnlock');
    return AppState.fromJson(jsonDecode(json!) as Map<String, dynamic>);
  }

  @override
  Future<void> scheduleBackgroundSync() => _invoke<void>('scheduleSync');

  @override
  Future<void> syncWhenOnline() => _invoke<void>('syncWhenOnline');

  @override
  Future<AutofillStatus> autofillStatus() async {
    final m = await _channel.invokeMapMethod<String, bool>('autofillStatus') ?? const {};
    return AutofillStatus(
      autofillSupported: m['autofill_supported'] ?? false,
      autofillEnabled: m['autofill_enabled'] ?? false,
      credentialProviderSupported: m['credential_provider_supported'] ?? false,
      ios: m['ios'] ?? false,
    );
  }

  @override
  Future<bool> openAutofillSettings() async => await _invoke<bool>('openAutofillSettings') ?? false;

  @override
  Future<bool> openCredentialSettings() async => await _invoke<bool>('openCredentialSettings') ?? false;

  @override
  Future<void> credentialsChanged() => _invoke<void>('credentialsChanged');

  @override
  void listen({required void Function() onVaultChanged, required void Function() onOnline}) {
    _channel.setMethodCallHandler((call) async {
      switch (call.method) {
        case 'vaultChanged':
          onVaultChanged();
        case 'online':
          onOnline();
      }
    });
  }
}
