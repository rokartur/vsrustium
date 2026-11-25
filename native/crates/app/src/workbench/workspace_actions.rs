//! workspaceActions.ts, windowActions.ts and editorActions.ts: Open, Open File, Open Folder, Open Workspace from
//! File, Close Folder, Open Recent and Clear Recently Opened, with windowsMainService.ts `open` for what they pick.

use std::path::{Path, PathBuf};

use gpui::{Context, PathPromptOptions, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::dialog;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput, path_label};
use crate::recently_opened::{self, Kind as RecentKind};

/// `IWindowOpenable`: a folder or `.code-workspace` file opens as the window's workspace, anything else as a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Openable {
    Workspace(PathBuf),
    File(PathBuf),
}

impl Openable {
    fn of(path: PathBuf) -> Openable {
        match path.is_dir() || path.extension().is_some_and(|extension| extension == "code-workspace") {
            true => Openable::Workspace(path),
            false => Openable::File(path),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Openable::Workspace(path) | Openable::File(path) => path,
        }
    }
}

impl Workbench {
    pub(super) fn register_workspace_commands(&mut self) {
        self.commands.register("workbench.action.files.openFileFolder", |workbench, _, window, cx| workbench.pick_and_open(true, true, window, cx));
        self.commands.register("workbench.action.files.openFile", |workbench, _, window, cx| workbench.pick_and_open(true, false, window, cx));
        self.commands.register("workbench.action.files.openFolder", |workbench, _, window, cx| workbench.pick_and_open(false, true, window, cx));
        // shortcut: GPUI's open panel takes no file type filter, so WORKSPACE_FILTER is not applied.
        self.commands.register("workbench.action.openWorkspace", |workbench, _, window, cx| workbench.pick_and_open(true, false, window, cx));
        self.commands.register("workbench.action.closeFolder", |workbench, _, window, cx| match workbench.workspace() {
            Some(_) => workbench.relaunch(None, window, cx),
            None => Outcome::done(),
        });
        self.commands.register("workbench.action.openRecent", |workbench, _, window, cx| {
            workbench.show_open_recent(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.clearRecentFiles", |workbench, _, window, cx| {
            workbench.clear_recently_opened(window, cx);
            Outcome::done()
        });
    }

    /// The path of the open folder, or of the `.code-workspace` file of a multi-root window.
    pub(super) fn workspace_location(&self) -> Option<PathBuf> {
        self.workspace().map(|workspace| workspace.file.clone().unwrap_or_else(|| workspace.folders[0].path.clone()))
    }

    /// `pickFileFolderAndOpen`, `pickFileAndOpen` and `pickFolderAndOpen` of the native dialogs: what is picked
    /// opens with `forceNewWindow: false`.
    fn pick_and_open(&mut self, files: bool, directories: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let picked = cx.prompt_for_paths(PathPromptOptions { files, directories, multiple: true, prompt: None });
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                workbench.open_window(paths.into_iter().map(Openable::of).collect(), false, window, cx);
            });
        })
        .detach();
        Outcome::done()
    }

    /// windowsMainService.ts `open` from this window: files open as editors here, the first workspace replaces
    /// this window unless files opened in it too, the others get windows of their own. Paths that do not exist
    /// show the "Path does not exist" box; returns whether anything opened.
    pub(super) fn open_window(&mut self, openables: Vec<Openable>, force_new_window: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut opened = false;
        let mut files = Vec::new();
        let mut workspaces = Vec::new();
        for openable in openables {
            if !openable.path().exists() {
                let detail = format!("The path '{}' does not exist on this computer.", path_label(openable.path(), None));
                let _ = dialog::show_message_box(window, PromptLevel::Info, "Path does not exist", Some(&detail), &["OK"], None, cx);
                continue;
            }
            opened = true;
            match openable {
                Openable::File(path) => files.push(path),
                Openable::Workspace(path) => workspaces.push(path),
            }
        }
        // shortcut: a file always opens in this window; the port opens a file in a new window only with a folder.
        if !files.is_empty() {
            let entries = files.iter().map(|path| recently_opened::entry(RecentKind::File, path, None)).collect();
            recently_opened::add(&mut self.shared_storage.lock().unwrap(), entries);
            self.recently_opened_changed(window, cx);
        }
        let mut reuse_window = !force_new_window && files.is_empty();
        for path in files {
            self.open_file(path, true, window, cx);
        }
        let current = self.workspace_location();
        for path in workspaces {
            // A workspace already open in this window is focused, not opened again.
            if Some(&path) == current.as_ref() {
                continue;
            }
            if reuse_window {
                reuse_window = false;
                self.relaunch(Some(path), window, cx);
            } else if let Err(error) = super::host::open_new_window(Some(path)) {
                self.notify_error(&format!("Failed to open a new window: {error}"));
            }
        }
        cx.notify();
        opened
    }

    /// windowActions.ts `OpenRecentAction`: the recent folders and workspaces, then the recent files.
    fn show_open_recent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recent = recently_opened::load(&mut self.shared_storage.lock().unwrap());
        let current = self.workspace_location();
        let has_workspaces = recent.workspaces.iter().any(|entry| recently_opened::kind(entry) == Some(RecentKind::Workspace));
        let folders_label = if has_workspaces { "folders & workspaces" } else { "folders" };
        let mut entries = vec![Entry::Separator(Some(folders_label.into()))];
        let mut picks = Vec::new();
        // Entries of other schemes (remote) are left out: the port opens `file` paths only.
        for entry in recent.workspaces.into_iter().filter(|entry| recently_opened::path(entry).is_some()) {
            entries.push(Entry::Item(recent_pick(&entry, picks.len() as i64, current.as_deref())));
            picks.push(entry);
        }
        let workspace_picks = picks.len();
        entries.push(Entry::Separator(Some("files".into())));
        for entry in recent.files.into_iter().filter(|entry| recently_opened::path(entry).is_some()) {
            entries.push(Entry::Item(recent_pick(&entry, picks.len() as i64, current.as_deref())));
            picks.push(entry);
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select to open (hold Cmd-key to force new window or Option-key for same window)".into());
        input.match_on_description = true;
        input.set_items(entries);
        let first_is_current = workspace_picks > 0 && recently_opened::path(&picks[0]) == current;
        if picks.len() > usize::from(first_is_current) {
            input.set_active_items(&[i64::from(first_is_current)]);
        }
        let id = self.quick_inputs.add(input, Owner::RecentlyOpened(picks));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn recently_opened_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::RecentlyOpened(picks)) = self.quick_inputs.owner(id) else { return };
        match event {
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let Some(&handle) = input.selected_handles().first() else { return };
                let openable = recent_openable(&picks[handle as usize]);
                let force_new_window = window.modifiers().platform;
                self.hide_quick_input_by_id(id, window, cx);
                self.open_window(vec![openable], force_new_window, window, cx);
            }
            // The button of the window's own workspace only tells it is open here; the others remove their entry.
            Event::TriggerItemButton(handle, _, _) => {
                let entry = &picks[handle as usize];
                if recently_opened::path(entry) == self.workspace_location() && recently_opened::kind(entry) != Some(RecentKind::File) {
                    return;
                }
                let uri = recently_opened::location(entry).to_string();
                recently_opened::remove(&mut self.shared_storage.lock().unwrap(), &[&uri]);
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.remove_item(handle);
                }
                self.recently_opened_changed(window, cx);
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `ClearRecentFilesAction`: after the confirmation, the global list and the editor history are cleared.
    fn clear_recently_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = dialog::show_message_box(window, PromptLevel::Warning, "Do you want to clear all recently opened files and workspaces?", Some("This action is irreversible!"), &["Clear", "Cancel"], None, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok((0, _)) = answer.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| {
                recently_opened::clear(&mut workbench.shared_storage.lock().unwrap());
                workbench.clear_editor_history();
                workbench.recently_opened_changed(window, cx);
            });
        })
        .detach();
    }

    /// `onDidChangeRecentlyOpened`: the menu bar's Open Recent entries follow the list.
    pub(super) fn recently_opened_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recently_opened = recently_opened::load(&mut self.shared_storage.lock().unwrap());
        self.update_menubar(window, cx);
    }

    /// menubarControl.ts `onDidChangeWindowFocus`: other windows (processes) change the list, so it is read again
    /// whenever this window gains focus.
    pub fn watch_recently_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recently_opened_changed(window, cx);
        cx.observe_window_activation(window, |workbench, window, cx| {
            if window.is_window_active() {
                workbench.recently_opened_changed(window, cx);
            }
        })
        .detach();
    }
}

pub fn recent_openable(entry: &Value) -> Openable {
    let path = recently_opened::path(entry).expect("picks are `file` entries");
    match recently_opened::kind(entry) {
        Some(RecentKind::File) => Openable::File(path),
        _ => Openable::Workspace(path),
    }
}

/// `toQuickPick`: the label split into name and parent path, and the entry's button.
fn recent_pick(entry: &Value, handle: i64, current: Option<&Path>) -> Item {
    let path = recently_opened::path(entry).expect("picks are `file` entries");
    let kind = recently_opened::kind(entry);
    let full_label = recently_opened::label(entry);
    let (name, parent) = split_recent_label(&full_label);
    let button = match kind {
        Some(RecentKind::File) | None => None,
        Some(kind) if Some(path.as_path()) == current => Some(Button {
            handle: 0,
            codicon: Some("window-active".into()),
            tooltip: Some(if kind == RecentKind::Workspace { "Workspace Opened in Active Window" } else { "Folder Opened in Active Window" }.into()),
            always_visible: true,
            ..Button::default()
        }),
        Some(_) => None,
    };
    let button = button.unwrap_or_else(|| Button { handle: 0, codicon: Some("close".into()), tooltip: Some("Remove from Recently Opened".into()), ..Button::default() });
    Item {
        handle,
        label: name,
        description: Some(parent),
        pickable: true,
        file_icon: (kind == Some(RecentKind::File)).then_some(path),
        buttons: vec![button],
        ..Item::default()
    }
}

/// labels.ts `splitRecentLabel` of a label without a remote suffix: posix `basename` and `dirname`.
pub fn split_recent_label(label: &str) -> (String, String) {
    let trimmed = label.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", name)) => (name.to_string(), "/".to_string()),
        Some((parent, name)) => (name.to_string(), parent.to_string()),
        None if trimmed.is_empty() => (label.to_string(), String::new()),
        None => (trimmed.to_string(), ".".to_string()),
    }
}
