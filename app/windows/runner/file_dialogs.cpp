#include "file_dialogs.h"

#include <shobjidl.h>
#include <wrl/client.h>

#include "utils.h"

using Microsoft::WRL::ComPtr;

namespace {

std::wstring Utf16FromUtf8(const std::string& utf8) {
  if (utf8.empty()) {
    return std::wstring();
  }
  int length = ::MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                     utf8.data(), static_cast<int>(utf8.size()),
                                     nullptr, 0);
  if (length <= 0) {
    return std::wstring();
  }
  std::wstring utf16(length, L'\0');
  ::MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(),
                        static_cast<int>(utf8.size()), utf16.data(), length);
  return utf16;
}

}  // namespace

std::optional<std::string> ShowFileDialog(HWND owner,
                                          const FileDialogOptions& options) {
  ComPtr<IFileDialog> dialog;
  if (FAILED(::CoCreateInstance(
          options.save ? CLSID_FileSaveDialog : CLSID_FileOpenDialog, nullptr,
          CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog)))) {
    return std::nullopt;
  }

  DWORD flags = 0;
  dialog->GetOptions(&flags);
  // Only real files: the engine opens the path itself.
  flags |= FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR;
  flags |= options.save ? FOS_OVERWRITEPROMPT
                        : FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST;
  dialog->SetOptions(flags);

  if (!options.title.empty()) {
    dialog->SetTitle(Utf16FromUtf8(options.title).c_str());
  }

  // "KeyRoam backup (*.krbackup)", then "All files (*.*)".
  std::wstring pattern;
  for (const auto& ext : options.extensions) {
    if (!pattern.empty()) pattern += L";";
    pattern += L"*." + Utf16FromUtf8(ext);
  }
  std::wstring label = Utf16FromUtf8(options.type_label) + L" (" + pattern + L")";
  std::vector<COMDLG_FILTERSPEC> specs;
  if (!pattern.empty()) {
    specs.push_back({label.c_str(), pattern.c_str()});
  }
  specs.push_back({L"All files (*.*)", L"*.*"});
  dialog->SetFileTypes(static_cast<UINT>(specs.size()), specs.data());
  dialog->SetFileTypeIndex(1);
  if (options.save && !options.extensions.empty()) {
    dialog->SetDefaultExtension(Utf16FromUtf8(options.extensions[0]).c_str());
  }

  if (!options.suggested_name.empty()) {
    dialog->SetFileName(Utf16FromUtf8(options.suggested_name).c_str());
  }
  if (!options.initial_directory.empty()) {
    ComPtr<IShellItem> folder;
    if (SUCCEEDED(::SHCreateItemFromParsingName(
            Utf16FromUtf8(options.initial_directory).c_str(), nullptr,
            IID_PPV_ARGS(&folder)))) {
      dialog->SetFolder(folder.Get());
    }
  }

  // Modal: returns when the user picks a file or cancels.
  if (FAILED(dialog->Show(owner))) {
    return std::nullopt;
  }
  ComPtr<IShellItem> item;
  if (FAILED(dialog->GetResult(&item))) {
    return std::nullopt;
  }
  PWSTR path = nullptr;
  if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &path))) {
    return std::nullopt;
  }
  std::string result = Utf8FromUtf16(path);
  ::CoTaskMemFree(path);
  if (result.empty()) {
    return std::nullopt;
  }
  return result;
}
