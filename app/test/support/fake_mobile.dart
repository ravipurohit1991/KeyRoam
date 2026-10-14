// Stand-in for the Android and iOS integrations (the platform channel),
// used by widget tests. Biometric unlock opens the fake engine's vault.

import 'package:keyroam/src/app/mobile_platform.dart';
import 'package:keyroam/src/engine/api.dart';
import 'package:keyroam/src/engine/models.dart';

import 'fake_api.dart';

class FakeMobilePlatform implements MobilePlatform {
  FakeMobilePlatform(this.api);

  final FakeKeyRoamApi api;
  bool available = true;
  bool enabled = false;
  bool autofillEnabled = false;
  bool ios = false;
  final calls = <String>[];
  void Function()? _vaultChanged;
  void Function()? _online;

  /// Simulate MainActivity reporting a vault change made outside the UI.
  void vaultChanged() => _vaultChanged?.call();

  /// Simulate connectivity returning.
  void online() => _online?.call();

  @override
  Future<BiometricStatus> biometricStatus() async => BiometricStatus(available: available, enabled: enabled);

  @override
  Future<void> enableBiometric(String password) async {
    calls.add('enableBiometric');
    if (password != api.password) throw const KeyRoamException('wrong_password', 'wrong');
    enabled = true;
  }

  @override
  Future<void> disableBiometric() async {
    calls.add('disableBiometric');
    enabled = false;
  }

  @override
  Future<AppState> biometricUnlock() async {
    calls.add('biometricUnlock');
    if (!enabled) throw const KeyRoamException('not_enabled', 'Biometric unlock is off.');
    await api.unlock(api.password);
    return api.state();
  }

  @override
  Future<void> scheduleBackgroundSync() async => calls.add('scheduleSync');

  @override
  Future<void> syncWhenOnline() async => calls.add('syncWhenOnline');

  @override
  Future<AutofillStatus> autofillStatus() async => AutofillStatus(
    autofillSupported: true,
    autofillEnabled: autofillEnabled,
    credentialProviderSupported: !ios,
    ios: ios,
  );

  @override
  Future<bool> openAutofillSettings() async {
    calls.add('openAutofillSettings');
    autofillEnabled = true;
    return true;
  }

  @override
  Future<bool> openCredentialSettings() async {
    calls.add('openCredentialSettings');
    return true;
  }

  @override
  Future<void> credentialsChanged() async => calls.add('credentialsChanged');

  @override
  void listen({required void Function() onVaultChanged, required void Function() onOnline}) {
    _vaultChanged = onVaultChanged;
    _online = onOnline;
  }
}
