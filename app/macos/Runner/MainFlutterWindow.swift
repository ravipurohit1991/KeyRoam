import Cocoa
import FlutterMacOS
import UniformTypeIdentifiers

class MainFlutterWindow: NSWindow {
  private var systemObservers: [NSObjectProtocol] = []

  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    observeSystemLock(flutterViewController.engine.binaryMessenger)
    serveFileDialogs(flutterViewController.engine.binaryMessenger)

    super.awakeFromNib()
  }

  /// Tell the app to lock the vault when the screen locks, the Mac sleeps or
  /// the user switches away.
  private func observeSystemLock(_ messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "keyroam/system", binaryMessenger: messenger)
    let lock: (Notification) -> Void = { _ in channel.invokeMethod("lock", arguments: nil) }
    systemObservers.append(
      DistributedNotificationCenter.default().addObserver(
        forName: NSNotification.Name("com.apple.screenIsLocked"), object: nil, queue: .main,
        using: lock))
    let workspace = NSWorkspace.shared.notificationCenter
    for name in [NSWorkspace.willSleepNotification, NSWorkspace.sessionDidResignActiveNotification] {
      systemObservers.append(workspace.addObserver(forName: name, object: nil, queue: .main, using: lock))
    }
  }

  /// Native open and save panels for backups and CSV imports. Under the App
  /// Sandbox, the file the user picks becomes readable and writable by the
  /// app (`com.apple.security.files.user-selected.read-write`).
  private func serveFileDialogs(_ messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "keyroam/files", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self, call.method == "save" || call.method == "open" else {
        return result(FlutterMethodNotImplemented)
      }
      let args = call.arguments as? [String: Any] ?? [:]
      let panel: NSSavePanel
      if call.method == "save" {
        panel = NSSavePanel()
        panel.nameFieldStringValue = args["suggestedName"] as? String ?? ""
        panel.canCreateDirectories = true
      } else {
        let open = NSOpenPanel()
        open.canChooseFiles = true
        open.canChooseDirectories = false
        open.allowsMultipleSelection = false
        panel = open
      }
      panel.title = args["title"] as? String ?? ""
      if let dir = args["initialDirectory"] as? String, !dir.isEmpty {
        panel.directoryURL = URL(fileURLWithPath: dir, isDirectory: true)
      }
      let types = (args["extensions"] as? [String] ?? []).compactMap { UTType(filenameExtension: $0) }
      if !types.isEmpty {
        panel.allowedContentTypes = types
        panel.allowsOtherFileTypes = call.method == "open"
      }
      panel.beginSheetModal(for: self) { response in
        result(response == .OK ? panel.url?.path : nil)
      }
    }
  }
}
