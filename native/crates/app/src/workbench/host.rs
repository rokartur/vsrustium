//! workbench/services/host (hostService.ts, nativeHostService.ts, windowsMainService.ts): new windows, the window of
//! a folder or file, `reload`, a workspace replacing this window's, switching windows, and the window's extension host
//! ending with it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::channel::oneshot;
use gpui::{App, Context, Window, WindowHandle};
use objc2_app_kit::{NSFloatingWindowLevel, NSNormalWindowLevel};
use objc2_foundation::NSString;
use serde_json::Value;

use super::Workbench;
use super::lifecycle::ShutdownReason;
use super::workspace_actions::Openable;
use crate::commands::Outcome;
use crate::main_thread_quick_open::Owner;
use crate::quick_input::{Button, Entry, Event, Item, Kind, QuickInput};
use crate::{CliProfile, WindowRequest, open_window_or_log};

impl Workbench {
    pub(super) fn register_host_commands(&mut self) {
        self.commands.register("workbench.action.reloadWindow", |workbench, _, window, cx| {
            let folder = workbench.workspace_location();
            workbench.relaunch(ShutdownReason::Reload, folder, window, cx)
        });
        // windowActions.ts `NewWindowAction` (Shift+Cmd+N): an empty window.
        self.commands.register("workbench.action.newWindow", |_, _, _, cx| {
            open_new_window(None, cx);
            Outcome::done()
        });
        // windowActions.ts `ToggleFullScreenAction` (Ctrl+Cmd+F, View > Appearance > Full Screen).
        self.commands.register("workbench.action.toggleFullScreen", |_, _, window, _| {
            window.toggle_fullscreen();
            Outcome::done()
        });
        // electron-browser windowActions.ts `ToggleWindowAlwaysOnTopAction`: Electron's floating window level.
        self.commands.register("workbench.action.toggleWindowAlwaysOnTop", |_, _, window, _| {
            let ns_window = crate::dialog::ns_window(window);
            let level = if ns_window.level() == NSFloatingWindowLevel { NSNormalWindowLevel } else { NSFloatingWindowLevel };
            ns_window.setLevel(level);
            Outcome::done()
        });
        self.commands.register("workbench.action.switchWindow", |workbench, _, window, cx| {
            workbench.show_switch_window(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.quickSwitchWindow", |workbench, _, window, cx| {
            workbench.show_switch_window(window, cx);
            Outcome::done()
        });
    }

    /// contextkeys.ts `IsMainWindowFullscreenContext`.
    pub(super) fn host_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        context.insert("isFullscreen".into(), Value::Bool(window.is_fullscreen()));
        let always_on_top = crate::dialog::app_kit_window(window).is_some_and(|ns_window| ns_window.level() == NSFloatingWindowLevel);
        context.insert("isWindowAlwaysOnTop".into(), Value::Bool(always_on_top));
    }

    /// window.ts `vscode:showResolveShellEnvError`: the error of resolving the login shell's environment at startup.
    pub fn show_resolve_shell_env_error(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let reply = self.prompt_notification(crate::notifications::Severity::Error, message, &["Restart", "Configure", "Learn More"]);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(choice)) = reply.await else { return };
            let _ = workbench.update_in(cx, |workbench, window, cx| match choice.as_u64() {
                Some(0) => super::relauncher::relaunch(cx),
                Some(1) => drop(workbench.execute_command("workbench.action.openSettings", vec![serde_json::json!({ "query": "application.shellEnvironmentResolutionTimeout" })], window, cx)),
                Some(2) => cx.open_url("https://go.microsoft.com/fwlink/?linkid=2149667"),
                _ => {}
            });
        })
        .detach();
    }

    /// The window's extension host session ends once the workbench is released (its window closed); `ended` answers
    /// when the session thread is done with the window's storage.
    pub fn watch_session_end(&mut self, closed: Arc<AtomicBool>, ended: oneshot::Receiver<()>, cx: &mut Context<Self>) {
        cx.on_release(move |_, _| closed.store(true, Ordering::Relaxed)).detach();
        self.lifecycle.session_ended = Some(ended);
    }

    /// `reload` and opening a workspace in this window: after the shutdown vetoes this window closes, and once its
    /// session let go of the workspace storage a window opens on `folder` (a folder or workspace file) in the same
    /// place, as the reference renderer loads again in its BrowserWindow. A reloaded empty window keeps its backups.
    pub(super) fn relaunch(&mut self, reason: ShutdownReason, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.shutdown(reason, window, cx, move |workbench, window, cx| {
            let ended = workbench.lifecycle.session_ended.take();
            let backup_folder = workbench.lifecycle.backups.home.clone().filter(|_| reason == ShutdownReason::Reload && folder.is_none());
            // A reload keeps the window's command line profile; another folder in this window resolves its own.
            let profile = if reason == ShutdownReason::Reload { workbench.cli_profile.clone() } else { Default::default() };
            let request = WindowRequest { workspace: folder.map(config::workspace::Workspace::open), backup_folder, ui: Some(crate::windows_state::ui_state(window, cx)), profile, ..WindowRequest::default() };
            workbench.lifecycle.window_closed = true;
            window.remove_window();
            cx.spawn(async move |_, cx| {
                if let Some(ended) = ended {
                    let _ = ended.await;
                }
                let _ = cx.update(|cx| open_window_or_log(request, cx));
            })
            .detach();
        });
        Outcome::done()
    }

    /// The window's title on its NSWindow, as Electron sets the document title on the BrowserWindow: the Window menu
    /// and the window switcher show it.
    /// window.ts `updateDocumentEdited` and `updateRepresentedFilename` on macOS too: the close button's dot while
    /// a working copy is dirty, and the active editor's file as the window's represented file.
    pub(super) fn sync_window_title(&mut self, window: &mut Window) {
        let title = self.window_title();
        if self.lifecycle.title != title {
            window.set_window_title(&title);
            self.lifecycle.title = title;
        }
        let edited = !self.dirty_editors().is_empty();
        let represented = self.editors.active().filter(|editor| editor.document().scheme == "file").map(|editor| editor.document().path.clone());
        if self.lifecycle.document_edited == edited && self.lifecycle.represented_filename == represented {
            return;
        }
        if let Some(ns_window) = crate::dialog::app_kit_window(window) {
            ns_window.setDocumentEdited(edited);
            ns_window.setRepresentedFilename(&NSString::from_str(&represented.as_deref().map(|path| path.to_string_lossy()).unwrap_or_default()));
        }
        self.lifecycle.document_edited = edited;
        self.lifecycle.represented_filename = represented;
    }

    /// windowActions.ts `BaseSwitchWindow`: the open windows by title with their file's icon, this one described as the
    /// current window, the next one active; the picked one comes to the front and each row's button closes its window.
    fn show_switch_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = window.window_handle().window_id();
        let mut windows: Vec<WindowHandle<Workbench>> = cx.windows().into_iter().filter_map(|handle| handle.downcast::<Workbench>()).collect();
        windows.sort_by_key(|handle| handle.window_id().as_u64());
        let mut entries = Vec::new();
        let mut active = 0;
        for (index, handle) in windows.iter().enumerate() {
            let is_current = handle.window_id() == current;
            let (title, filename, dirty) = match is_current {
                true => (self.window_title(), self.lifecycle.represented_filename.clone(), self.lifecycle.document_edited),
                false => handle.read(cx).map(|workbench| (workbench.window_title(), workbench.lifecycle.represented_filename.clone(), workbench.lifecycle.document_edited)).unwrap_or_default(),
            };
            if is_current && index + 1 < windows.len() {
                active = index as i64 + 1;
            }
            let button = match (dirty, is_current) {
                (true, _) => Button { codicon: Some("close-dirty".into()), tooltip: Some("Close Window".into()), always_visible: true, ..Button::default() },
                (false, true) => Button { codicon: Some("window-active".into()), tooltip: Some("Close Active Window".into()), always_visible: true, ..Button::default() },
                (false, false) => Button { codicon: Some("remove-close".into()), tooltip: Some("Close Window".into()), ..Button::default() },
            };
            entries.push(Entry::Item(Item {
                handle: index as i64,
                label: title,
                description: is_current.then(|| "Current Window".to_owned()),
                file_icon: filename,
                buttons: vec![button],
                ..Item::default()
            }));
        }
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.placeholder = Some("Select a window to switch to".into());
        input.set_items(entries);
        input.set_active_items(&[active]);
        let id = self.quick_inputs.add(input, Owner::SwitchWindow(windows));
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub fn switch_window_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Owner::SwitchWindow(windows)) = self.quick_inputs.owner(id) else { return };
        let windows = windows.clone();
        match event {
            Event::Accept(_) => {
                let Some(handle) = self.quick_inputs.get(id).and_then(|input| input.selected_handles().first().copied()) else { return };
                self.hide_quick_input_by_id(id, window, cx);
                let target = windows[handle as usize];
                cx.defer(move |cx| _ = target.update(cx, |_, window, _| window.activate_window()));
            }
            Event::TriggerItemButton(handle, _, _) => {
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.remove_item(handle);
                }
                let target = windows[handle as usize];
                if target.window_id() == window.window_handle().window_id() {
                    self.hide_quick_input_by_id(id, window, cx);
                    _ = self.execute_command("workbench.action.closeWindow", vec![], window, cx);
                    return;
                }
                cx.defer(move |cx| _ = target.update(cx, |workbench, window, cx| _ = workbench.execute_command("workbench.action.closeWindow", vec![], window, cx)));
                cx.notify();
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }
}

/// launchMainService.ts `startOpenWindow` for a second instance's command line: without paths the last active window
/// comes to the front (the macOS default of `window.openWithoutArgumentsInNewWindow`), or an empty window opens with
/// `on` or `--new-window`; folders and workspaces open as in `open_new_window`, files in the last active window.
pub(crate) fn start_open_window(args: Vec<String>, cwd: PathBuf, cx: &mut App) {
    let paths: Vec<PathBuf> = crate::positional_indexes(&args).into_iter().map(|index| cwd.join(&args[index])).filter(|path| path.exists()).collect();
    let profile = CliProfile::from_args(&args);
    let new_window = args.iter().any(|arg| arg == "--new-window" || arg == "-n");
    let last_active = cx.window_stack().unwrap_or_else(|| cx.windows()).into_iter().find_map(|window| window.downcast::<Workbench>()).filter(|_| !new_window);
    if paths.is_empty() {
        let in_new_window = profile.is_forced() || last_active.and_then(|handle| handle.read(cx).ok()).is_none_or(|workbench| workbench.setting("window.openWithoutArgumentsInNewWindow") == "on");
        match last_active.filter(|_| !in_new_window) {
            Some(handle) => drop(handle.update(cx, |_, window, _| window.activate_window())),
            None => open_command_line_window(None, profile, cx),
        }
        return;
    }
    for path in paths {
        match (Openable::of(path), last_active) {
            (Openable::File(path), Some(handle)) => drop(handle.update(cx, |workbench, window, cx| {
                workbench.open_file(path, true, window, cx);
                window.activate_window();
            })),
            (openable, _) => open_command_line_window(Some(openable.path().to_owned()), profile.clone(), cx),
        }
    }
}

/// windowsMainService.ts `open` with `forceNewWindow`: a folder or `.code-workspace` file opens as a new window's
/// workspace unless a window has it open already (that one comes to the front), any other path as an empty window's
/// file, nothing as an empty window. Opened after the caller's update.
pub(super) fn open_new_window(path: Option<PathBuf>, cx: &mut App) {
    open_command_line_window(path, CliProfile::default(), cx);
}

/// `open_new_window` for a command line, with its `--profile` or `--profile-temp`.
fn open_command_line_window(path: Option<PathBuf>, profile: CliProfile, cx: &mut App) {
    let mut request = WindowRequest { profile, ..WindowRequest::default() };
    match path.map(Openable::of) {
        Some(Openable::Workspace(path)) => request.workspace = Some(config::workspace::Workspace::open(path)),
        Some(Openable::File(path)) => request.file = Some(path),
        None => {}
    }
    cx.defer(move |cx| {
        let location = request.workspace.as_ref().map(|workspace| workspace.file.clone().unwrap_or_else(|| workspace.folders[0].path.clone()));
        let windows = cx.windows().into_iter().filter_map(|handle| handle.downcast::<Workbench>());
        let existing = windows.filter(|_| location.is_some()).find(|handle| handle.read(cx).is_ok_and(|workbench| workbench.workspace_location() == location));
        match existing {
            Some(handle) => drop(handle.update(cx, |_, window, _| window.activate_window())),
            None => open_window_or_log(request, cx),
        }
    });
}

#[cfg(test)]
mod tests;
