import Foundation

/// Settings shared by the app and the AutoFill extension.
enum KeyRoamShared {
  /// App Group holding the vault folder, so the AutoFill extension (a
  /// separate process) opens the same vault as the app.
  static let appGroup = "group.io.github.ravipurohit1991.keyroam"

  /// The vault folder in the App Group container, or nil when the build has
  /// no App Group entitlement (an unsigned build): the app then uses the
  /// engine's default folder and AutoFill cannot reach the vault.
  static func sharedDataDirectory() -> URL? {
    FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
      .appendingPathComponent("KeyRoam", isDirectory: true)
  }

  /// Small non-secret flags both processes read (biometric unlock on/off).
  static var defaults: UserDefaults {
    UserDefaults(suiteName: appGroup) ?? .standard
  }
}

/// An error answered by the engine (`code` as in the JSON API) or by the
/// platform code around it (`cancelled`, `invalidated`, …).
struct KeyRoamError: Error, LocalizedError {
  let code: String
  let message: String

  var errorDescription: String? { message }
}

/// The Rust engine (crates/keyroam-ffi) for Swift code.
///
/// The Flutter build hook bundles the engine as a framework in
/// `Runner.app/Frameworks`. Swift opens that same framework with `dlopen`:
/// in the app it is the image Dart already loaded, so Swift and the Flutter
/// UI share one process-wide engine and one unlocked vault; in the AutoFill
/// extension (`Runner.app/PlugIns/AutoFill.appex`) it is a separate engine
/// over the same App Group folder.
///
/// Calls block (Argon2, network): make them off the main thread.
final class KeyRoamEngine {
  typealias CallFn = @convention(c) (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
  typealias FreeFn = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void

  static let shared: KeyRoamEngine? = KeyRoamEngine()

  private let callFn: CallFn
  private let freeFn: FreeFn

  private init?() {
    for path in KeyRoamEngine.candidates() {
      guard let handle = dlopen(path, RTLD_NOW),
        let call = dlsym(handle, "keyroam_call"),
        let free = dlsym(handle, "keyroam_free_string")
      else { continue }
      callFn = unsafeBitCast(call, to: CallFn.self)
      freeFn = unsafeBitCast(free, to: FreeFn.self)
      return
    }
    return nil
  }

  /// Engine frameworks next to this executable or in the containing app.
  private static func candidates() -> [String] {
    var dirs: [URL] = []
    if let own = Bundle.main.privateFrameworksURL { dirs.append(own) }
    // Runner.app/PlugIns/AutoFill.appex → Runner.app/Frameworks
    let bundle = Bundle.main.bundleURL
    if bundle.pathExtension == "appex" {
      dirs.append(bundle.deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Frameworks"))
    }
    var out: [String] = []
    for dir in dirs {
      let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
      for name in names.sorted() where name.hasSuffix(".framework") && name.lowercased().contains("keyroam") {
        let binary = String(name.dropLast(".framework".count))
        out.append(dir.appendingPathComponent(name).appendingPathComponent(binary).path)
      }
    }
    return out
  }

  /// Call `method`; returns the `ok` value or throws `KeyRoamError`.
  @discardableResult
  func call(_ method: String, _ params: [String: Any] = [:]) throws -> Any? {
    let request = try JSONSerialization.data(withJSONObject: ["method": method, "params": params])
    var text = String(decoding: request, as: UTF8.self)
    defer { text.removeAll() }
    guard let out = text.withCString({ callFn($0) }) else {
      throw KeyRoamError(code: "internal", message: "The KeyRoam engine did not answer.")
    }
    let data = Data(bytes: out, count: strlen(out))
    freeFn(out)
    guard let response = try JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) as? [String: Any]
    else {
      throw KeyRoamError(code: "internal", message: "Malformed engine response.")
    }
    if let error = response["error"] as? [String: Any] {
      throw KeyRoamError(
        code: error["code"] as? String ?? "internal",
        message: error["message"] as? String ?? "KeyRoam error")
    }
    return response["ok"]
  }

  func callObject(_ method: String, _ params: [String: Any] = [:]) throws -> [String: Any] {
    try call(method, params) as? [String: Any] ?? [:]
  }

  /// The JSON text of the `ok` value, for handing an `AppState` to Dart.
  func callJSON(_ method: String, _ params: [String: Any] = [:]) throws -> String {
    let value = try call(method, params) ?? NSNull()
    let data = try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])
    return String(decoding: data, as: UTF8.self)
  }

  /// Open the shared vault folder unless this process already did.
  func ensureOpen() throws {
    let state = try callObject("state")
    if state["data_dir"] is String { return }
    var params: [String: Any] = [:]
    if let dir = KeyRoamShared.sharedDataDirectory() { params["data_dir"] = dir.path }
    try call("open", params)
  }
}
