//! workbench/services/dialogs: abstractFileDialogService.ts default paths, fileDialogService.ts (electron-browser)
//! open and save dialogs, mainThreadDialogs.ts `$showOpenDialog` and `$showSaveDialog`.

use std::path::{Path, PathBuf};

use futures::channel::oneshot;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::file_dialog::{self, FileDialog};
use crate::languages::Languages;
use crate::uri::Uri;

/// dialogMainService.ts `WORKSPACE_FILTER`.
pub const WORKSPACE_FILTER: (&str, &str) = ("Code Workspace", "code-workspace");

impl Workbench {
    /// `defaultFilePath`: the last active file's folder, then the last active workspace root, then the preferred home.
    pub(super) fn default_file_path(&self) -> PathBuf {
        let last_active_folder = self.last_active_file().and_then(|file| file.parent().map(Path::to_path_buf));
        last_active_folder.or_else(|| self.last_active_root()).unwrap_or_else(|| self.preferred_home())
    }

    /// `defaultFolderPath`: the folder around the last active workspace root or file, else the preferred home.
    pub(super) fn default_folder_path(&self) -> PathBuf {
        let candidate = self.last_active_root().or_else(|| self.last_active_file());
        match candidate {
            Some(candidate) => candidate.parent().map_or(candidate.clone(), Path::to_path_buf),
            None => self.preferred_home(),
        }
    }

    /// `defaultWorkspacePath`: the workspace file's folder, else the default folder path.
    pub(super) fn default_workspace_path(&self) -> PathBuf {
        match self.workspace().and_then(|workspace| workspace.file.as_ref()) {
            Some(file) => file.parent().map_or(file.clone(), Path::to_path_buf),
            None => self.default_folder_path(),
        }
    }

    fn last_active_file(&self) -> Option<PathBuf> {
        self.quick_access.editor_history.entries.first().cloned()
    }

    /// historyService.ts `getLastActiveWorkspaceRoot`: the one folder, or the folder of the most recent editor that is
    /// in one, else the first folder.
    fn last_active_root(&self) -> Option<PathBuf> {
        let folders = &self.workspace()?.folders;
        if let [folder] = folders.as_slice() {
            return Some(folder.path.clone());
        }
        let in_folder = self.quick_access.editor_history.entries.iter().find_map(|entry| folders.iter().find(|folder| entry.starts_with(&folder.path)));
        in_folder.or(folders.first()).map(|folder| folder.path.clone())
    }

    /// `preferredHome`: an absolute `files.dialog.defaultPath` that exists, else the user home.
    fn preferred_home(&self) -> PathBuf {
        let configured = self.setting("files.dialog.defaultPath");
        if let Some(configured) = configured.as_str().map(PathBuf::from).filter(|path| path.is_absolute() && path.exists()) {
            return configured;
        }
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
    }

    /// `pickFileToSave`: the Save As dialog at `default`; a picked file goes into the recently opened list.
    pub(super) fn pick_file_to_save(&mut self, default: PathBuf, cx: &mut Context<Self>) -> oneshot::Receiver<Option<PathBuf>> {
        let filters = save_filters(&self.languages, &default);
        let dialog = FileDialog { title: Some("Save As".into()), filters, default_path: Some(default), ..FileDialog::default() };
        let picked = file_dialog::show_save_dialog(dialog, cx);
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let picked = picked.await.ok().flatten();
            if let Some(path) = &picked {
                let _ = workbench.update(cx, |workbench, _| {
                    let entry = crate::recently_opened::entry(crate::recently_opened::Kind::File, path, None);
                    crate::recently_opened::add(&mut workbench.shared_storage.lock().unwrap(), vec![entry]);
                });
            }
            let _ = sender.send(picked);
        })
        .detach();
        receiver
    }

    /// `pickFileFolderAndOpen`, `pickFileAndOpen`, `pickFolderAndOpen` and `pickWorkspaceAndOpen`: the native dialog
    /// of `doPick`, at the default path the kind of pick takes.
    pub(super) fn pick_to_open(&mut self, pick: Pick, cx: &mut Context<Self>) -> oneshot::Receiver<Vec<PathBuf>> {
        let (title, default_path) = match pick {
            Pick::FileFolder => ("Open", self.default_file_path()),
            Pick::File => ("Open File", self.default_file_path()),
            Pick::Folder => ("Open Folder", self.default_folder_path()),
            Pick::Workspace => ("Open Workspace from File", self.default_workspace_path()),
        };
        let mut dialog = FileDialog { title: Some(title.into()), default_path: Some(default_path), multiple: true, packages_as_directories: true, ..FileDialog::default() };
        dialog.files = pick != Pick::Folder;
        dialog.folders = matches!(pick, Pick::FileFolder | Pick::Folder);
        if pick == Pick::Workspace {
            dialog.button_label = Some("Open".into());
            dialog.filters = vec![(WORKSPACE_FILTER.0.into(), vec![WORKSPACE_FILTER.1.into()])];
        }
        file_dialog::show_open_dialog(dialog, cx)
    }

    /// `$showOpenDialog`: the picked file URIs, `undefined` when nothing was picked.
    pub(crate) fn api_show_open_dialog(&mut self, options: &Value, cx: &mut Context<Self>) -> Outcome {
        let mut dialog = api_dialog(options, "openLabel");
        dialog.files = options["canSelectFiles"] == true || (options["canSelectFiles"] != true && options["canSelectFolders"] != true);
        dialog.folders = options["canSelectFolders"] == true;
        dialog.multiple = options["canSelectMany"] == true;
        let picked = self.show_open_dialog(dialog, cx);
        let to_value = |paths: Vec<PathBuf>| match paths.is_empty() {
            true => Value::Null,
            false => Value::Array(paths.iter().map(|path| file_uri(path)).collect()),
        };
        Outcome::Pending(answer(picked, to_value, cx))
    }

    /// fileDialogService.ts `showOpenDialog`: without a default path the panel opens at the default folder path for a
    /// folder pick, else at the default file path.
    pub(super) fn show_open_dialog(&mut self, mut dialog: FileDialog, cx: &mut Context<Self>) -> oneshot::Receiver<Vec<PathBuf>> {
        if dialog.default_path.is_none() {
            dialog.default_path = Some(match dialog.folders && !dialog.files {
                true => self.default_folder_path(),
                false => self.default_file_path(),
            });
        }
        file_dialog::show_open_dialog(dialog, cx)
    }

    /// fileDialogService.ts `showSaveDialog`, at the default file path unless `dialog` names one.
    pub(super) fn show_save_dialog(&mut self, mut dialog: FileDialog, cx: &mut Context<Self>) -> oneshot::Receiver<Option<PathBuf>> {
        dialog.default_path.get_or_insert_with(|| self.default_file_path());
        file_dialog::show_save_dialog(dialog, cx)
    }

    /// `$showSaveDialog`: the picked file URI, `undefined` when cancelled.
    pub(crate) fn api_show_save_dialog(&mut self, options: &Value, cx: &mut Context<Self>) -> Outcome {
        let dialog = api_dialog(options, "saveLabel");
        let picked = self.show_save_dialog(dialog, cx);
        Outcome::Pending(answer(picked, |path: Option<PathBuf>| path.as_deref().map_or(Value::Null, file_uri), cx))
    }
}

/// `_convertOpenOptions` and `_convertSaveOptions`: the label, title, default URI and the `{ name: extensions }` filters.
fn api_dialog(options: &Value, label_key: &str) -> FileDialog {
    let text = |key: &str| options[key].as_str().filter(|text| !text.is_empty()).map(str::to_owned);
    let default_path = Uri::from_json(&options["defaultUri"]).ok().filter(|_| options["defaultUri"].is_object()).map(|uri| PathBuf::from(uri.path));
    let mut filters = Vec::new();
    for (name, extensions) in options["filters"].as_object().into_iter().flatten() {
        let extensions = extensions.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        filters.push((name.clone(), extensions));
    }
    FileDialog { title: text("title"), button_label: text(label_key), filters, default_path, ..FileDialog::default() }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Pick {
    FileFolder,
    File,
    Folder,
    Workspace,
}

/// `getPickFileToSaveDialogOptions` filters: All Files, the language of the name's extension, every other language
/// with extensions (10 at most), No Extension.
fn save_filters(languages: &Languages, default: &Path) -> Vec<(String, Vec<String>)> {
    let extension = default.extension().map(|extension| format!(".{}", extension.to_string_lossy()));
    let extension_or_plaintext = extension.clone().unwrap_or_else(|| ".txt".into());
    let mut matching = None;
    let mut language_filters = Vec::new();
    for (name, id) in languages.sorted_names() {
        let extensions = languages.extensions(id);
        if extensions.is_empty() {
            continue;
        }
        let mut trimmed: Vec<String> = extensions.iter().take(10).map(|extension| extension.trim_matches('.').to_owned()).collect();
        if matching.is_none() && extensions.contains(&extension_or_plaintext.as_str()) {
            let selected = extension_or_plaintext.trim_matches('.').to_owned();
            if !trimmed.contains(&selected) {
                trimmed.insert(0, selected);
            }
            matching = Some((name.to_owned(), trimmed));
            continue;
        }
        language_filters.push((name.to_owned(), trimmed));
    }
    if matching.is_none() {
        matching = extension.map(|extension| {
            let extension = extension.trim_matches('.').to_owned();
            (extension.to_uppercase(), vec![extension])
        });
    }
    let mut filters = vec![("All Files".to_owned(), vec!["*".to_owned()])];
    filters.extend(matching);
    filters.extend(language_filters);
    filters.push(("No Extension".into(), vec![String::new()]));
    filters
}

fn file_uri(path: &Path) -> Value {
    json!({ "$mid": 1, "scheme": "file", "authority": "", "path": path.to_string_lossy(), "query": "", "fragment": "" })
}

fn answer<T: Send + 'static>(picked: oneshot::Receiver<T>, to_value: impl FnOnce(T) -> Value + Send + 'static, cx: &mut Context<Workbench>) -> oneshot::Receiver<Result<Value, Value>> {
    let (sender, receiver) = oneshot::channel();
    cx.background_executor()
        .spawn(async move {
            let value = picked.await.map_or(Value::Null, to_value);
            let _ = sender.send(Ok(value));
        })
        .detach();
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_options_become_the_panel_options() {
        let options = json!({ "openLabel": "Pick", "title": "", "defaultUri": { "scheme": "file", "path": "/tmp/a.txt" }, "filters": { "Images": ["png", "jpg"], "All": ["*"] } });
        let dialog = api_dialog(&options, "openLabel");
        assert_eq!(dialog.button_label.as_deref(), Some("Pick"));
        assert_eq!(dialog.title, None);
        assert_eq!(dialog.default_path, Some(PathBuf::from("/tmp/a.txt")));
        assert_eq!(dialog.filters, vec![("Images".to_owned(), vec!["png".to_owned(), "jpg".to_owned()]), ("All".to_owned(), vec!["*".to_owned()])]);
        assert_eq!(api_dialog(&json!({}), "saveLabel"), FileDialog::default());
    }

    #[test]
    fn save_filters_put_the_matching_language_after_all_files() {
        let languages = Languages::new(&[]);
        let filters = save_filters(&languages, Path::new("/x/a.txt"));
        assert_eq!(filters[0], ("All Files".to_owned(), vec!["*".to_owned()]));
        assert_eq!(filters[1].0, "Plain Text");
        assert_eq!(filters[1].1[0], "txt");
        assert_eq!(filters.last().unwrap(), &("No Extension".to_owned(), vec![String::new()]));
        assert_eq!(save_filters(&languages, Path::new("/x/a.zzz"))[1], ("ZZZ".to_owned(), vec!["zzz".to_owned()]));
    }
}
