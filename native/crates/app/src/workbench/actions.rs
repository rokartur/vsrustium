//! Workbench actions in the command registry under their upstream ids: close, save and revert of the
//! editor group (editorActions.ts, fileActions.ts, fileCommands.ts).

use gpui::{Context, PromptLevel, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{EditReason, Position, TextDocument};

/// `MAX_CONFIRM_FILES` of platform/dialogs/common/dialogs.ts.
const MAX_CONFIRM_FILES: usize = 10;

impl Workbench {
    pub(super) fn register_workbench_actions(&mut self) {
        self.commands.register("workbench.action.files.save", |workbench, _, _, cx| {
            if let Some(index) = workbench.editors.active_index() {
                workbench.save_editor(index, cx);
            }
            Outcome::done()
        });
        // `saveAll` in fileCommands.ts (Alt+Cmd+S), the next in openEditorsView.ts, `saveFiles` in fileCommands.ts.
        for id in ["saveAll", "workbench.action.files.saveAll", "workbench.action.files.saveFiles"] {
            self.commands.register(id, |workbench, _, _, cx| {
                for index in workbench.dirty_editors() {
                    workbench.save_editor(index, cx);
                }
                Outcome::done()
            });
        }
        self.commands.register("workbench.action.files.revert", |workbench, _, _, cx| {
            if let Some(index) = workbench.editors.active_index() {
                workbench.revert_editor(index, cx);
            }
            Outcome::done()
        });
        self.commands.register("_workbench.revertAllDirty", |workbench, _, _, cx| {
            for index in workbench.dirty_editors() {
                workbench.revert_editor(index, cx);
            }
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

    fn dirty_editors(&self) -> Vec<usize> {
        (0..self.editors.editors.len()).filter(|index| self.editors.editors[*index].is_dirty()).collect()
    }

    fn close_all_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        self.close_editors((0..self.editors.editors.len()).collect(), window, cx)
    }

    /// `AbstractCloseAllAction.run` and `handleDirtyClosing`: dirty editors among `indices` ask to save first;
    /// Cancel keeps every editor open.
    fn close_editors(&mut self, indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let dirty: Vec<usize> = indices.iter().copied().filter(|index| self.editors.editors[*index].is_dirty()).collect();
        // showSaveConfirm answers Don't Save without a dialog while extension tests run.
        if dirty.is_empty() || self.skip_dialogs {
            self.discard_and_close(&dirty, &indices, cx);
            return Outcome::done();
        }
        let names: Vec<String> = dirty.iter().map(|index| self.editors.editors[*index].document.name()).collect();
        let (message, detail) = save_confirm_text(&names);
        let save = if names.len() > 1 { "Save All" } else { "Save" };
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &[save, "Don't Save", "Cancel"], cx);
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn_in(window, async move |workbench, cx| {
            let answer = answer.await;
            let _ = workbench.update(cx, |workbench, cx| match answer {
                Ok(0) => {
                    for index in &dirty {
                        workbench.save_editor(*index, cx);
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

    /// The text goes back to the file's content and the editor is clean again.
    /// shortcut: one whole-text edit plus a save of the same bytes, since `editor/` has no revert yet; move to an `OpenEditor::revert` that flushes the model like `setValue` once it does.
    fn revert_editor(&mut self, index: usize, cx: &mut Context<Self>) {
        let editor = &mut self.editors.editors[index];
        let disk = match TextDocument::read(&editor.document.path, &self.languages, &self.settings.lock().unwrap()) {
            Ok(document) => document,
            Err(error) => {
                eprintln!("reverting {}: {error}", editor.document.path.display());
                return;
            }
        };
        let lines = &editor.document.lines;
        let end = Position::new(lines.len() - 1, lines[lines.len() - 1].chars().count());
        let change = editor.execute_edits(vec![(Position::new(0, 0), end, disk.lines.join("\n"), false)], None, EditReason::Unknown("revert"));
        if let Some(change) = change {
            self.host.send_content_change(&self.editors, index, &change);
        }
        self.save_editor(index, cx);
    }
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
