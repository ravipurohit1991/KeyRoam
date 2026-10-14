import Foundation
import LocalAuthentication
import Security

/// Face ID / Touch ID unlock.
///
/// The vault's key-encryption key (the Argon2 output of the master password,
/// from the engine's `biometric_key`) is kept in the keychain under an access
/// control that requires the currently enrolled biometrics
/// (`.biometryCurrentSet`) and a device passcode, and never leaves this
/// device. Adding or removing a fingerprint or face makes the item unusable.
/// A master-password change or vault-key rotation makes the stored key stop
/// working (`unlock_with_key` answers `wrong_password`); biometric unlock
/// then turns itself off, as on Android.
///
/// The item is in the App Group's keychain access group, so the AutoFill
/// extension can unlock with Face ID too. Without that entitlement (unsigned
/// builds) it falls back to the app's own keychain.
enum BiometricKey {
  private static let service = "io.github.ravipurohit1991.keyroam.kek"
  private static let account = "vault-kek"
  /// Vault id the stored key belongs to, so a different vault is not offered.
  private static let vaultKey = "biometricVaultId"

  /// A biometric is enrolled and usable.
  static func available() -> Bool {
    var error: NSError?
    return LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
  }

  /// Biometric unlock is on for `vaultId` (or for any vault, when nil).
  static func enabled(vaultId: String?) -> Bool {
    guard let stored = KeyRoamShared.defaults.string(forKey: vaultKey) else { return false }
    return vaultId == nil || vaultId == stored
  }

  static var biometryName: String {
    let context = LAContext()
    _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    switch context.biometryType {
    case .faceID: return "Face ID"
    case .touchID: return "Touch ID"
    default: return "biometrics"
    }
  }

  private static func baseQuery(group: Bool) -> [String: Any] {
    var q: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
    ]
    if group { q[kSecAttrAccessGroup as String] = KeyRoamShared.appGroup }
    return q
  }

  /// Turn biometric unlock on: check the master password, confirm with a
  /// biometric, then store the key. Blocks; call off the main thread.
  static func enable(engine: KeyRoamEngine, password: String) throws {
    let r = try engine.callObject("biometric_key", ["password": password])
    guard let b64 = r["key"] as? String, var key = Data(base64Encoded: b64), let vaultId = r["vault_id"] as? String
    else { throw KeyRoamError(code: "failed", message: "No key") }
    defer { key.resetBytes(in: 0..<key.count) }
    try confirm(reason: "Turn on \(biometryName) for KeyRoam")

    guard
      let access = SecAccessControlCreateWithFlags(
        nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, .biometryCurrentSet, nil)
    else { throw KeyRoamError(code: "failed", message: "Could not protect the key") }
    disable()
    var status = errSecMissingEntitlement
    for group in [true, false] {
      var q = baseQuery(group: group)
      q[kSecValueData as String] = key
      q[kSecAttrAccessControl as String] = access
      status = SecItemAdd(q as CFDictionary, nil)
      if status != errSecMissingEntitlement { break }
    }
    guard status == errSecSuccess else {
      throw KeyRoamError(code: "failed", message: "The keychain refused the key (\(status)).")
    }
    KeyRoamShared.defaults.set(vaultId, forKey: vaultKey)
  }

  static func disable() {
    for group in [true, false] {
      SecItemDelete(baseQuery(group: group) as CFDictionary)
    }
    KeyRoamShared.defaults.removeObject(forKey: vaultKey)
  }

  /// Ask for a biometric, then unlock the engine's vault with the stored key.
  /// Returns the engine's new `state`. Blocks; call off the main thread.
  @discardableResult
  static func unlock(engine: KeyRoamEngine, reason: String = "Unlock KeyRoam") throws -> [String: Any] {
    guard enabled(vaultId: nil) else {
      throw KeyRoamError(code: "not_enabled", message: "Biometric unlock is off.")
    }
    var key = try readKey(reason: reason)
    defer { key.resetBytes(in: 0..<key.count) }
    do {
      return try engine.callObject("unlock_with_key", ["key": key.base64EncodedString()])
    } catch let e as KeyRoamError where e.code == "wrong_password" {
      disable()
      throw KeyRoamError(
        code: "stale",
        message: "Your master password changed. Unlock with it, then turn on \(biometryName) again.")
    }
  }

  private static func readKey(reason: String) throws -> Data {
    let context = LAContext()
    context.localizedReason = reason
    context.localizedFallbackTitle = ""
    var status = errSecItemNotFound
    var result: CFTypeRef?
    for group in [true, false] {
      var q = baseQuery(group: group)
      q[kSecReturnData as String] = true
      q[kSecMatchLimit as String] = kSecMatchLimitOne
      q[kSecUseAuthenticationContext as String] = context
      status = SecItemCopyMatching(q as CFDictionary, &result)
      if status != errSecItemNotFound && status != errSecMissingEntitlement { break }
    }
    switch status {
    case errSecSuccess:
      guard let data = result as? Data else { throw KeyRoamError(code: "failed", message: "No key") }
      return data
    case errSecUserCanceled:
      throw KeyRoamError(code: "cancelled", message: "Cancelled")
    case errSecItemNotFound, errSecInvalidData:
      // Enrolled biometrics changed: the item is gone or unusable.
      disable()
      throw KeyRoamError(
        code: "invalidated",
        message: "Your \(biometryName) settings changed. Unlock with your master password, then turn it on again.")
    default:
      throw KeyRoamError(code: "failed", message: "\(biometryName) did not unlock KeyRoam (\(status)).")
    }
  }

  private static func confirm(reason: String) throws {
    let context = LAContext()
    context.localizedFallbackTitle = ""
    let done = DispatchSemaphore(value: 0)
    var failure: Error?
    context.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, localizedReason: reason) { ok, error in
      if !ok { failure = error ?? KeyRoamError(code: "failed", message: "Not confirmed") }
      done.signal()
    }
    done.wait()
    if let e = failure as? LAError, [.userCancel, .appCancel, .systemCancel].contains(e.code) {
      throw KeyRoamError(code: "cancelled", message: "Cancelled")
    }
    if let e = failure {
      throw KeyRoamError(code: "failed", message: e.localizedDescription)
    }
  }
}
