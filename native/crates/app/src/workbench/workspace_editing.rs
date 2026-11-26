//! workspaceEditingService.ts (browser and electron-browser), workspaceCommands.ts, workspaceActions.ts and the
//! `removeRootFolder` command of fileCommands.ts: adding and removing workspace folders, untitled workspaces in
//! `<user data>/Workspaces`, Save Workspace As, Duplicate Workspace and `$updateWorkspaceFolders`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use config::configuration::Overrides;
use config::workspace::{UNTITLED_WORKSPACE_NAME, Workspace, is_untitled_workspace, rewrite_for_new_location, stored_folder, updated_folders};
use gpui::{AsyncWindowContext, Context, PromptLevel, WeakEntity, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::file_dialogs::WORKSPACE_FILTER;
use super::lifecycle::ShutdownReason;
use crate::commands::Outcome;
use crate::dialog;
use crate::file_dialog::FileDialog;
use crate::quick_input::{Entry, Item, Kind, QuickInput, path_label};
use crate::{WindowRequest, open_window_or_log};

/// `IWorkspaceFolderCreationData`: a folder and the name it shows under.
pub type FolderToAdd = (PathBuf, Option<String>);

impl Workbench {
    pub(super) fn register_workspace_editing_commands(&mut self) {
        for id in ["addRootFolder", "workbench.action.addRootFolder"] {
            self.commands.register(id, |workbench, _, window, cx| {
                workbench.pick_and_add_folders(window, cx);
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.removeRootFolder", |workbench, _, window, cx| {
            workbench.pick_and_remove_folder(window, cx);
            Outcome::done()
        });
        // fileCommands.ts REMOVE_ROOT_FOLDER_COMMAND_ID: the selected roots, else the picker.
        self.commands.register("removeRootFolder", |workbench, args, window, cx| {
            let folders: Vec<PathBuf> = workbench.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default();
            let selected = workbench.multi_selected_resources(&args, window);
            let roots: Vec<PathBuf> = selected.into_iter().filter(|path| folders.contains(path)).collect();
            match roots.is_empty() {
                true => workbench.pick_and_remove_folder(window, cx),
                false => workbench.remove_workspace_folders(&roots, window, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.saveWorkspaceAs", |workbench, _, window, cx| {
            let picked = workbench.pick_new_workspace_path(cx);
            cx.spawn_in(window, async move |workbench, cx| {
                let Ok(Some(target)) = picked.await else { return };
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.save_workspace_as(target, window, cx));
            })
            .detach();
            Outcome::done()
        });
        self.commands.register("workbench.action.duplicateWorkspaceInNewWindow", |workbench, _, _, cx| {
            workbench.duplicate_workspace(cx);
            Outcome::done()
        });
    }

    /// `$updateWorkspaceFolders`: the status message, then `updateFolders` with errors going back to the extension.
    pub(crate) fn api_update_workspace_folders(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let extension = args.first().and_then(Value::as_str).unwrap_or_default();
        let index = args.get(1).and_then(Value::as_u64).unwrap_or_default() as usize;
        let delete_count = args.get(2).and_then(Value::as_u64).unwrap_or_default() as usize;
        let mut to_add = Vec::new();
        for folder in args.get(3).and_then(Value::as_array).into_iter().flatten() {
            let path = folder["uri"]["path"].as_str().filter(|_| folder["uri"]["scheme"] == "file");
            let Some(path) = path else { return Outcome::Ready(Err(crate::commands::error_value("Only file folders can be added to the workspace."))) };
            to_add.push((PathBuf::from(path), folder["name"].as_str().map(str::to_owned)));
        }
        self.show_status(folders_status_message(extension, to_add.len(), delete_count), Duration::from_secs(10), window, cx);
        Outcome::Ready(self.update_workspace_folders(index, delete_count, to_add, window, cx).map(|()| Value::Null).map_err(|error| crate::commands::error_value(&error)))
    }

    /// `updateFolders(index, deleteCount, foldersToAdd)`.
    pub(crate) fn update_workspace_folders(&mut self, index: usize, delete_count: usize, to_add: Vec<FolderToAdd>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let folders: Vec<PathBuf> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone()).collect()).unwrap_or_default();
        let to_delete: Vec<PathBuf> = folders.iter().skip(index).take(delete_count).cloned().collect();
        let to_add: Vec<FolderToAdd> = to_add.into_iter().map(|(path, name)| (without_trailing_separator(path), name)).collect();
        match (to_add.is_empty(), to_delete.is_empty()) {
            (true, true) => Ok(()),
            (false, true) => self.add_workspace_folders(to_add, Some(index), window, cx),
            (true, false) => {
                self.remove_workspace_folders(&to_delete, window, cx);
                Ok(())
            }
            (false, false) if self.includes_single_folder_workspace(&to_delete) => {
                self.create_and_enter_workspace(to_add, None, window, cx);
                Ok(())
            }
            (false, false) if !self.in_workspace() => self.add_workspace_folders(to_add, Some(index), window, cx),
            (false, false) => self.write_workspace_folders(&to_add, &to_delete, Some(index), window, cx),
        }
    }

    /// `doAddFolders`: in a folder or empty window the folders and the window's enter a new untitled workspace.
    pub(super) fn add_workspace_folders(&mut self, to_add: Vec<FolderToAdd>, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if self.in_workspace() {
            return self.write_workspace_folders(&to_add, &[], index, window, cx);
        }
        let current: Vec<FolderToAdd> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| (folder.path.clone(), None)).collect()).unwrap_or_default();
        let had = current.len();
        let mut folders = current;
        let at = index.unwrap_or(folders.len()).min(folders.len());
        folders.splice(at..at, to_add);
        let mut distinct: Vec<FolderToAdd> = Vec::new();
        for folder in folders {
            if !distinct.iter().any(|(path, _)| same_path(path, &folder.0)) {
                distinct.push(folder);
            }
        }
        if (had == 0 && distinct.is_empty()) || (had == 1 && distinct.len() == 1) {
            return Ok(());
        }
        self.create_and_enter_workspace(distinct, None, window, cx);
        Ok(())
    }

    /// `removeFolders`: removing a folder window's folder enters an empty untitled workspace.
    fn remove_workspace_folders(&mut self, to_remove: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if self.includes_single_folder_workspace(to_remove) {
            return self.create_and_enter_workspace(Vec::new(), None, window, cx);
        }
        if !self.in_workspace() {
            return;
        }
        if let Err(error) = self.write_workspace_folders(&[], to_remove, None, window, cx) {
            self.notify_error(error);
        }
    }

    /// configurationService.ts `doUpdateFolders` and `setFolders`: the workspace file's `folders` written, then the
    /// window follows (`onWorkspaceConfigurationChanged`).
    fn write_workspace_folders(&mut self, to_add: &[FolderToAdd], to_remove: &[PathBuf], index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let file = self.workspace().and_then(|workspace| workspace.file.clone()).expect("called in a workspace");
        let content = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let raw = config::json::parse(&content, &mut Vec::new(), config::json::ParseOptions { allow_trailing_comma: true, allow_empty_content: true, ..Default::default() }).unwrap_or_default();
        let configured = raw["folders"].as_array().cloned().unwrap_or_default();
        let Some(folders) = updated_folders(&configured, &file, to_add, to_remove, index) else { return Ok(()) };
        let before = self.model_creation_options();
        let change = self.settings.lock().unwrap().write_workspace_folders(&folders);
        let change = change.map_err(|_| "Unable to write into workspace configuration file. Please open the file to correct errors/warnings in it and try again.".to_owned())?;
        self.workspace_folders_changed(&change, before, Some(window), cx);
        Ok(())
    }

    /// `onDidChangeWorkspaceFolders` listeners, with the folders already in the settings: the Explorer's roots, the
    /// extension host's workspace, the file watcher; `WorkspaceChangeExtHostRelauncher` restarts the extension host
    /// when the first folder changed, except while extension tests run.
    pub(super) fn workspace_folders_changed(&mut self, change: &config::model::Change, before: Vec<config::indentation::CreationOptions>, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(workspace) = self.settings.lock().unwrap().workspace.clone() else { return };
        let Some(tree) = self.folder.as_mut() else { return };
        let first_changed = tree.workspace.folders.first().map(|folder| &folder.path) != workspace.folders.first().map(|folder| &folder.path);
        tree.workspace = workspace.clone();
        self.host.send_workspace_data(&workspace);
        self.configuration_changed(change, before, cx);
        // configuration.ts `onDidChangeWorkspaceFolders`: the added folders' settings migrate too.
        self.migrate_configurations(cx);
        self.text_files.watched_files.clear();
        self.start_file_watcher(cx);
        if let Some(window) = window.filter(|_| first_changed && !std::env::args().any(|arg| arg.starts_with("--extensionTestsPath"))) {
            self.restart_extension_host(window, cx);
        }
        cx.notify();
    }

    /// `createAndEnterWorkspace`: a new untitled workspace with `folders`, saved to `path` when given.
    fn create_and_enter_workspace(&mut self, folders: Vec<FolderToAdd>, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &path
            && !self.is_valid_target_workspace_path(path, window, cx)
        {
            return;
        }
        let user_data = cx.global::<crate::AppState>().user_data.clone();
        let result = create_untitled_workspace(&user_data, &folders).and_then(|untitled| match path {
            Some(path) => {
                let saved = self.save_workspace_file_as(&untitled, &path);
                delete_untitled_workspace(&user_data, &untitled);
                saved.map(|()| path)
            }
            None => Ok(untitled),
        });
        match result {
            Ok(file) => self.enter_workspace(file, window, cx),
            Err(error) => self.notify_error(error.to_string()),
        }
    }

    /// `saveAndEnterWorkspace`, or `createAndEnterWorkspace` with this window's folders outside a workspace.
    fn save_workspace_as(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.workspace().and_then(|workspace| workspace.file.clone());
        let Some(current) = current else {
            let folders = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| (folder.path.clone(), None)).collect()).unwrap_or_default();
            return self.create_and_enter_workspace(folders, Some(target), window, cx);
        };
        if current == target {
            return self.save_workspace(&current, cx);
        }
        if !self.is_valid_target_workspace_path(&target, window, cx) {
            return;
        }
        match self.save_workspace_file_as(&current, &target) {
            Ok(()) => self.enter_workspace(target, window, cx),
            Err(error) => self.notify_error(error.to_string()),
        }
    }

    /// `saveWorkspace`: the open workspace file saved even when clean, or written again when it was deleted.
    fn save_workspace(&mut self, file: &Path, cx: &mut Context<Self>) {
        if self.editors.model("file", file).is_some() {
            let _ = self.save_resource(&json!({ "scheme": "file", "path": file.to_string_lossy() }), cx);
            return;
        }
        if file.exists() {
            return;
        }
        let content = rewrite_for_new_location("{\n\t\"folders\": []\n}", file, file);
        if let Err(error) = std::fs::write(file, content) {
            self.notify_error(error.to_string());
        }
    }

    /// `saveWorkspaceAs`: the workspace file's text with its folders rewritten for `target`, then the target trusted
    /// when this window is (`trustWorkspaceConfiguration`).
    fn save_workspace_file_as(&mut self, source: &Path, target: &Path) -> std::io::Result<()> {
        if same_path(source, target) {
            return Ok(());
        }
        let content = std::fs::read_to_string(source)?;
        std::fs::write(target, rewrite_for_new_location(&content, source, target))?;
        if self.workspace().is_some() && self.workspace_trust.is_trusted() {
            let empty_window_trusted = self.setting("security.workspace.trust.emptyWindow") == true;
            self.workspace_trust.set_uris_trust(&[target.to_path_buf()], true, &mut self.workspace_storage.lock().unwrap(), empty_window_trusted);
        }
        Ok(())
    }

    /// `enterWorkspace`: this window's editors are backed up and its workspace storage copied for the workspace,
    /// then the window opens again on it in the same place.
    // shortcut: the window reopens where upstream switches the storage, the backups and the extension host in place;
    // the editors and view state come back from the copied storage and backups.
    fn enter_workspace(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.backup_before_shutdown();
        let user_data = cx.global::<crate::AppState>().user_data.clone();
        let old_backups = self.lifecycle.backups.home.clone();
        let storage = self.workspace_storage.clone();
        let ended = self.lifecycle.session_ended.take();
        let workspace = Workspace::open(file);
        let id = exthost::session::workspace_id(&workspace);
        let request = WindowRequest { workspace: Some(workspace), ui: Some(crate::windows_state::ui_state(window, cx)), ..WindowRequest::default() };
        self.lifecycle.window_closed = true;
        window.remove_window();
        cx.spawn(async move |_, cx| {
            if let Some(ended) = ended {
                let _ = ended.await;
            }
            let target = user_data.join("User/workspaceStorage").join(&id);
            if let Err(error) = copy_storage(&storage.lock().unwrap(), &target) {
                eprintln!("[workspaces] copying the workspace storage to {}: {error}", target.display());
            }
            if let Some(old) = old_backups.filter(|old| old.exists()) {
                let new = user_data.join("Backups").join(&id);
                if let Err(error) = move_backups(&user_data, &old, &new) {
                    eprintln!("[workspaces] moving the backups to {}: {error}", new.display());
                }
            }
            let _ = cx.update(|cx| open_window_or_log(request, cx));
        })
        .detach();
    }

    /// DuplicateWorkspaceInNewWindowAction: an untitled workspace with this window's folders and its workspace
    /// settings (`copyWorkspaceSettings`) opens in a new window.
    fn duplicate_workspace(&mut self, cx: &mut Context<Self>) {
        let folders: Vec<FolderToAdd> = self.workspace().map(|workspace| workspace.folders.iter().map(|folder| (folder.path.clone(), Some(folder.name.clone()))).collect()).unwrap_or_default();
        let user_data = cx.global::<crate::AppState>().user_data.clone();
        let file = match create_untitled_workspace(&user_data, &folders) {
            Ok(file) => file,
            Err(error) => return self.notify_error(error.to_string()),
        };
        if let Err(error) = self.copy_workspace_settings(&file) {
            self.notify_error(error);
        }
        let request = WindowRequest { workspace: Some(Workspace::open(file)), ..WindowRequest::default() };
        cx.defer(move |cx| open_window_or_log(request, cx));
    }

    /// `copyWorkspaceSettings`: the workspace values of every registered setting that is not application scoped.
    fn copy_workspace_settings(&self, file: &Path) -> Result<(), String> {
        let settings = self.settings.lock().unwrap();
        let mut copied = serde_json::Map::new();
        for key in &settings.configuration.workspace.keys {
            let Some(property) = settings.registry.property(key) else { continue };
            if config::registry::APPLICATION_SCOPES.contains(&property.scope) {
                continue;
            }
            copied.insert(key.clone(), settings.configuration.workspace.get_value(Some(key)).unwrap_or(Value::Null));
        }
        drop(settings);
        let content = std::fs::read_to_string(file).map_err(|error| error.to_string())?;
        let options = config::json_edit::FormattingOptions { insert_spaces: false, tab_size: 4, eol: "\n".into() };
        let edits = config::json_edit::set_property(&content, &[config::json::Segment::Key("settings".into())], Some(&Value::Object(copied)), &options);
        std::fs::write(file, config::json_edit::apply_edits(&content, &edits)).map_err(|error| error.to_string())
    }

    /// NativeWorkspaceEditingService `isValidTargetWorkspacePath`: a workspace open in another window is not
    /// written over.
    fn is_valid_target_workspace_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let this = window.window_handle().window_id();
        let open_elsewhere = cx.windows().into_iter().filter(|handle| handle.window_id() != this).filter_map(|handle| handle.downcast::<Workbench>()).any(|handle| {
            handle.read(cx).is_ok_and(|workbench| workbench.workspace().and_then(|workspace| workspace.file.as_deref()).is_some_and(|file| same_path(file, path)))
        });
        if open_elsewhere {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let detail = "The workspace is already opened in another window. Please close that window first and then try again.";
            let _ = dialog::show_message_box(window, PromptLevel::Info, &format!("Unable to save workspace '{name}'"), Some(detail), &["OK"], None, cx);
        }
        !open_elsewhere
    }

    /// `pickNewWorkspacePath`: the save dialog with the workspace filter, the extension added when missing.
    fn pick_new_workspace_path(&mut self, cx: &mut Context<Self>) -> futures::channel::oneshot::Receiver<Option<PathBuf>> {
        let default_path = Some(self.default_workspace_path().join(self.new_workspace_name()));
        let filters = vec![(WORKSPACE_FILTER.0.into(), vec![WORKSPACE_FILTER.1.into()])];
        let picked = self.show_save_dialog(FileDialog { title: Some("Save Workspace".into()), button_label: Some("Save".into()), filters, default_path, ..FileDialog::default() }, cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |_, _| {
            let picked = picked.await.ok().flatten().map(|path| match path.extension().is_some_and(|extension| extension == WORKSPACE_FILTER.1) {
                true => path,
                false => PathBuf::from(format!("{}.{}", path.display(), WORKSPACE_FILTER.1)),
            });
            let _ = sender.send(picked);
        })
        .detach();
        receiver
    }

    /// `getNewWorkspaceName`.
    fn new_workspace_name(&self) -> String {
        let workspace = self.workspace();
        if let Some(file) = workspace.and_then(|workspace| workspace.file.as_deref()).filter(|file| !is_untitled_workspace(file)) {
            return file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        }
        match workspace.and_then(|workspace| workspace.folders.first()) {
            Some(folder) => format!("{}.{}", folder.path.file_name().unwrap_or_default().to_string_lossy(), WORKSPACE_FILTER.1),
            None => format!("workspace.{}", WORKSPACE_FILTER.1),
        }
    }

    /// ADD_ROOT_FOLDER_COMMAND_ID with `selectWorkspaceFolders`.
    fn pick_and_add_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = FileDialog { title: Some("Add Folder to Workspace".into()), button_label: Some("Add".into()), folders: true, multiple: true, default_path: Some(self.default_folder_path()), ..FileDialog::default() };
        let picked = self.show_open_dialog(dialog, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(folders) = picked.await else { return };
            if folders.is_empty() {
                return;
            }
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                let to_add = folders.into_iter().map(|folder| (without_trailing_separator(folder), None)).collect();
                if let Err(error) = workbench.add_workspace_folders(to_add, None, window, cx) {
                    workbench.notify_error(error);
                }
            });
        })
        .detach();
    }

    /// RemoveRootFolderAction with PICK_WORKSPACE_FOLDER_COMMAND_ID's picker.
    fn pick_and_remove_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace().cloned().filter(|workspace| !workspace.folders.is_empty()) else { return };
        let mut entries = Vec::new();
        for (index, folder) in workspace.folders.iter().enumerate() {
            let parent = folder.path.parent().map(|parent| path_label(parent, Some(&workspace))).unwrap_or_default();
            let description = (parent != folder.name).then_some(parent);
            // shortcut: no ROOT_FOLDER icon classes; Seti, the default theme, has no folder icons, others would show one.
            entries.push(Entry::Item(Item { handle: index as i64, label: folder.name.clone(), description, ..Item::default() }));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select workspace folder".into());
        input.set_items(entries);
        input.set_active_items(&[0]);
        let picked = self.choose(input, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Value::Number(index))) = picked.await else { return };
            let Some(folder) = index.as_u64().and_then(|index| workspace.folders.get(index as usize)) else { return };
            let folder = folder.path.clone();
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.remove_workspace_folders(&[folder], window, cx));
        })
        .detach();
    }

    fn in_workspace(&self) -> bool {
        self.workspace().is_some_and(|workspace| workspace.file.is_some())
    }

    /// `includesSingleFolderWorkspace`.
    fn includes_single_folder_workspace(&self, folders: &[PathBuf]) -> bool {
        match self.workspace().filter(|workspace| workspace.file.is_none()) {
            Some(workspace) => folders.iter().any(|folder| same_path(folder, &workspace.folders[0].path)),
            None => false,
        }
    }
}

/// NativeWorkspaceEditingService `saveUntitledBeforeShutdown`: closing or leaving an untitled workspace asks to save
/// it (`window.confirmSaveUntitledWorkspace`); true vetoes.
pub(super) async fn save_untitled_before_shutdown(workbench: &WeakEntity<Workbench>, reason: ShutdownReason, cx: &mut AsyncWindowContext) -> bool {
    if reason != ShutdownReason::Load && reason != ShutdownReason::Close {
        return false;
    }
    let asked = workbench.update_in(cx, |workbench, window, cx| {
        let file = workbench.workspace().and_then(|workspace| workspace.file.clone())?;
        let user_data = cx.global::<crate::AppState>().user_data.clone();
        let untitled = Some(file).filter(|file| is_own_untitled_workspace(&user_data, file))?;
        if workbench.settings.lock().unwrap().get_value(Some("window.confirmSaveUntitledWorkspace"), &Overrides::default()) == Some(json!(false)) {
            delete_untitled_workspace(&user_data, &untitled);
            return None;
        }
        let message = "Do you want to save your workspace configuration as a file?";
        let answer = dialog::show_message_box(window, PromptLevel::Warning, message, Some("Save your workspace if you plan to open it again."), &["Save", "Don't Save", "Cancel"], Some("Always discard untitled workspaces without asking"), cx);
        Some((untitled, user_data, answer))
    });
    let Ok(Some((untitled, user_data, answer))) = asked else { return false };
    let Ok((button, checked)) = answer.await else { return true };
    if button != 2 && checked {
        let _ = workbench.update(cx, |workbench, _| workbench.update_setting("window.confirmSaveUntitledWorkspace", json!(false)));
    }
    match button {
        0 => {
            let Ok(picked) = workbench.update(cx, |workbench, cx| workbench.pick_new_workspace_path(cx)) else { return true };
            let Ok(Some(target)) = picked.await else { return true };
            let saved = workbench.update(cx, |workbench, _| workbench.save_workspace_file_as(&untitled, &target).map(|()| target));
            if let Ok(Ok(target)) = saved {
                let _ = workbench.update(cx, |workbench, _| {
                    let entry = crate::recently_opened::entry(crate::recently_opened::Kind::Workspace, &target, None);
                    crate::recently_opened::add(&mut workbench.shared_storage.lock().unwrap(), vec![entry]);
                });
                delete_untitled_workspace(&user_data, &untitled);
            }
            false
        }
        1 => {
            delete_untitled_workspace(&user_data, &untitled);
            false
        }
        _ => true,
    }
}

/// workspacesManagementMainService.ts `createUntitledWorkspace`: `Workspaces/<Date.now() + random>/workspace.json`
/// with the folders stored absolute, tab indented.
fn create_untitled_workspace(user_data: &Path, folders: &[FolderToAdd]) -> std::io::Result<PathBuf> {
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis());
    let random = std::hash::BuildHasher::hash_one(&std::collections::hash_map::RandomState::new(), millis) as u128 % 1000;
    let folder = user_data.join("Workspaces").join((millis + random).to_string());
    std::fs::create_dir_all(&folder)?;
    let stored: Vec<Value> = folders.iter().map(|(path, name)| stored_folder(path, true, name.as_deref(), &folder)).collect();
    let mut content = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut content, serde_json::ser::PrettyFormatter::with_indent(b"\t"));
    serde::Serialize::serialize(&json!({ "folders": stored }), &mut serializer).map_err(std::io::Error::other)?;
    let file = folder.join(UNTITLED_WORKSPACE_NAME);
    std::fs::write(&file, content)?;
    Ok(file)
}

/// `deleteUntitledWorkspace`: its folder goes and its workspace storage is marked obsolete.
fn delete_untitled_workspace(user_data: &Path, file: &Path) {
    if !is_own_untitled_workspace(user_data, file) {
        return;
    }
    if let Some(folder) = file.parent()
        && let Err(error) = std::fs::remove_dir_all(folder)
    {
        eprintln!("[workspaces] Unable to delete untitled workspace {} ({error}).", file.display());
    }
    let storage = user_data.join("User/workspaceStorage").join(exthost::session::workspace_id(&Workspace { file: Some(file.to_path_buf()), folders: Vec::new(), transient: false }));
    if storage.exists() {
        let _ = std::fs::write(storage.join("obsolete"), "");
    }
}

/// `isUntitledWorkspace` against this user data's `untitledWorkspacesHome`: a `Workspaces/<id>/workspace.json`
/// elsewhere on disk is the user's file, never deleted.
fn is_own_untitled_workspace(user_data: &Path, file: &Path) -> bool {
    is_untitled_workspace(file) && file.starts_with(user_data.join("Workspaces"))
}

/// `storageService.switch(workspace, preserveData)`: every item of this window's workspace storage in the new one.
fn copy_storage(from: &storage::Storage, folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let mut target = storage::Storage::open(&folder.join("state.vscdb"));
    for (key, value) in from.items() {
        target.set(key, value.clone());
    }
    Ok(())
}

/// backupMainService.ts `moveBackupFolder`: the backups of the window's old workspace move to the new one; backups
/// the new one already had become an empty window's (`convertToEmptyWindowBackup`), so none is lost.
fn move_backups(user_data: &Path, from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis());
        let folder = millis.to_string();
        std::fs::rename(to, user_data.join("Backups").join(&folder))?;
        crate::backups::register(user_data, "emptyWindows", json!({ "backupFolder": folder }));
    }
    std::fs::create_dir_all(to.parent().expect("in Backups"))?;
    std::fs::rename(from, to)
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase().trim_end_matches('/') == b.to_string_lossy().to_lowercase().trim_end_matches('/')
}

/// `removeTrailingPathSeparator`.
fn without_trailing_separator(path: PathBuf) -> PathBuf {
    match path.to_string_lossy().trim_end_matches('/') {
        "" => path,
        trimmed => PathBuf::from(trimmed),
    }
}

/// mainThreadWorkspace.ts `getStatusMessage`.
fn folders_status_message(extension: &str, add: usize, remove: usize) -> String {
    match (add, remove) {
        (1, 0) => format!("Extension '{extension}' added 1 folder to the workspace"),
        (add, 0) => format!("Extension '{extension}' added {add} folders to the workspace"),
        (0, 1) => format!("Extension '{extension}' removed 1 folder from the workspace"),
        (0, remove) => format!("Extension '{extension}' removed {remove} folders from the workspace"),
        _ => format!("Extension '{extension}' changed folders of the workspace"),
    }
}

#[cfg(test)]
mod tests;
