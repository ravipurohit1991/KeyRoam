import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  private var systemObservers: [NSObjectProtocol] = []

  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    observeSystemLock(flutterViewController.engine.binaryMessenger)

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
}
