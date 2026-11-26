//! workbench/services/lifecycle and the shutdown handling around it: `onBeforeShutdown` vetoes from window.ts
//! (`window.confirmBeforeClose`) and the native working copy backup tracker (workingCopyBackupTracker.ts: auto save,
//! then the "Do you want to save" confirm), the Quit menu's `confirmBeforeQuit` (menubar.ts), then the shutdown.

use futures::channel::oneshot;
use gpui::{App, AsyncWindowContext, Context, PromptLevel, WeakEntity, Window, WindowHandle};
use serde_json::{Value, json};

use super::Workbench;
use super::actions::{file_names_message, save_confirm_text};
use crate::commands::Outcome;
use crate::dialog;
use crate::windows_state::{self, LastClosedWindow, WindowState};

/// `ShutdownReason`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShutdownReason {
    /// The window closes.
    Close,
    /// The application quits.
    Quit,
    /// The window reloads.
    Reload,
    /// The window loads another workspace.
    Load,
}

#[derive(Default)]
pub struct Lifecycle {
    /// A shutdown waits for its vetoes.
    shutting_down: bool,
    /// The window passed its vetoes and closes now.
    pub(super) window_closed: bool,
    pub(super) backups: super::backup_tracker::BackupTracker,
    /// Answers once the window's extension host session let go of its storage (host.rs).
    pub(super) session_ended: Option<oneshot::Receiver<()>>,
    /// The title, document edited state and represented file last set on the window.
    pub(super) title: String,
    pub(super) document_edited: bool,
    pub(super) represented_filename: Option<std::path::PathBuf>,
}

/// `ConfirmResult` of `showSaveConfirm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmResult {
    Save,
    DontSave,
    Cancel,
}

impl Workbench {
    /// The names of the modified working copies (`workingCopyService.modifiedWorkingCopies`): dirty editors, then
    /// dirty notebooks, whose tabs are not dirty editors.
    fn modified_working_copy_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.dirty_editors().into_iter().map(|index| self.editor_name(&self.editors.editors[index])).collect();
        names.extend(self.host.notebooks.models.iter().filter(|model| model.dirty).map(|model| crate::notebook::label(&model.uri)));
        names
    }
    pub(super) fn register_lifecycle_commands(&mut self) {
        // menubar.ts: the Quit item asks first with `window.confirmBeforeClose`; the command itself does not.
        self.commands.register("workbench.action.quit", |_, args, window, cx| {
            let from_menu = args.first().is_some_and(|options| options["from"] == "menu");
            let keyboard = window.modifiers().modified();
            cx.spawn_in(window, async move |workbench, cx| {
                if from_menu && !confirm_before_quit(&workbench, keyboard, cx).await {
                    return;
                }
                quit_all_windows(cx).await;
            })
            .detach();
            Outcome::done()
        });
        // `CloseWindowAction` / nativeHostService.closeWindow: the window closes as its close button closes it.
        self.commands.register("workbench.action.closeWindow", |workbench, _, window, cx| {
            workbench.close_window(window, cx);
            Outcome::done()
        });
    }

    /// The window's close button asks the lifecycle first, as Electron's `close` event does in windowImpl.ts.
    pub fn watch_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(LastClosedWindow(None));
        let workbench = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = workbench.update(cx, |workbench, cx| {
                if workbench.lifecycle.window_closed {
                    return;
                }
                workbench.close_window(window, cx);
            });
            workbench.upgrade().is_none_or(|workbench| workbench.read(cx).lifecycle.window_closed)
        });
    }

    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shutdown(ShutdownReason::Close, window, cx, |workbench, window, cx| {
            if cx.windows().len() == 1 {
                let state = workbench.window_state(window, cx);
                cx.set_global(LastClosedWindow(Some(state)));
            }
            workbench.lifecycle.window_closed = true;
            window.remove_window();
        });
    }

    /// `lifecycleService.shutdown`: the `onBeforeShutdown` vetoes, then `then` unless one vetoed.
    pub(super) fn shutdown(&mut self, reason: ShutdownReason, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Self>) + 'static) {
        let veto = self.veto_shutdown(reason, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            if veto.await == Ok(false) {
                let _ = workbench.update_in(cx, |workbench, window, cx| {
                    // storageService's `onWillSaveState` on shutdown: a webview's state changes without an editor change.
                    workbench.save_editor_part_state();
                    then(workbench, window, cx)
                });
            }
        })
        .detach();
    }

    /// The `onBeforeShutdown` vetoes of this window: true when one vetoed. A shutdown asked while another runs is
    /// vetoed, one runs at a time.
    fn veto_shutdown(&mut self, reason: ShutdownReason, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        if self.lifecycle.shutting_down {
            let _ = sender.send(true);
            return receiver;
        }
        self.lifecycle.shutting_down = true;
        let keyboard = window.modifiers().modified();
        cx.spawn_in(window, async move |workbench, cx| {
            let veto = before_shutdown(&workbench, reason, keyboard, cx).await;
            let _ = workbench.update(cx, |workbench, _| workbench.lifecycle.shutting_down = false);
            let _ = sender.send(veto);
        })
        .detach();
        receiver
    }

    /// `toWindowState`.
    fn window_state(&self, window: &Window, cx: &App) -> WindowState {
        let workspace = self.workspace();
        WindowState {
            workspace: workspace.and_then(|workspace| workspace.file.clone()),
            folder: workspace.filter(|workspace| workspace.file.is_none()).map(|workspace| workspace.folders[0].path.clone()),
            backup_path: self.lifecycle.backups.home.clone(),
            ui: Some(windows_state::ui_state(window, cx)),
        }
    }

    /// `showSaveConfirm`: Save (Save All for more than one), Don't Save, Cancel; Don't Save without a dialog while
    /// extension tests run.
    pub(super) fn show_save_confirm(&mut self, names: &[String], window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<ConfirmResult> {
        let (sender, receiver) = oneshot::channel();
        if self.skip_dialogs || names.is_empty() {
            let _ = sender.send(ConfirmResult::DontSave);
            return receiver;
        }
        let (message, detail) = save_confirm_text(names);
        let save = if names.len() > 1 { "Save All" } else { "Save" };
        let answer = dialog::show_message_box(window, PromptLevel::Warning, &message, Some(&detail), &[save, "Don't Save", "Cancel"], None, cx);
        cx.background_executor()
            .spawn(async move {
                let result = match answer.await {
                    Ok((0, _)) => ConfirmResult::Save,
                    Ok((1, _)) => ConfirmResult::DontSave,
                    _ => ConfirmResult::Cancel,
                };
                let _ = sender.send(result);
            })
            .detach();
        receiver
    }
}

/// lifecycleMainService.ts `quit`: every window's `onBeforeShutdown` for quit in turn; the app quits unless one vetoes.
async fn quit_all_windows(cx: &mut AsyncWindowContext) {
    let windows = cx.update(|_, cx| {
        let mut windows: Vec<WindowHandle<Workbench>> = cx.windows().into_iter().filter_map(|handle| handle.downcast::<Workbench>()).collect();
        windows.sort_by_key(|handle| handle.window_id().as_u64());
        windows
    });
    for handle in windows.unwrap_or_default() {
        let Ok(veto) = handle.update(cx, |workbench, window, cx| workbench.veto_shutdown(ShutdownReason::Quit, window, cx)) else { continue };
        if veto.await.unwrap_or(true) {
            return;
        }
    }
    let _ = cx.update(|_, cx| {
        save_windows_state(cx);
        cx.quit();
    });
}

/// windowsStateHandler.ts `saveWindowsState`: the last window closed, else the active window, else the first; all
/// windows when there are two or more.
fn save_windows_state(cx: &mut App) {
    let mut last_active = cx.try_global::<LastClosedWindow>().and_then(|closed| closed.0.clone());
    let mut active = None;
    let mut opened = Vec::new();
    for handle in cx.windows().into_iter().filter_map(|handle| handle.downcast::<Workbench>()) {
        let _ = handle.update(cx, |workbench, window, cx| {
            if workbench.extension_development {
                return;
            }
            let state = workbench.window_state(window, cx);
            if window.is_window_active() {
                active = Some(state.clone());
            }
            opened.push(state);
        });
    }
    last_active = last_active.or(active).or_else(|| opened.first().cloned());
    if opened.len() < 2 {
        opened.clear();
    }
    let user_data = cx.global::<crate::AppState>().user_data.clone();
    windows_state::save(&user_data, last_active.as_ref(), &opened);
}

/// The `onBeforeShutdown` participants in order; true when one vetoes.
async fn before_shutdown(workbench: &WeakEntity<Workbench>, reason: ShutdownReason, keyboard: bool, cx: &mut AsyncWindowContext) -> bool {
    if reason == ShutdownReason::Close && !confirm_before_close(workbench, keyboard, cx).await {
        return true;
    }
    if super::workspace_editing::save_untitled_before_shutdown(workbench, reason, cx).await {
        return true;
    }
    let _ = workbench.update(cx, |workbench, _| workbench.suspend_backups(true));
    let veto = backup_tracker_before_shutdown(workbench, reason, cx).await;
    let _ = workbench.update(cx, |workbench, _| workbench.suspend_backups(false));
    veto
}

/// window.ts `onBeforeShutdown` with `confirmOnShutdown`: with `window.confirmBeforeClose` `always`, or
/// `keyboardOnly` and a modifier held, asks to close the window; "Do not ask me again" sets it to `never`.
async fn confirm_before_close(workbench: &WeakEntity<Workbench>, keyboard: bool, cx: &mut AsyncWindowContext) -> bool {
    let asked = workbench.update_in(cx, |workbench, window, cx| {
        if !asks_before_close(&workbench.setting("window.confirmBeforeClose"), keyboard) {
            return None;
        }
        Some(dialog::show_message_box(window, PromptLevel::Info, "Are you sure you want to close the window?", None, &["Close Window", "Cancel"], Some("Do not ask me again"), cx))
    });
    let Ok(Some(answer)) = asked else { return true };
    let Ok((button, checked)) = answer.await else { return false };
    if button == 0 && checked {
        let _ = workbench.update(cx, |workbench, _| workbench.update_setting("window.confirmBeforeClose", json!("never")));
    }
    button == 0
}

/// menubar.ts `confirmBeforeQuit`.
async fn confirm_before_quit(workbench: &WeakEntity<Workbench>, keyboard: bool, cx: &mut AsyncWindowContext) -> bool {
    let asked = workbench.update_in(cx, |workbench, window, cx| {
        if !asks_before_close(&workbench.setting("window.confirmBeforeClose"), keyboard) {
            return None;
        }
        Some(dialog::show_message_box(window, PromptLevel::Info, "Are you sure you want to quit?", None, &["Quit", "Cancel"], None, cx))
    });
    let Ok(Some(answer)) = asked else { return true };
    matches!(answer.await, Ok((0, _)))
}

fn asks_before_close(setting: &Value, keyboard: bool) -> bool {
    setting == "always" || (setting == "keyboardOnly" && keyboard)
}

/// NativeWorkingCopyBackupTracker `onFinalBeforeShutdown`: auto saves what auto save covers, then backs the rest
/// up for hot exit or asks about it; without modified editors the window's backups go.
async fn backup_tracker_before_shutdown(workbench: &WeakEntity<Workbench>, reason: ShutdownReason, cx: &mut AsyncWindowContext) -> bool {
    let Ok(auto_saves) = workbench.update(cx, |workbench, cx| {
        let indices: Vec<usize> = workbench.dirty_editors().into_iter().filter(|index| workbench.auto_saved_on_shutdown(&workbench.editors.editors[*index])).collect();
        indices.into_iter().map(|index| workbench.save_editor(index, cx)).collect::<Vec<_>>()
    }) else {
        return false;
    };
    // An error saving is left to the steps below, as `doSaveAllBeforeShutdown` errors are only logged.
    futures::future::join_all(auto_saves).await;
    // `joinBackups`: a backup still being written lands before this one or the discard below.
    let Ok(operations) = workbench.update(cx, |workbench, _| workbench.lifecycle.backups.take_operations()) else { return false };
    futures::future::join_all(operations).await;
    let backed_up = workbench.update(cx, |workbench, cx| {
        if workbench.modified_working_copy_names().is_empty() {
            workbench.discard_all_backups();
            return Some((true, Vec::new()));
        }
        workbench.backs_up_on_shutdown(reason).then(|| (workbench.backup_before_shutdown(), workbench.backup_documents_before_shutdown(cx)))
    });
    let backed_up = match backed_up {
        Ok(Some((text, custom))) => Some(text && futures::future::join_all(custom).await.iter().all(|backup| matches!(backup, Ok(Some(_))))),
        Ok(None) => None,
        Err(_) => Some(true),
    };
    match backed_up {
        Some(true) => false,
        Some(false) => backup_failed(workbench, reason, cx).await,
        None => confirm_before_shutdown(workbench, cx).await,
    }
}

/// `showErrorDialog` after a failed backup: OK vetoes, "Quit Anyway" (or Close, Reload) goes on.
async fn backup_failed(workbench: &WeakEntity<Workbench>, reason: ShutdownReason, cx: &mut AsyncWindowContext) -> bool {
    let force = match reason {
        ShutdownReason::Close | ShutdownReason::Load => "Close Anyway",
        ShutdownReason::Quit => "Quit Anyway",
        ShutdownReason::Reload => "Reload Anyway",
    };
    let answer = workbench.update_in(cx, |workbench, window, cx| {
        // Extension development never blocks a shutdown on backups.
        if workbench.extension_development {
            return None;
        }
        let names = workbench.modified_working_copy_names();
        let advice = "Try saving or reverting the editors with unsaved changes first and then try again.";
        let detail = format!("{}\n{advice}", file_names_message(&names));
        Some(dialog::show_message_box(window, PromptLevel::Critical, "The following editors with unsaved changes could not be saved to the backup location.", Some(&detail), &["OK", force], None, cx))
    });
    let Ok(Some(answer)) = answer else { return false };
    !matches!(answer.await, Ok((1, _)))
}


/// `confirmBeforeShutdown`: Save saves every modified editor and vetoes unless all saved, Don't Save reverts them,
/// Cancel vetoes.
async fn confirm_before_shutdown(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext) -> bool {
    let Ok(confirm) = workbench.update_in(cx, |workbench, window, cx| {
        let names = workbench.modified_working_copy_names();
        workbench.show_save_confirm(&names, window, cx)
    }) else {
        return false;
    };
    match confirm.await.unwrap_or(ConfirmResult::Cancel) {
        ConfirmResult::Save => {
            let Ok(saves) = workbench.update(cx, |workbench, cx| {
                let mut saves: Vec<_> = workbench.dirty_editors().into_iter().map(|index| workbench.save_editor(index, cx)).collect();
                saves.extend(workbench.save_dirty_notebook_editors(cx));
                saves
            }) else {
                return false;
            };
            let saved = futures::future::join_all(saves).await;
            !saved.into_iter().all(|saved| saved == Ok(true))
        }
        ConfirmResult::DontSave => {
            let _ = workbench.update(cx, |workbench, cx| {
                for index in workbench.dirty_editors() {
                    workbench.revert_editor(index, cx);
                }
                workbench.revert_dirty_notebooks(cx);
            });
            false
        }
        ConfirmResult::Cancel => true,
    }
}

#[cfg(test)]
mod tests;
