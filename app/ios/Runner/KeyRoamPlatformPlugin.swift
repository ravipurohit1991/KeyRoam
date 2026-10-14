import AuthenticationServices
import BackgroundTasks
import Flutter
import Network
import UIKit

/// The `keyroam/platform` channel on iOS. It gives the Flutter UI what the
/// Rust engine cannot do by itself, with the same methods as Android's
/// `MainActivity`: the shared vault folder, Google sign-in in an
/// `ASWebAuthenticationSession`, Face ID unlock, background sync, the AutoFill
/// settings and QuickType suggestions. The `keyroam/system` channel asks the
/// UI to lock when the device locks.
final class KeyRoamPlatformPlugin: NSObject, FlutterPlugin, ASWebAuthenticationPresentationContextProviding {
  private static var instance: KeyRoamPlatformPlugin?

  private let channel: FlutterMethodChannel
  private let system: FlutterMethodChannel
  private let io = DispatchQueue(label: "keyroam-engine", qos: .userInitiated)
  private var session: ASWebAuthenticationSession?
  private var signInAttempt: UUID?
  private var monitor: NWPathMonitor?
  private var online = true
  private var observers: [NSObjectProtocol] = []

  static func register(with registrar: FlutterPluginRegistrar) {
    let plugin = KeyRoamPlatformPlugin(messenger: registrar.messenger())
    registrar.addMethodCallDelegate(plugin, channel: plugin.channel)
    instance = plugin
  }

  private init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "keyroam/platform", binaryMessenger: messenger)
    system = FlutterMethodChannel(name: "keyroam/system", binaryMessenger: messenger)
    super.init()
    // The device locked (with a passcode): lock the vault too.
    observers.append(
      NotificationCenter.default.addObserver(
        forName: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil, queue: .main
      ) { [weak self] _ in self?.system.invokeMethod("lock", arguments: nil) })
  }

  /// Background sync changed the vault; the UI reloads (if it is running).
  static func vaultChanged() {
    DispatchQueue.main.async { instance?.channel.invokeMethod("vaultChanged", arguments: nil) }
  }

  func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    switch call.method {
    case "paths":
      result(paths())
    case "openUrl":
      result(signIn(url: args["url"] as? String))
    case "closeSignIn":
      closeSignIn()
      result(nil)
    case "biometricStatus":
      background(result) { engine in
        let vaultId = (try? engine.callObject("state"))?["vault_id"] as? String
        return ["available": BiometricKey.available(), "enabled": BiometricKey.enabled(vaultId: vaultId)]
      }
    case "enableBiometric":
      let password = args["password"] as? String ?? ""
      background(result) { engine in
        try BiometricKey.enable(engine: engine, password: password)
        return true
      }
    case "disableBiometric":
      BiometricKey.disable()
      result(nil)
    case "biometricUnlock":
      background(result) { engine in
        try BiometricKey.unlock(engine: engine)
        return try engine.callJSON("state")
      }
    case "scheduleSync":
      SyncTask.schedule()
      result(nil)
    case "syncWhenOnline":
      watchConnectivity()
      result(nil)
    case "autofillStatus":
      ASCredentialIdentityStore.shared.getState { state in
        DispatchQueue.main.async {
          result([
            "autofill_supported": true,
            "autofill_enabled": state.isEnabled,
            "credential_provider_supported": false,
            "ios": true,
          ])
        }
      }
    case "openAutofillSettings":
      openAutofillSettings(result)
    case "openCredentialSettings":
      result(false)
    case "credentialsChanged":
      AutoFillIdentities.refresh(io: io)
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  /// Run `work` against the engine off the main thread and answer on it.
  private func background(_ result: @escaping FlutterResult, _ work: @escaping (KeyRoamEngine) throws -> Any?) {
    io.async {
      let answer: Any?
      do {
        guard let engine = KeyRoamEngine.shared else {
          throw KeyRoamError(code: "internal", message: "The KeyRoam engine is missing from the app.")
        }
        answer = try work(engine)
      } catch let e as KeyRoamError {
        answer = FlutterError(code: e.code, message: e.message, details: nil)
      } catch {
        answer = FlutterError(code: "failed", message: error.localizedDescription, details: nil)
      }
      DispatchQueue.main.async { result(answer) }
    }
  }

  // ----- storage -----

  /// The vault lives in the App Group container, which the AutoFill
  /// extension can read. A vault created by an earlier version in the app's
  /// own Application Support folder is moved there once. Backups go to
  /// Documents, which the Files app shows.
  private func paths() -> [String: String] {
    var out: [String: String] = [:]
    let fm = FileManager.default
    if let docs = fm.urls(for: .documentDirectory, in: .userDomainMask).first {
      out["backups"] = docs.path
    }
    guard let shared = KeyRoamShared.sharedDataDirectory() else { return out }
    if let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
      let legacy = support.appendingPathComponent("KeyRoam", isDirectory: true)
      if fm.fileExists(atPath: legacy.path) && !fm.fileExists(atPath: shared.path) {
        try? fm.createDirectory(at: shared.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? fm.moveItem(at: legacy, to: shared)
      }
    }
    try? fm.createDirectory(at: shared, withIntermediateDirectories: true)
    out["data"] = shared.path
    return out
  }

  // ----- Google sign-in -----

  /// Show Google's sign-in page in an `ASWebAuthenticationSession`. The
  /// engine is already waiting in `complete_sign_in`: the redirect to the
  /// mobile client's reversed-id scheme goes to `oauth_redirect`, which checks
  /// `state` and redeems the code with PKCE. Closing the sheet cancels.
  private func signIn(url: String?) -> Bool {
    guard let url, url.hasPrefix("https://"), let page = URL(string: url) else { return false }
    let redirect = URLComponents(url: page, resolvingAgainstBaseURL: false)?
      .queryItems?.first(where: { $0.name == "redirect_uri" })?.value ?? ""
    // `com.googleusercontent.apps.<id>:/oauth2redirect` → its scheme. A
    // loopback redirect is served by the engine itself; the sheet then only
    // needs closing (closeSignIn).
    let scheme = redirect.hasPrefix("http") ? nil : redirect.split(separator: ":").first.map(String.init)
    closeSignIn()
    let io = self.io
    let attempt = UUID()
    signInAttempt = attempt
    let s = ASWebAuthenticationSession(url: page, callbackURLScheme: scheme) { [weak self] callback, error in
      DispatchQueue.main.async {
        // Not current: the app closed the sheet itself (closeSignIn).
        let current = self?.signInAttempt == attempt
        if current {
          self?.signInAttempt = nil
          self?.session = nil
        }
        let userClosed = (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin
        io.async {
          if let callback {
            _ = try? KeyRoamEngine.shared?.call("oauth_redirect", ["url": callback.absoluteString])
          } else if current && userClosed {
            _ = try? KeyRoamEngine.shared?.call("cancel_sign_in")
          }
        }
      }
    }
    s.presentationContextProvider = self
    // A private session: Google's cookies are not shared with Safari.
    s.prefersEphemeralWebBrowserSession = true
    session = s
    return s.start()
  }

  /// Dismiss the sheet without cancelling the engine's sign-in (it finished,
  /// or a new one starts).
  private func closeSignIn() {
    let s = session
    signInAttempt = nil
    session = nil
    s?.cancel()
  }

  func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    return scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first ?? ASPresentationAnchor()
  }

  // ----- connectivity -----

  /// Tell the UI when the network comes back, so it syncs at once.
  private func watchConnectivity() {
    if monitor != nil { return }
    let m = NWPathMonitor()
    m.pathUpdateHandler = { [weak self] path in
      let up = path.status == .satisfied
      DispatchQueue.main.async {
        guard let self else { return }
        if up && !self.online { self.channel.invokeMethod("online", arguments: nil) }
        self.online = up
      }
    }
    m.start(queue: DispatchQueue(label: "keyroam-network"))
    monitor = m
  }

  // ----- AutoFill -----

  private func openAutofillSettings(_ result: @escaping FlutterResult) {
    if #available(iOS 17.0, *) {
      ASSettingsHelper.openCredentialProviderAppSettings { error in
        DispatchQueue.main.async { result(error == nil) }
      }
      return
    }
    guard let url = URL(string: UIApplication.openSettingsURLString) else { return result(false) }
    UIApplication.shared.open(url) { ok in result(ok) }
  }
}

/// QuickType suggestions: the entry id, username and domain of each login,
/// given to `ASCredentialIdentityStore` while KeyRoam is the AutoFill
/// provider. They hold no passwords; picking one asks the extension, which
/// unlocks and re-checks the site.
enum AutoFillIdentities {
  static func refresh(io: DispatchQueue) {
    let store = ASCredentialIdentityStore.shared
    store.getState { state in
      guard state.isEnabled else { return }
      io.async {
        guard let list = try? KeyRoamEngine.shared?.call("ios_identities") as? [[String: Any]] else { return }
        let identities = list.compactMap { item -> ASPasswordCredentialIdentity? in
          guard let id = item["id"] as? String, let domain = item["domain"] as? String else { return nil }
          return ASPasswordCredentialIdentity(
            serviceIdentifier: ASCredentialServiceIdentifier(identifier: domain, type: .domain),
            user: item["user"] as? String ?? "",
            recordIdentifier: id)
        }
        store.replaceCredentialIdentities(with: identities) { _, _ in }
      }
    }
  }
}

/// Background sync with `BGAppRefreshTask`. iOS decides when it runs (at
/// most about every 15 minutes, more rarely for little-used apps). Sync needs
/// the vault key, so it does something only while the vault is unlocked in
/// a suspended app, as on Android.
enum SyncTask {
  static let identifier = "io.github.ravipurohit1991.keyroam.sync"

  /// Called from `application(_:didFinishLaunchingWithOptions:)`.
  static func register() {
    BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
      guard let task = task as? BGAppRefreshTask else { return task.setTaskCompleted(success: false) }
      run(task)
    }
  }

  static func schedule() {
    let request = BGAppRefreshTaskRequest(identifier: identifier)
    request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
    try? BGTaskScheduler.shared.submit(request)
  }

  private static func run(_ task: BGAppRefreshTask) {
    schedule()
    let work = DispatchWorkItem {
      let r = try? KeyRoamEngine.shared?.callObject("sync_if_unlocked")
      let changed = (r?["report"] as? [String: Any])?["local_changed"] as? Bool ?? false
      if changed {
        KeyRoamPlatformPlugin.vaultChanged()
        AutoFillIdentities.refresh(io: DispatchQueue.global(qos: .utility))
      }
      task.setTaskCompleted(success: r != nil)
    }
    task.expirationHandler = { work.cancel() }
    DispatchQueue.global(qos: .utility).async(execute: work)
  }
}
