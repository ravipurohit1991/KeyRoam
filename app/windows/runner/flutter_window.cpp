#include "flutter_window.h"

#include <flutter/standard_method_codec.h>
#include <wtsapi32.h>

#include <optional>

#include "file_dialogs.h"
#include "flutter/generated_plugin_registrant.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  system_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "keyroam/system",
          &flutter::StandardMethodCodec::GetInstance());
  files_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(), "keyroam/files",
          &flutter::StandardMethodCodec::GetInstance());
  files_channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) { HandleFileDialog(call, std::move(result)); });
  // Receive WM_WTSSESSION_CHANGE when the session is locked.
  WTSRegisterSessionNotification(GetHandle(), NOTIFY_FOR_THIS_SESSION);
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  WTSUnRegisterSessionNotification(GetHandle());
  system_channel_ = nullptr;
  files_channel_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
    case WM_WTSSESSION_CHANGE:
      if (wparam == static_cast<WPARAM>(WTS_SESSION_LOCK)) {
        LockVault();
      }
      break;
    case WM_POWERBROADCAST:
      if (wparam == static_cast<WPARAM>(PBT_APMSUSPEND)) {
        LockVault();
      }
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

void FlutterWindow::HandleFileDialog(
    const flutter::MethodCall<flutter::EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  const bool save = call.method_name() == "save";
  if (!save && call.method_name() != "open") {
    result->NotImplemented();
    return;
  }
  const auto* args = std::get_if<flutter::EncodableMap>(call.arguments());
  auto text = [args](const char* key) -> std::string {
    if (!args) return std::string();
    auto it = args->find(flutter::EncodableValue(key));
    if (it == args->end()) return std::string();
    const auto* s = std::get_if<std::string>(&it->second);
    return s ? *s : std::string();
  };
  FileDialogOptions options;
  options.save = save;
  options.title = text("title");
  options.initial_directory = text("initialDirectory");
  options.suggested_name = text("suggestedName");
  options.type_label = text("typeLabel");
  if (args) {
    auto it = args->find(flutter::EncodableValue("extensions"));
    if (it != args->end()) {
      if (const auto* list = std::get_if<flutter::EncodableList>(&it->second)) {
        for (const auto& e : *list) {
          if (const auto* s = std::get_if<std::string>(&e)) {
            options.extensions.push_back(*s);
          }
        }
      }
    }
  }
  std::optional<std::string> path = ShowFileDialog(GetHandle(), options);
  if (path) {
    result->Success(flutter::EncodableValue(*path));
  } else {
    result->Success();
  }
}

void FlutterWindow::LockVault() {
  if (system_channel_) {
    system_channel_->InvokeMethod("lock", nullptr);
  }
}
