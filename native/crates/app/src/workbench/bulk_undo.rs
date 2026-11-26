//! undoRedoService.ts workspace elements for the text edits of `workspace.applyEdit` (bulkTextEdits.ts pushes one
//! `MultiModelEditStackElement` for the models it edited): Undo in one of them offers to undo the edit in every
//! file, Redo then redoes it in every file.

use std::path::PathBuf;

use gpui::{Context, PromptLevel, Window};

use super::Workbench;
use crate::commands::Outcome;
use crate::undo_redo::Direction;

/// One multi-file text edit: each model with its alternative version before and after the edit.
pub(super) struct WorkspaceTextEdit {
    pub label: String,
    pub models: Vec<(String, PathBuf, u32, u32)>,
    pub undone: bool,
}

impl Workbench {
    /// Called with the alternative versions of every model a workspace edit changed.
    pub(super) fn push_workspace_text_edit(&mut self, label: String, models: Vec<(String, PathBuf, u32, u32)>) {
        if models.len() > 1 {
            self.workspace_text_edits.push(WorkspaceTextEdit { label, models, undone: false });
        }
    }

    /// `undo`/`redo` in the active editor when its last change is a multi-file edit whose other models are still
    /// where the edit left them: undo asks "Would you like to undo '{0}' across all files?" first. `None` leaves
    /// the command to the editor alone (`_tryToSplitAndUndo` when the other files moved on).
    pub(super) fn workspace_text_undo(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) -> Option<Outcome> {
        let index = self.workspace_text_edit_at(direction)?;
        let element = &self.workspace_text_edits[index];
        if direction == Direction::Redo {
            self.run_workspace_text_edit(index, direction, cx);
            return Some(Outcome::done());
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
        Some(Outcome::done())
    }

    /// The edit the active model's undo or redo would take back: its last, still in effect in every model.
    fn workspace_text_edit_at(&self, direction: Direction) -> Option<usize> {
        let active = self.editors.active()?;
        let (scheme, path) = (&active.document().scheme, &active.document().path);
        let index = self.workspace_text_edits.iter().rposition(|element| element.models.iter().any(|(model_scheme, model_path, _, _)| model_scheme == scheme && model_path == path))?;
        let element = &self.workspace_text_edits[index];
        let wanted_undone = direction == Direction::Redo;
        let in_place = element.undone == wanted_undone
            && element.models.iter().all(|(scheme, path, before, after)| {
                let at = if wanted_undone { *before } else { *after };
                self.editors.model(scheme, path).is_some_and(|model| model.alternative_version_id() == at)
            });
        in_place.then_some(index)
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

#[cfg(test)]
mod tests;
