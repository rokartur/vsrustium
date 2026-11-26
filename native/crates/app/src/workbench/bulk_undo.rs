//! undoRedoService.ts workspace elements of `workspace.applyEdit` and the other bulk edits: the text edits (bulkTextEdits.ts
//! pushes one `MultiModelEditStackElement` for the models it edited), the file edits (`FileUndoRedoElement`, kept in
//! the Explorer's `UndoRedo`), and the `UndoRedoGroup` that runs every element of one workspace edit together.
//! Undo in one model of a multi-file text edit offers to undo it in every file, Redo then redoes it in every file.

use std::path::PathBuf;

use files::operations::FileUndo;
use gpui::{AsyncWindowContext, Context, PromptLevel, WeakEntity, Window};

use super::Workbench;
use super::explorer_actions::{confirm, run_file_element};
use crate::commands::Outcome;
use crate::undo_redo::Direction;

/// One text edit of a workspace edit: each model with its alternative version before and after the edit.
pub(super) struct WorkspaceTextEdit {
    pub label: String,
    pub models: Vec<(String, PathBuf, u32, u32)>,
    pub undone: bool,
    /// `UndoRedoGroup` id of the workspace edit.
    pub group: u64,
    /// When it was pushed (`next_undo_stamp`), the order of the group's elements.
    pub stamp: u64,
}

/// An element of an undo group: a workspace text edit by index, or a file element by id.
#[derive(Clone, Copy, PartialEq)]
enum GroupElement {
    Text(usize),
    File(u64),
}

impl Workbench {
    /// Called with the alternative versions of every model a workspace edit changed.
    pub(super) fn push_workspace_text_edit(&mut self, label: String, models: Vec<(String, PathBuf, u32, u32)>, group: u64) {
        if !models.is_empty() {
            self.workspace_text_edits.push(WorkspaceTextEdit { label, models, undone: false, group, stamp: crate::editor::next_undo_stamp() });
        }
    }

    /// The file edits of `workspace.applyEdit` (`BulkFileEdits` with `UndoRedoSource.None`), on the stack of every
    /// file they touched.
    pub(crate) fn push_file_undo(&mut self, undo: FileUndo) {
        if undo.operations.iter().all(Vec::is_empty) {
            return;
        }
        let resources = undo.resources().iter().map(|resource| (resource.path.to_lowercase(), resource.path.clone())).collect();
        self.explorer.undo.push("Workspace Edit".into(), resources, 0, undo.group, false, undo);
    }

    /// `undo`/`redo` in the active editor (undoRedoService.ts `undo(resource)`): the later of the model's own last
    /// change and the file element on its stack runs. An element whose group has another element ready runs the
    /// group in order (`_isPartOfUndoGroup`, no split prompt); a multi-file text edit alone asks first. `None`
    /// leaves the command to the editor alone.
    pub(super) fn workspace_undo(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) -> Option<Outcome> {
        let active = self.editors.active()?;
        let (scheme, path) = (active.document().scheme.clone(), active.document().path.clone());
        let text_stamp = match (direction, active.undo_stamps()) {
            (Direction::Undo, (undo, _)) => undo,
            (Direction::Redo, (_, redo)) => redo,
        };
        let file = match scheme.as_str() {
            "file" => self.explorer.undo.next_on_resource(&path.to_string_lossy().to_lowercase(), direction),
            _ => None,
        };
        let file_first = file.filter(|file| match (direction, text_stamp) {
            (_, None) => true,
            (Direction::Undo, Some(stamp)) => file.stamp > stamp,
            (Direction::Redo, Some(stamp)) => file.stamp < stamp,
        });
        let (element, group) = match file_first {
            Some(file) => (GroupElement::File(file.id), file.group),
            None => {
                let index = self.workspace_text_edit_at(direction)?;
                (GroupElement::Text(index), self.workspace_text_edits[index].group)
            }
        };
        if self.ready_in_group(group, direction).iter().any(|(_, other)| *other != element) {
            cx.spawn_in(window, async move |workbench, cx| run_undo_group(&workbench, cx, group, direction).await).detach();
            return Some(Outcome::done());
        }
        match element {
            GroupElement::Text(index) => Some(self.workspace_text_undo(index, direction, window, cx)),
            GroupElement::File(id) => {
                let file = file_first.expect("a file element was picked");
                // `_confirmAndContinueUndo`: an element of another source (the Explorer's) or one that asks first.
                let confirm_first = direction == Direction::Undo && (file.source != 0 || file.confirm_before_undo);
                let label = file.label.clone();
                cx.spawn_in(window, async move |workbench, cx| {
                    if confirm_first && !confirm(&workbench, cx, PromptLevel::Info, format!("Would you like to undo '{label}'?"), None, "Yes").await {
                        return;
                    }
                    run_file_element(&workbench, cx, id, direction).await;
                })
                .detach();
                Some(Outcome::done())
            }
        }
    }

    /// `_confirmAndExecuteWorkspaceUndo` of a text edit: undo of a multi-file one asks "Would you like to undo '{0}'
    /// across all files?" first.
    fn workspace_text_undo(&mut self, index: usize, direction: Direction, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let element = &self.workspace_text_edits[index];
        if direction == Direction::Redo || element.models.len() == 1 {
            self.run_workspace_text_edit(index, direction, cx);
            return Outcome::done();
        }
        let message = format!("Would you like to undo '{}' across all files?", element.label);
        let buttons = [format!("Undo in {} Files", element.models.len()), "Undo this File".to_owned(), "Cancel".to_owned()];
        let buttons: Vec<&str> = buttons.iter().map(String::as_str).collect();
        let answer = window.prompt(PromptLevel::Info, &message, None, &buttons, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(button) = answer.await else { return };
            let _ = workbench.update(cx, |workbench, cx| match button {
                0 => workbench.run_workspace_text_edit(index, Direction::Undo, cx),
                1 => {
                    workbench.workspace_text_edits.remove(index);
                    drop(workbench.run_on_active_editor(crate::editor::OpenEditor::undo, cx));
                }
                _ => {}
            });
        })
        .detach();
        Outcome::done()
    }

    /// The edit the active model's undo or redo would take back: its last, still in effect in every model.
    fn workspace_text_edit_at(&self, direction: Direction) -> Option<usize> {
        let active = self.editors.active()?;
        let (scheme, path) = (&active.document().scheme, &active.document().path);
        let index = self.workspace_text_edits.iter().rposition(|element| element.models.iter().any(|(model_scheme, model_path, _, _)| model_scheme == scheme && model_path == path))?;
        self.text_edit_in_place(&self.workspace_text_edits[index], direction).then_some(index)
    }

    /// Whether undo (or redo) can take the edit back in every model: each is at the version the edit left (or found).
    fn text_edit_in_place(&self, element: &WorkspaceTextEdit, direction: Direction) -> bool {
        let wanted_undone = direction == Direction::Redo;
        element.undone == wanted_undone
            && element.models.iter().all(|(scheme, path, before, after)| {
                let at = if wanted_undone { *before } else { *after };
                self.editors.model(scheme, path).is_some_and(|model| model.alternative_version_id() == at)
            })
    }

    /// The group's elements that can run next in `direction`, with their stamps.
    fn ready_in_group(&self, group: u64, direction: Direction) -> Vec<(u64, GroupElement)> {
        let mut ready = Vec::new();
        for (index, element) in self.workspace_text_edits.iter().enumerate() {
            if element.group == group && self.text_edit_in_place(element, direction) {
                ready.push((element.stamp, GroupElement::Text(index)));
            }
        }
        if let Some(file) = self.explorer.undo.next_in_group(group, direction) {
            ready.push((file.stamp, GroupElement::File(file.id)));
        }
        ready
    }

    fn run_workspace_text_edit(&mut self, index: usize, direction: Direction, cx: &mut Context<Self>) {
        let models = self.workspace_text_edits[index].models.clone();
        let active = self.editors.active().map(|editor| (editor.document().scheme.clone(), editor.document().path.clone()));
        for (scheme, path, _, _) in &models {
            if active.as_ref().is_some_and(|(active_scheme, active_path)| active_scheme == scheme && active_path == path) {
                let command = match direction {
                    Direction::Undo => crate::editor::OpenEditor::undo,
                    Direction::Redo => crate::editor::OpenEditor::redo,
                };
                drop(self.run_on_active_editor(command, cx));
                continue;
            }
            let Some(model) = self.editors.model_mut(scheme, path) else { continue };
            let change = match direction {
                Direction::Undo => model.undo(),
                Direction::Redo => model.redo(),
            };
            if let Some(change) = change {
                self.send_model_edit(scheme, path, &change);
            }
        }
        self.workspace_text_edits[index].undone = direction == Direction::Undo;
        self.host.send_visible_ranges(&self.editors);
        cx.notify();
    }
}

/// `_continueUndoInGroup` / `_continueRedoInGroup`: the group's ready elements one after another, the one pushed last
/// first for undo, first pushed first for redo, until none is left or one cannot run.
pub(super) async fn run_undo_group(workbench: &WeakEntity<Workbench>, cx: &mut AsyncWindowContext, group: u64, direction: Direction) {
    loop {
        let next = workbench.update(cx, |workbench, _| {
            let ready = workbench.ready_in_group(group, direction);
            match direction {
                Direction::Undo => ready.into_iter().max_by_key(|(stamp, _)| *stamp),
                Direction::Redo => ready.into_iter().min_by_key(|(stamp, _)| *stamp),
            }
        });
        match next {
            Ok(Some((_, GroupElement::Text(index)))) => {
                let _ = workbench.update(cx, |workbench, cx| workbench.run_workspace_text_edit(index, direction, cx));
            }
            Ok(Some((_, GroupElement::File(id)))) => {
                if !run_file_element(workbench, cx, id, direction).await {
                    return;
                }
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests;
