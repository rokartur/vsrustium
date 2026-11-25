//! unusualLineTerminators.ts `UnusualLineTerminatorsDetector`: LS and PS in the shown model are removed, after a
//! confirmation unless `editor.unusualLineTerminators` is `auto`.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;

use config::configuration::Overrides;
use gpui::{Context, PromptLevel, Window};

use super::Workbench;
use crate::editor::OpenEditor;
use crate::editor::unusual_line_terminators::has_unusual_line_terminators;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct UnusualLineTerminators {
    /// The model, version and setting last checked: a check runs when the code editor shows another model, the
    /// model changes or the setting changes.
    checked: Cell<Option<(u32, u32, &'static str)>>,
    /// `_isPresentingDialog`.
    presenting_dialog: bool,
    /// The `ignoreUnusualLineTerminators` model property, kept by resource as `codeEditorService.setModelProperty` does.
    ignored: HashSet<(String, PathBuf)>,
}

impl Workbench {
    /// Schedules `_checkForUnusualLineTerminators` from the frame that shows another model, version or setting.
    pub(super) fn schedule_unusual_line_terminators_check(&self, editor: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) {
        let setting = self.unusual_line_terminators_setting(editor);
        let key = (editor.model_id, editor.version_id, setting);
        let Some(previous) = self.unusual_line_terminators.checked.replace(Some(key)) else {
            return self.defer_unusual_line_terminators_check(window, cx);
        };
        // Content changed by an undo is not checked (`e.isUndoing`).
        let undone = previous.0 == key.0 && previous.2 == key.2 && editor.last_change_was_undo();
        if previous != key && !undone {
            self.defer_unusual_line_terminators_check(window, cx);
        }
    }

    fn defer_unusual_line_terminators_check(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |workbench, window, cx| workbench.check_unusual_line_terminators(window, cx));
    }

    fn unusual_line_terminators_setting(&self, editor: &OpenEditor) -> &'static str {
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let value = self.settings.lock().unwrap().get_value(Some("editor.unusualLineTerminators"), &overrides);
        match value.as_ref().and_then(|value| value.as_str()) {
            Some("auto") => "auto",
            Some("off") => "off",
            _ => "prompt",
        }
    }

    /// `_checkForUnusualLineTerminators`. The port has no read-only text editors, so that exit never applies.
    fn check_unusual_line_terminators(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.active() else { return };
        let setting = self.unusual_line_terminators_setting(editor);
        let resource = (editor.document.scheme.clone(), editor.document.path.clone());
        if setting == "off" || !has_unusual_line_terminators(&editor.document.lines) || self.unusual_line_terminators.ignored.contains(&resource) {
            return;
        }
        if setting == "auto" {
            self.remove_unusual_line_terminators(&resource);
            return cx.notify();
        }
        if self.unusual_line_terminators.presenting_dialog {
            return;
        }
        let name = editor.document.path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        let detail = format!(
            "The file '{name}' contains one or more unusual line terminator characters, like Line Separator (LS) or Paragraph Separator (PS).\n\nIt is recommended to remove them from the file. This can be configured via `editor.unusualLineTerminators`."
        );
        let answer = match self.skip_dialogs {
            true => None,
            false => Some(window.prompt(PromptLevel::Info, "Detected unusual line terminators", Some(&detail), &["Remove Unusual Line Terminators", "Ignore"], cx)),
        };
        self.unusual_line_terminators.presenting_dialog = true;
        cx.spawn_in(window, async move |workbench, cx| {
            let confirmed = match answer {
                Some(answer) => answer.await == Ok(0),
                None => true,
            };
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.unusual_line_terminators.presenting_dialog = false;
                match confirmed {
                    true => workbench.remove_unusual_line_terminators(&resource),
                    false => _ = workbench.unusual_line_terminators.ignored.insert(resource),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `model.removeUnusualLineTerminators`, then the host hears of the change.
    fn remove_unusual_line_terminators(&mut self, (scheme, path): &(String, PathBuf)) {
        let Some(model) = self.editors.model_mut(scheme, path) else { return };
        if let Some(change) = model.remove_unusual_line_terminators() {
            self.send_model_edit(scheme, path, &change);
        }
    }
}
