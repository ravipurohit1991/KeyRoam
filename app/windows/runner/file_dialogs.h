#ifndef RUNNER_FILE_DIALOGS_H_
#define RUNNER_FILE_DIALOGS_H_

#include <windows.h>

#include <optional>
#include <string>
#include <vector>

// Native open and save dialogs for the `keyroam/files` channel: backups and
// CSV imports. All strings are UTF-8.
struct FileDialogOptions {
  bool save = false;
  std::string title;
  // Save dialogs: the folder and file name to start with.
  std::string initial_directory;
  std::string suggested_name;
  // Shown as "<type_label> (*.ext1;*.ext2)"; extensions without the dot.
  std::string type_label;
  std::vector<std::string> extensions;
};

// The chosen file's path, or nothing if the user cancelled.
std::optional<std::string> ShowFileDialog(HWND owner,
                                          const FileDialogOptions& options);

#endif  // RUNNER_FILE_DIALOGS_H_
