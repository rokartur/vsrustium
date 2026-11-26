//! The compare commands of fileCommands.ts and fileActions.ts: Compare with Saved (`showModifications` side read
//! from disk), Select for Compare, Compare with Selected, Compare Selected, Compare Active File With...,
//! Compare Active File with Clipboard and Compare New Untitled Text Files.

use std::path::PathBuf;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::diff_editor::Side;
use crate::editor::uri_value;

/// fileCommands.ts `COMPARE_WITH_SAVED_SCHEMA`.
const COMPARE_WITH_SAVED_SCHEME: &str = "showModifications";

impl Workbench {
    pub(super) fn register_file_compare_commands(&mut self) {
        self.commands.register("workbench.files.action.compareWithSaved", |workbench, args, window, cx| {
            let Some(resource) = workbench.resource_for_command(&args, window).filter(|side| side.scheme == "file") else { return Outcome::done() };
            let content = match std::fs::read(&resource.path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(error) => return Outcome::error(error.to_string()),
            };
            let saved = Side { scheme: COMPARE_WITH_SAVED_SCHEME.into(), path: resource.path.clone() };
            // TextFileContentProvider: the side shows the file as it is on disk now.
            match workbench.editors.model(&saved.scheme, &saved.path).is_some() {
                true => drop(workbench.update_model(&saved.scheme, &saved.path, &content, true)),
                false => {
                    if let Err(error) = workbench.try_open_document(&uri_value(&saved.scheme, &saved.path), &Value::Null, &json!(content), false, cx) {
                        return Outcome::Ready(Err(error));
                    }
                }
            }
            let name = resource.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            workbench.compare_resources(saved, resource, Some(format!("{name} (in file) \u{2194} {name}")), window, cx);
            Outcome::done()
        });
        self.commands.register("selectForCompare", |workbench, args, window, cx| {
            workbench.resource_to_compare = workbench.resource_for_command(&args, window);
            cx.notify();
            Outcome::done()
        });
        self.commands.register("compareFiles", |workbench, args, window, cx| {
            if let (Some(original), Some(modified)) = (workbench.resource_to_compare.clone(), workbench.resource_for_command(&args, window)) {
                workbench.compare_resources(original, modified, None, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("compareSelected", |workbench, args, window, cx| {
            if let [original, modified] = workbench.multi_selected_resources(&args, window).as_slice() {
                workbench.compare_resources(file_side(original.clone()), file_side(modified.clone()), None, window, cx);
            }
            Outcome::done()
        });
        self.commands.register("workbench.files.action.compareFileWith", |workbench, _, window, cx| {
            let Some(active) = workbench.editors.active().map(|editor| Side { scheme: editor.document().scheme.clone(), path: editor.document().path.clone() }) else { return Outcome::done() };
            workbench.show_quick_access_activating("", false, false, crate::quick_input::ItemActivation::Second, window, cx);
            workbench.quick_access.compare_with = workbench.quick_inputs.current().map(|input| (input.id, active));
            Outcome::done()
        });
        self.commands.register("workbench.files.action.compareWithClipboard", |workbench, _, window, cx| {
            let Some(active) = workbench.editors.active().map(|editor| Side { scheme: editor.document().scheme.clone(), path: editor.document().path.clone() }) else { return Outcome::done() };
            if active.scheme != "file" && active.scheme != "untitled" {
                return Outcome::done();
            }
            // fileActions.ts `SCHEME_COUNTER`: a new scheme per comparison, so each reads the clipboard of its time.
            workbench.clipboard_compare_count += 1;
            let clipboard = Side { scheme: format!("clipboardCompare{}", workbench.clipboard_compare_count - 1), path: active.path.clone() };
            let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
            if let Err(error) = workbench.try_open_document(&uri_value(&clipboard.scheme, &clipboard.path), &Value::Null, &json!(text), false, cx) {
                return Outcome::Ready(Err(error));
            }
            let name = active.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            workbench.compare_resources(clipboard, active, Some(format!("Clipboard \u{2194} {name}")), window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.files.action.compareNewUntitledTextFiles", |workbench, _, window, cx| {
            let mut sides = Vec::new();
            for _ in 0..2 {
                let path = (1..).map(|n| PathBuf::from(format!("Untitled-{n}"))).find(|path| workbench.editors.model("untitled", path).is_none()).expect("a free name");
                if let Err(error) = workbench.try_open_document(&uri_value("untitled", &path), &Value::Null, &Value::Null, false, cx) {
                    return Outcome::Ready(Err(error));
                }
                sides.push(path);
            }
            let (original, modified) = (sides.remove(0), sides.remove(0));
            workbench.compare_resources(Side { scheme: "untitled".into(), path: original }, Side { scheme: "untitled".into(), path: modified }, None, window, cx);
            Outcome::done()
        });
    }

    /// `editorService.openEditor({ original, modified, label, options: { pinned: true } })`.
    pub(super) fn compare_resources(&mut self, original: Side, modified: Side, label: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let args = vec![uri_value(&original.scheme, &original.path), uri_value(&modified.scheme, &modified.path), label.map_or(Value::Null, Value::String), json!([null, { "pinned": true }])];
        drop(self.execute_command("_workbench.diff", args, window, cx));
    }

    /// files.ts `getResourceForCommand`: the command's resource, else the focused Explorer row, else the active
    /// editor's.
    fn resource_for_command(&self, args: &[Value], window: &Window) -> Option<Side> {
        if let Some(path) = args.first().and_then(|uri| uri["path"].as_str()) {
            let scheme = args[0]["scheme"].as_str().unwrap_or("file");
            return Some(Side { scheme: scheme.into(), path: PathBuf::from(path) });
        }
        if self.explorer.focus.is_focused(window)
            && let Some(focused) = self.folder.as_ref().and_then(|tree| tree.focused.clone())
        {
            return Some(file_side(focused));
        }
        self.editors.active().map(|editor| Side { scheme: editor.document().scheme.clone(), path: editor.document().path.clone() })
    }
}

pub(super) fn file_side(path: PathBuf) -> Side {
    Side { scheme: "file".into(), path }
}

#[cfg(test)]
mod tests;
