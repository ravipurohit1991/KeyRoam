import AuthenticationServices
import UIKit

/// KeyRoam's AutoFill Credential Provider.
///
/// iOS runs it in its own process, which opens the vault in the App Group
/// folder with its own copy of the Rust engine. It starts locked: the user
/// unlocks with Face ID (the key the app stored in the shared keychain) or
/// the master password, picks one of the logins saved for the site or app
/// (`ios_logins`), and gets only that login (`ios_credential`, re-checked
/// against the request). The vault is locked again as soon as the request
/// ends. Nothing is filled without the user's choice.
final class CredentialProviderViewController: ASCredentialProviderViewController, UITableViewDataSource,
  UITableViewDelegate, UITextFieldDelegate
{
  private enum Mode {
    case list
    /// A QuickType suggestion: the entry id it names.
    case single(String)
    case configuration
  }

  private var mode = Mode.list
  private var services: [[String: String]] = []
  private var site = ""
  private var logins: [[String: Any]] = []
  private let io = DispatchQueue(label: "keyroam-autofill", qos: .userInitiated)

  private let header = UILabel()
  private let detail = UILabel()
  private let password = UITextField()
  private let unlockButton = UIButton(type: .system)
  private let biometricButton = UIButton(type: .system)
  private let message = UILabel()
  private let spinner = UIActivityIndicatorView(style: .medium)
  private let unlockForm = UIStackView()
  private let doneButton = UIButton(type: .system)
  private let table = UITableView(frame: .zero, style: .insetGrouped)

  // ----- requests from iOS -----

  override func prepareCredentialList(for serviceIdentifiers: [ASCredentialServiceIdentifier]) {
    mode = .list
    services = serviceIdentifiers.map(Self.describe)
    begin()
  }

  override func provideCredentialWithoutUserInteraction(for credentialIdentity: ASPasswordCredentialIdentity) {
    // Every request starts locked and KeyRoam never keeps the vault unlocked
    // between requests, so a QuickType pick always needs the unlock UI.
    extensionContext.cancelRequest(
      withError: NSError(domain: ASExtensionErrorDomain, code: ASExtensionError.userInteractionRequired.rawValue))
  }

  override func prepareInterfaceToProvideCredential(for credentialIdentity: ASPasswordCredentialIdentity) {
    mode = .single(credentialIdentity.recordIdentifier ?? "")
    services = [Self.describe(credentialIdentity.serviceIdentifier)]
    begin()
  }

  override func prepareInterfaceForExtensionConfiguration() {
    mode = .configuration
    loadViewIfNeeded()
    header.text = "KeyRoam AutoFill is on"
    detail.text =
      "Open KeyRoam and unlock it once: your saved logins then appear as suggestions above the keyboard. "
      + "KeyRoam asks for Face ID or your master password before it fills anything."
    unlockForm.isHidden = true
    table.isHidden = true
    doneButton.isHidden = false
  }

  private static func describe(_ s: ASCredentialServiceIdentifier) -> [String: String] {
    ["identifier": s.identifier, "type": s.type == .URL ? "url" : "domain"]
  }

  // ----- flow -----

  private func begin() {
    loadViewIfNeeded()
    header.text = "KeyRoam"
    detail.text = nil
    unlockForm.isHidden = true
    table.isHidden = true
    setBusy(true)
    io.async { [self] in
      do {
        let engine = try loadEngine()
        try engine.ensureOpen()
        let state = try engine.callObject("state")
        guard state["has_vault"] as? Bool == true else {
          throw KeyRoamError(code: "no_vault", message: "Open KeyRoam and set up your vault first.")
        }
        let r = try engine.callObject("ios_logins", ["services": services])
        let site = r["label"] as? String ?? ""
        if state["unlocked"] as? Bool == true {
          return afterUnlock(engine)
        }
        let biometric = BiometricKey.enabled(vaultId: state["vault_id"] as? String)
        DispatchQueue.main.async { [self] in
          self.site = site
          showUnlock(biometric: biometric)
          if biometric { unlockWithBiometrics() } else { password.becomeFirstResponder() }
        }
      } catch {
        show(error)
      }
    }
  }

  private func loadEngine() throws -> KeyRoamEngine {
    guard let engine = KeyRoamEngine.shared else {
      throw KeyRoamError(code: "internal", message: "KeyRoam could not load its engine. Reinstall KeyRoam.")
    }
    return engine
  }

  private func showUnlock(biometric: Bool) {
    setBusy(false)
    header.text = "Unlock KeyRoam"
    detail.text = site.isEmpty ? nil : "To fill your login for \(site)."
    unlockForm.isHidden = false
    biometricButton.isHidden = !biometric
    biometricButton.setTitle("Use \(BiometricKey.biometryName)", for: .normal)
  }

  @objc private func unlockWithPassword() {
    let pw = password.text ?? ""
    guard !pw.isEmpty else { return }
    password.text = ""
    setBusy(true)
    io.async { [self] in
      do {
        let engine = try loadEngine()
        try engine.call("unlock", ["password": pw])
        afterUnlock(engine)
      } catch let e as KeyRoamError where e.code == "wrong_password" {
        DispatchQueue.main.async { [self] in
          setBusy(false)
          message.text = "Wrong master password."
          password.becomeFirstResponder()
        }
      } catch {
        show(error, keepUnlock: true)
      }
    }
  }

  @objc private func unlockWithBiometrics() {
    setBusy(true)
    io.async { [self] in
      do {
        let engine = try loadEngine()
        try BiometricKey.unlock(engine: engine, reason: "Unlock KeyRoam to fill your login")
        afterUnlock(engine)
      } catch let e as KeyRoamError where e.code == "cancelled" {
        DispatchQueue.main.async { [self] in
          setBusy(false)
          password.becomeFirstResponder()
        }
      } catch {
        DispatchQueue.main.async { [self] in
          biometricButton.isHidden = !BiometricKey.enabled(vaultId: nil)
        }
        show(error, keepUnlock: true)
      }
    }
  }

  /// Runs on `io`.
  private func afterUnlock(_ engine: KeyRoamEngine) {
    do {
      switch mode {
      case .single(let id):
        try provide(engine, id: id)
      case .list, .configuration:
        let r = try engine.callObject("ios_logins", ["services": services])
        let found = r["logins"] as? [[String: Any]] ?? []
        let site = r["label"] as? String ?? self.site
        DispatchQueue.main.async { [self] in
          self.site = site
          showLogins(found)
        }
      }
    } catch {
      show(error)
    }
  }

  /// Runs on `io`.
  private func provide(_ engine: KeyRoamEngine, id: String) throws {
    let c = try engine.callObject("ios_credential", ["services": services, "id": id])
    let credential = ASPasswordCredential(
      user: c["username"] as? String ?? "", password: c["password"] as? String ?? "")
    _ = try? engine.call("lock")
    DispatchQueue.main.async { [self] in
      extensionContext.completeRequest(withSelectedCredential: credential, completionHandler: nil)
    }
  }

  private func showLogins(_ found: [[String: Any]]) {
    setBusy(false)
    logins = found
    unlockForm.isHidden = true
    view.endEditing(true)
    header.text = site.isEmpty ? "Choose a login" : "Logins for \(site)"
    detail.text =
      found.isEmpty
      ? "KeyRoam has no login saved for \(site.isEmpty ? "this website or app" : site). "
        + "Add the website's address to a login in the KeyRoam app."
      : nil
    table.isHidden = found.isEmpty
    table.reloadData()
  }

  @objc private func cancel() {
    lockQuietly()
    extensionContext.cancelRequest(
      withError: NSError(domain: ASExtensionErrorDomain, code: ASExtensionError.userCanceled.rawValue))
  }

  @objc private func finishConfiguration() {
    extensionContext.completeExtensionConfigurationRequest()
  }

  private func lockQuietly() {
    io.async { _ = try? KeyRoamEngine.shared?.call("lock") }
  }

  private func show(_ error: Error, keepUnlock: Bool = false) {
    let text = (error as? KeyRoamError)?.message ?? error.localizedDescription
    DispatchQueue.main.async { [self] in
      setBusy(false)
      if !keepUnlock {
        unlockForm.isHidden = true
        table.isHidden = true
      }
      message.text = text
    }
  }

  private func setBusy(_ busy: Bool) {
    if busy {
      spinner.startAnimating()
      message.text = nil
    } else {
      spinner.stopAnimating()
    }
    unlockButton.isEnabled = !busy
    biometricButton.isEnabled = !busy
    password.isEnabled = !busy
  }

  override func viewWillDisappear(_ animated: Bool) {
    super.viewWillDisappear(animated)
    lockQuietly()
  }

  // ----- list -----

  func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { logins.count }

  func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
    let cell = tableView.dequeueReusableCell(withIdentifier: "login")
      ?? UITableViewCell(style: .subtitle, reuseIdentifier: "login")
    let login = logins[indexPath.row]
    let username = login["username"] as? String ?? ""
    cell.textLabel?.text = login["title"] as? String ?? ""
    cell.detailTextLabel?.text = username.isEmpty ? "No username" : username
    cell.detailTextLabel?.textColor = .secondaryLabel
    cell.imageView?.image = UIImage(systemName: "key.fill")
    cell.accessoryType = .none
    return cell
  }

  func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
    tableView.deselectRow(at: indexPath, animated: true)
    guard let id = logins[indexPath.row]["id"] as? String else { return }
    setBusy(true)
    io.async { [self] in
      do {
        try provide(try loadEngine(), id: id)
      } catch {
        show(error)
      }
    }
  }

  func textFieldShouldReturn(_ textField: UITextField) -> Bool {
    unlockWithPassword()
    return false
  }

  // ----- layout -----

  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = .systemGroupedBackground

    let cancelButton = UIButton(type: .system)
    cancelButton.setTitle("Cancel", for: .normal)
    cancelButton.addTarget(self, action: #selector(cancel), for: .touchUpInside)
    let brand = UILabel()
    brand.text = "KeyRoam"
    brand.font = .preferredFont(forTextStyle: .headline)
    let bar = UIStackView(arrangedSubviews: [cancelButton, UIView(), brand, UIView(), spinner])
    bar.alignment = .center

    header.font = .preferredFont(forTextStyle: .title2)
    header.adjustsFontForContentSizeCategory = true
    header.numberOfLines = 0
    detail.font = .preferredFont(forTextStyle: .body)
    detail.textColor = .secondaryLabel
    detail.numberOfLines = 0
    message.font = .preferredFont(forTextStyle: .footnote)
    message.textColor = .systemRed
    message.numberOfLines = 0

    password.placeholder = "Master password"
    password.isSecureTextEntry = true
    password.textContentType = .password
    password.borderStyle = .roundedRect
    password.returnKeyType = .go
    password.delegate = self
    unlockButton.setTitle("Unlock", for: .normal)
    unlockButton.titleLabel?.font = .preferredFont(forTextStyle: .headline)
    unlockButton.addTarget(self, action: #selector(unlockWithPassword), for: .touchUpInside)
    biometricButton.setImage(UIImage(systemName: "faceid"), for: .normal)
    biometricButton.addTarget(self, action: #selector(unlockWithBiometrics), for: .touchUpInside)
    unlockForm.axis = .vertical
    unlockForm.spacing = 12
    [password, unlockButton, biometricButton].forEach(unlockForm.addArrangedSubview)
    unlockForm.isHidden = true

    doneButton.setTitle("Done", for: .normal)
    doneButton.titleLabel?.font = .preferredFont(forTextStyle: .headline)
    doneButton.addTarget(self, action: #selector(finishConfiguration), for: .touchUpInside)
    doneButton.isHidden = true

    let top = UIStackView(arrangedSubviews: [bar, header, detail, unlockForm, doneButton, message])
    top.axis = .vertical
    top.spacing = 12
    top.setCustomSpacing(20, after: bar)

    table.dataSource = self
    table.delegate = self
    table.isHidden = true
    table.backgroundColor = .clear

    for v in [top, table] as [UIView] {
      v.translatesAutoresizingMaskIntoConstraints = false
      view.addSubview(v)
    }
    let g = view.safeAreaLayoutGuide
    NSLayoutConstraint.activate([
      top.topAnchor.constraint(equalTo: g.topAnchor, constant: 12),
      top.leadingAnchor.constraint(equalTo: g.leadingAnchor, constant: 20),
      top.trailingAnchor.constraint(equalTo: g.trailingAnchor, constant: -20),
      table.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 8),
      table.leadingAnchor.constraint(equalTo: g.leadingAnchor),
      table.trailingAnchor.constraint(equalTo: g.trailingAnchor),
      table.bottomAnchor.constraint(equalTo: view.bottomAnchor),
    ])
  }
}
