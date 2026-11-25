//! Workbench actions in the command registry under their upstream ids: close, save and revert of the
//! editor group (editorActions.ts, fileActions.ts, fileCommands.ts).

use futures::channel::oneshot;
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::host_requests::uri_parts;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use crate::commands::Outcome;
use crate::host_sync::tab_id;

/// `MAX_CONFIRM_FILES` of platform/dialogs/common/dialogs.ts.
const MAX_CONFIRM_FILES: usize = 10;
/// editorService.ts `ACTIVE_GROUP`, as a `viewColumn` of `$moveTab`.
const ACTIVE_GROUP_COLUMN: i64 = -1;

impl Workbench {
    pub(super) fn register_workbench_actions(&mut self) {
        self.commands.register("workbench.action.files.save", |workbench, _, _, cx| {
            let saves = workbench.editors.active_index().map(|index| workbench.save_editor(index, cx));
            when_saved(saves.into_iter().collect(), |_| Value::Null, cx)
        });
        // `saveAll` in fileCommands.ts (Alt+Cmd+S), the next in openEditorsView.ts, `saveFiles` in fileCommands.ts.
        for id in ["saveAll", "workbench.action.files.saveAll", "workbench.action.files.saveFiles"] {
            self.commands.register(id, |workbench, _, _, cx| {
                let saves = workbench.dirty_editors().into_iter().map(|index| workbench.save_editor(index, cx)).collect();
                when_saved(saves, |_| Value::Null, cx)
            });
        }
        self.commands.register("workbench.action.toggleAutoSave", |workbench, _, _, cx| {
            workbench.toggle_auto_save();
            cx.notify();
            Outcome::done()
        });
        self.commands.register("workbench.action.files.revert", |workbench, _, _, cx| {
            if let Some(index) = workbench.editors.active_index() {
                workbench.revert_and_close_untitled(vec![index], cx);
            }
            Outcome::done()
        });
        self.commands.register("_workbench.open", |workbench, args, window, cx| match &args[..] {
            [resource @ Value::Object(_), rest @ ..] => {
                let options = rest.first().map_or(Value::Null, |column_and_options| column_and_options[1].clone());
                let override_id = options["override"].as_str().map(str::to_owned);
                workbench.open_with_resolver(resource, override_id.as_deref(), options, window, cx)
            }
            _ => Outcome::Ready(workbench.api_open_editor(&args, window, cx)),
        });
        self.commands.register("_workbench.openWith", |workbench, args, window, cx| {
            // `{ pinned: true, ...optionsArg }`
            let mut options = json!({ "pinned": true });
            if let Some(Value::Object(given)) = args.get(2).map(|column_and_options| &column_and_options[1]) {
                options.as_object_mut().expect("an object").extend(given.clone());
            }
            let resource = args.first().cloned().unwrap_or(Value::Null);
            let view_type = args.get(1).and_then(Value::as_str).unwrap_or("default").to_owned();
            workbench.open_with_resolver(&resource, Some(&view_type), options, window, cx)
        });
        self.commands.register("reopenActiveEditorWith", |workbench, args, window, cx| {
            workbench.reopen_active_editor_with(args.first().and_then(Value::as_str).map(str::to_owned), window, cx)
        });
        self.commands.register("workbench.action.reopenWithEditor", |workbench, _, window, cx| workbench.reopen_active_editor_with(None, window, cx));
        self.commands.register("vscode.open", |workbench, args, window, cx| workbench.execute_command("_workbench.open", args.into_iter().take(1).collect(), window, cx));
        self.commands.register("_workbench.revertAllDirty", |workbench, _, _, cx| {
            let dirty = workbench.dirty_editors();
            workbench.revert_and_close_untitled(dirty, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.closeAllEditors", |workbench, _, window, cx| workbench.close_all_editors(window, cx));
        // The port has one editor group, so closing the group's editors closes them all.
        self.commands.register("workbench.action.closeEditorsInGroup", |workbench, _, window, cx| workbench.close_all_editors(window, cx));
        // editorCommands.ts: these act on the editor a tab's context menu passes, else the active one.
        self.commands.register("workbench.action.closeActiveEditor", |workbench, args, window, cx| {
            let Some(index) = workbench.context_editor(&args) else { return Outcome::done() };
            workbench.close_editors(vec![index], window, cx)
        });
        self.commands.register("workbench.action.closeOtherEditors", |workbench, args, window, cx| {
            let Some(index) = workbench.context_editor(&args) else { return Outcome::done() };
            let others = (0..workbench.editors.editors.len()).filter(|other| *other != index).collect();
            workbench.close_editors(others, window, cx)
        });
        self.commands.register("workbench.action.closeEditorsToTheRight", |workbench, args, window, cx| {
            let Some(index) = workbench.context_editor(&args) else { return Outcome::done() };
            let right = (index + 1..workbench.editors.editors.len()).collect();
            workbench.close_editors(right, window, cx)
        });
        // `closeEditors({ savedOnly: true })`: dirty editors stay, so nothing asks.
        self.commands.register("workbench.action.closeUnmodifiedEditors", |workbench, _, window, cx| {
            let saved = (0..workbench.editors.editors.len()).filter(|index| !workbench.editors.editors[*index].is_dirty()).collect();
            workbench.close_editors(saved, window, cx)
        });
        self.commands.register("workbench.action.keepEditor", |workbench, args, _, cx| {
            if let Some(index) = workbench.context_editor(&args) {
                workbench.editors.pin(index);
                workbench.editors_changed(cx);
            }
            Outcome::done()
        });
        // shortcut: dirty editors ask to save as with `files.hotExit: off`, since the port has no backups to restore
        // them from; quit without asking once hot exit lands.
        self.commands.register("workbench.action.quit", |workbench, _, window, cx| {
            let closed = workbench.close_all_editors(window, cx);
            cx.spawn(async move |workbench, cx| {
                if let Outcome::Pending(pending) = closed {
                    let _ = pending.await;
                }
                let _ = workbench.update(cx, |workbench, cx| {
                    if workbench.editors.editors.is_empty() {
                        cx.quit();
                    }
                });
            })
            .detach();
            Outcome::done()
        });
    }

    /// mainThreadWorkspace.ts `$save`: the editors of `uri` saved even when clean (`force`); the URI once saved.
    pub(crate) fn save_resource(&mut self, uri: &Value, cx: &mut Context<Self>) -> Outcome {
        let Ok((scheme, path)) = uri_parts(uri) else { return Outcome::done() };
        let indexes: Vec<usize> = (0..self.editors.editors.len()).filter(|index| self.editors.editors[*index].document.is(scheme, &path)).collect();
        let saves: Vec<_> = indexes.into_iter().map(|index| self.save_editor(index, cx)).collect();
        let uri = uri.clone();
        let saved_any = !saves.is_empty();
        when_saved(saves, move |all_saved| if all_saved && saved_any { uri } else { Value::Null }, cx)
    }

    /// mainThreadWorkspace.ts `$saveAll`: the dirty editors, untitled ones only with `include_untitled`; true once
    /// every one is saved.
    pub(crate) fn save_all(&mut self, include_untitled: bool, cx: &mut Context<Self>) -> Outcome {
        let dirty: Vec<usize> = self.dirty_editors().into_iter().filter(|index| include_untitled || self.editors.editors[*index].document.scheme != "untitled").collect();
        let saves = dirty.into_iter().map(|index| self.save_editor(index, cx)).collect();
        when_saved(saves, Value::Bool, cx)
    }

    fn dirty_editors(&self) -> Vec<usize> {
        (0..self.editors.editors.len()).filter(|index| self.editors.editors[*index].is_dirty()).collect()
    }

    pub(super) fn close_all_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.close_editors((0..self.editors.editors.len()).collect(), window, cx)
    }

    /// mainThreadEditorTabs.ts `$moveTab`: the tab moves inside its group and is pinned; `index` past the tabs
    /// moves it last.
    pub(crate) fn move_tab(&mut self, id: &str, index: i64, view_column: i64, cx: &mut Context<Self>) -> Outcome {
        let Some(from) = self.editors.editors.iter().position(|editor| tab_id(editor) == id) else {
            return Outcome::error(format!("Attempted to close tab with id {id} which does not exist"));
        };
        // shortcut: one editor group until milestone 11; a move to another view column (a new group upstream) does nothing.
        if !matches!(view_column, 0 | ACTIVE_GROUP_COLUMN) {
            return Outcome::done();
        }
        let len = self.editors.editors.len() as i64;
        let index = if index < 0 || index > len { len } else { index };
        self.editors.move_editor(from, index as usize);
        self.editors_changed(cx);
        Outcome::done()
    }

    /// mainThreadEditorTabs.ts `$closeTab`: the tabs named by `tab_ids` close as `closeEditors` closes them, dirty
    /// ones after the save prompt; true when every one of them closed.
    pub(crate) fn close_tabs(&mut self, tab_ids: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let indices: Vec<usize> = (0..self.editors.editors.len()).filter(|index| tab_ids.iter().any(|id| *id == tab_id(&self.editors.editors[*index]))).collect();
        self.close_and_report(indices, window, cx)
    }

    /// mainThreadEditorTabs.ts `$closeGroup`: the one group closes all its editors and stays, as the last group does.
    pub(crate) fn close_groups(&mut self, group_ids: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if !group_ids.iter().any(|id| *id == 0) {
            return Outcome::Ready(Ok(Value::Bool(true)));
        }
        self.close_and_report((0..self.editors.editors.len()).collect(), window, cx)
    }

    /// `close_editors`, answering whether every editor at `indices` closed (a Cancel keeps them).
    fn close_and_report(&mut self, indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let models: Vec<u32> = indices.iter().map(|index| self.editors.editors[*index].model_id).collect();
        let all_closed = move |workbench: &Workbench| !workbench.editors.editors.iter().any(|editor| models.contains(&editor.model_id));
        match self.close_editors(indices, window, cx) {
            Outcome::Ready(_) => Outcome::Ready(Ok(Value::Bool(all_closed(self)))),
            Outcome::Pending(closing) => {
                let (sender, receiver) = oneshot::channel();
                cx.spawn(async move |workbench, cx| {
                    let _ = closing.await;
                    let closed = workbench.read_with(cx, |workbench, _| all_closed(workbench)).unwrap_or(false);
                    let _ = sender.send(Ok(Value::Bool(closed)));
                })
                .detach();
                Outcome::Pending(receiver)
            }
        }
    }

    /// `AbstractCloseAllAction.run` and `handleDirtyClosing`: dirty editors among `indices` ask to save first;
    /// Cancel keeps every editor open.
    pub(super) fn close_editors(&mut self, indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let dirty: Vec<usize> = indices.iter().copied().filter(|index| self.editors.editors[*index].is_dirty()).collect();
        // showSaveConfirm answers Don't Save without a dialog while extension tests run.
        if dirty.is_empty() || self.skip_dialogs {
            self.discard_and_close(&dirty, &indices, cx);
            return Outcome::done();
        }
        let names: Vec<String> = dirty.iter().map(|index| self.editor_name(&self.editors.editors[*index])).collect();
        let (message, detail) = save_confirm_text(&names);
        let save = if names.len() > 1 { "Save All" } else { "Save" };
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &[save, "Don't Save", "Cancel"], cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let answer = answer.await;
            let _ = workbench.update(cx, |workbench, cx| match answer {
                Ok(0) => {
                    for index in &dirty {
                        drop(workbench.save_editor(*index, cx));
                    }
                    workbench.discard_and_close(&[], &indices, cx);
                }
                Ok(1) => workbench.discard_and_close(&dirty, &indices, cx),
                _ => {}
            });
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// Reverts the editors at `indices` and closes the untitled ones, whose inputs a revert disposes (untitledTextEditorInput.ts).
    fn revert_and_close_untitled(&mut self, mut indices: Vec<usize>, cx: &mut Context<Self>) {
        indices.sort_unstable();
        let mut closed = false;
        for index in indices.into_iter().rev() {
            if self.revert_editor(index, cx) {
                self.editors.close(index);
                closed = true;
            }
        }
        if closed {
            self.editors_changed(cx);
        }
    }

    /// Reverts `dirty` (ConfirmResult.DONT_SAVE), then closes the editors at `indices`.
    fn discard_and_close(&mut self, dirty: &[usize], indices: &[usize], cx: &mut Context<Self>) {
        for index in dirty {
            self.revert_editor(*index, cx);
        }
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        for index in indices.into_iter().rev() {
            self.editors.close(index);
        }
        self.editors_changed(cx);
    }

    /// `editorService.revert` of one editor: a file's model reads its file again and is clean; an untitled model
    /// is emptied and answers true: its input is disposed (untitledTextEditorInput.ts), so the caller closes it.
    pub(super) fn revert_editor(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let document = &self.editors.editors[index].document;
        let (scheme, path) = (document.scheme.clone(), document.path.clone());
        match scheme.as_str() {
            SEARCH_EDITOR_SCHEME => {
                self.revert_search_editor(index, cx);
                false
            }
            "untitled" => {
                self.revert_untitled(&path);
                true
            }
            _ => {
                if let Err(error) = self.revert_text_file(&scheme, &path, cx) {
                    self.notify_error(error);
                }
                false
            }
        }
    }
}

/// The command's answer once every save is done; `answer` gets whether all of them succeeded.
pub(super) fn when_saved(saves: Vec<oneshot::Receiver<bool>>, answer: impl FnOnce(bool) -> Value + Send + 'static, cx: &mut Context<Workbench>) -> Outcome {
    let (sender, receiver) = oneshot::channel();
    cx.background_executor()
        .spawn(async move {
            let results = futures::future::join_all(saves).await;
            let _ = sender.send(Ok(answer(results.into_iter().all(|saved| saved == Ok(true)))));
        })
        .detach();
    Outcome::Pending(receiver)
}

/// `doShowSaveConfirm` and `getFileNamesMessage`.
fn save_confirm_text(names: &[String]) -> (String, String) {
    let detail = "Your changes will be lost if you don't save them.";
    if let [name] = names {
        return (format!("Do you want to save the changes you made to {name}?"), detail.into());
    }
    let mut lines: Vec<String> = names.iter().take(MAX_CONFIRM_FILES).cloned().collect();
    match names.len().saturating_sub(MAX_CONFIRM_FILES) {
        0 => {}
        1 => lines.push("...1 additional file not shown".into()),
        more => lines.push(format!("...{more} additional files not shown")),
    }
    lines.push(String::new());
    (format!("Do you want to save the changes to the following {} files?", names.len()), format!("{}\n{detail}", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::save_confirm_text;

    #[test]
    fn save_confirm_names_one_file_or_lists_up_to_ten() {
        assert_eq!(save_confirm_text(&["a.txt".into()]).0, "Do you want to save the changes you made to a.txt?");
        let names: Vec<String> = (0..12).map(|index| format!("{index}.txt")).collect();
        let (message, detail) = save_confirm_text(&names);
        assert_eq!(message, "Do you want to save the changes to the following 12 files?");
        assert!(detail.starts_with("0.txt\n1.txt\n"));
        assert!(detail.ends_with("9.txt\n...2 additional files not shown\n\nYour changes will be lost if you don't save them."));
    }
}
