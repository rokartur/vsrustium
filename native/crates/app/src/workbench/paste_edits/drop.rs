//! dropIntoEditorController.ts and the drop half of defaultProviders.ts: files dropped on the text with Shift held
//! go through the extension `DocumentDropEditProvider`s and the built-in path providers; the first edit applies
//! at the drop position and, with more than one, the post-drop widget switches to another (Cmd+. or its button).
//! shortcut: the data transfer carries the dropped files' `text/uri-list` only, without `asFile()` contents;
//! upgrade with file data items that `$resolveDocumentOnDropFileData` reads.

use std::path::{Path, PathBuf};

use futures::future::join_all;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::{PasteEdit, Workbench, kind_contains, matches_mime, paste_edit, sort_by_yield_to, strings};
use crate::editor::{Position, Selection, utf16_column};
use crate::editor_history::file_uri_string;
use crate::language_features::Candidate;

const REGISTER: &str = "$registerDocumentOnDropEditProvider";
const URI_LIST: &str = "text/uri-list";

impl Workbench {
    /// `onDropIntoEditor`: the drop position takes the cursor, then the providers whose drop mime types match turn
    /// the dropped files into edits.
    pub(super) fn drop_into_editor(&mut self, position: Position, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_edits.drop_target = None;
        let Some(editor) = self.editors.active() else { return };
        if self.editor_setting_value(editor, "editor.dropIntoEditor.enabled") == json!(false) || paths.is_empty() || self.edit_blocked(true, cx) {
            return cx.notify();
        }
        window.focus(&self.editor_focus, cx);
        self.clear_paste_widget(cx);
        self.paste_edits.fallback = None;
        let editor = self.editors.active_mut().expect("checked above");
        editor.set_selections(vec![Selection::caret(position)]);
        let editor = self.editors.active().expect("checked above");
        let uri = editor.document().uri();
        let providers: Vec<u64> = self
            .language_features
            .ordered(REGISTER, &Candidate { uri: &uri, language: &editor.document().language_id })
            .into_iter()
            .filter(|registration| {
                let mimes = &registration.args[2]["dropMimeTypes"];
                mimes.is_null() || strings(mimes).iter().any(|pattern| matches_mime(pattern, URI_LIST))
            })
            .map(|registration| registration.handle)
            .collect();
        let uri_list = paths.iter().map(|path| file_uri_string(path)).collect::<Vec<_>>().join("\r\n");
        let uri_list_data: Vec<Value> = paths.iter().map(|path| crate::editor::uri_value("file", path)).collect();
        let transfer = json!({ "items": [[URI_LIST, { "id": "0", "asString": uri_list, "uriListData": uri_list_data }]] });
        let column = utf16_column(&editor.document().lines[position.line], position.column);
        let position_dto = json!({ "lineNumber": position.line + 1, "column": column + 1 });
        let (model_id, version_id) = (editor.model_id, editor.text_model().version_id);
        let calls: Vec<_> = providers
            .iter()
            .enumerate()
            .map(|(request_id, handle)| self.host.call_cancellable("ExtHostLanguageFeatures", "$provideDocumentOnDropEdits", vec![json!(handle), json!(request_id), uri.clone(), position_dto.clone(), transfer.clone()]))
            .collect();
        let defaults = default_drop_edits(&paths, &self.workspace_folders());
        let operation = cx.spawn_in(window, async move |workbench, cx| {
            let answers: Vec<Value> = join_all(calls).await.into_iter().map(|answer| answer.ok().and_then(Result::ok).unwrap_or(Value::Null)).collect();
            let _ = workbench.update_in(cx, |workbench, _, cx| {
                workbench.clear_inline_progress(cx);
                workbench.paste_edits.operation = None;
                let mut edits = Vec::new();
                for (handle, answer) in providers.iter().zip(&answers) {
                    edits.extend(answer.as_array().into_iter().flatten().map(|dto| drop_edit(*handle, dto)));
                }
                edits.extend(defaults);
                let edits = sort_by_yield_to(edits);
                let still = workbench.editors.active().is_some_and(|editor| editor.model_id == model_id && editor.text_model().version_id == version_id);
                if edits.is_empty() || !still {
                    return;
                }
                workbench.finish_drop(edits, position, cx);
            });
        });
        self.paste_edits.operation = Some(operation);
        self.show_inline_progress(model_id, position, "Running drop handlers. Click to cancel".into(), cx);
    }

    /// `applyEditAndShowIfNeeded` with the `editor.dropIntoEditor.preferences` edit, the widget after it.
    fn finish_drop(&mut self, edits: Vec<PasteEdit>, position: Position, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("checked by the caller");
        let preferences = self.editor_setting_value(editor, "editor.dropIntoEditor.preferences");
        let active = strings(&preferences).iter().find_map(|kind| edits.iter().position(|edit| edit.kind.as_deref().is_some_and(|edit_kind| kind_contains(kind, edit_kind)))).unwrap_or(0);
        let show_widget = self.editor_string_setting(editor, "editor.dropIntoEditor.showDropSelector") == "afterDrop";
        let edit = edits[active].clone();
        let ranges = vec![(position, position)];
        if show_widget && edits.len() > 1 {
            self.show_paste_widget(edits, active, ranges.clone(), true);
        }
        self.apply_paste_edit(edit, &ranges, cx);
    }

    /// editorDropTarget.ts lets a drag reach the text only with Shift held (`showDropIndicatorAt`); `None` is a drag
    /// off the text.
    pub(in crate::workbench) fn drag_files_over_text(&mut self, over: Option<(usize, f32)>, shift: bool, cx: &mut Context<Self>) {
        let enabled = self.editors.active().is_some_and(|editor| self.editor_setting_value(editor, "editor.dropIntoEditor.enabled") != json!(false));
        let target = over.filter(|_| enabled && shift).map(|(line, visible)| self.position_at(line, visible));
        if self.paste_edits.drop_target != target {
            self.paste_edits.drop_target = target;
            cx.notify();
        }
    }

    /// Files dropped on the text: with Shift they go into it, else the editor group's drop opens them.
    pub(in crate::workbench) fn drop_files_on_text(&mut self, line: usize, visible: f32, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if !window.modifiers().shift {
            self.paste_edits.drop_target = None;
            return cx.notify();
        }
        cx.stop_propagation();
        let position = self.position_at(line, visible);
        self.drop_into_editor(position, paths, window, cx);
    }

    pub(in crate::workbench) fn drop_target(&self) -> Option<Position> {
        self.paste_edits.drop_target
    }
}

/// `IDocumentDropEditDto` as revived by mainThreadLanguageFeatures.ts: `yieldTo` kinds become `{ kind }`.
fn drop_edit(handle: u64, dto: &Value) -> PasteEdit {
    let mut edit = paste_edit(Some(handle), dto);
    edit.yield_to = strings(&dto["yieldTo"]).into_iter().map(|kind| json!({ "kind": kind })).collect();
    edit
}

/// `RelativePathProvider` then `PathProvider` (the newer registration answers first): the files' paths relative to
/// their workspace folder, when every one has one, and their absolute paths, joined by spaces.
fn default_drop_edits(paths: &[PathBuf], folders: &[PathBuf]) -> Vec<PasteEdit> {
    let plural = paths.len() > 1;
    let mut edits = Vec::new();
    let relative: Option<Vec<String>> = paths.iter().map(|path| relative_path(path, folders)).collect();
    if let Some(relative) = relative {
        edits.push(path_edit(if plural { "Insert Relative Paths" } else { "Insert Relative Path" }, "uri.path.relative", relative.join(" ")));
    }
    let absolute: Vec<String> = paths.iter().map(|path| path.to_string_lossy().into_owned()).collect();
    edits.push(path_edit(if plural { "Insert Paths" } else { "Insert Path" }, "uri.path.absolute", absolute.join(" ")));
    edits
}

/// `relativePath(workspaceFolder.uri, uri)`: `None` outside every folder, empty for the folder itself.
fn relative_path(path: &Path, folders: &[PathBuf]) -> Option<String> {
    let folder = folders.iter().filter(|folder| path.starts_with(folder)).max_by_key(|folder| folder.as_os_str().len())?;
    Some(path.strip_prefix(folder).expect("filtered above").to_string_lossy().into_owned())
}

fn path_edit(title: &str, kind: &str, text: String) -> PasteEdit {
    PasteEdit { handle: None, title: title.into(), kind: Some(kind.into()), insert_text: json!(text), additional_edit: Value::Null, yield_to: Vec::new(), handled_mime: Some(URI_LIST.into()), cache_id: Value::Null }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_need_a_workspace_folder_for_every_file() {
        let folders = vec![PathBuf::from("/w"), PathBuf::from("/w/inner")];
        let titles = |paths: &[&str]| default_drop_edits(&paths.iter().map(PathBuf::from).collect::<Vec<_>>(), &folders).into_iter().map(|edit| (edit.title, edit.insert_text)).collect::<Vec<_>>();
        assert_eq!(titles(&["/w/inner/a.md"]), vec![("Insert Relative Path".into(), json!("a.md")), ("Insert Path".into(), json!("/w/inner/a.md"))]);
        assert_eq!(titles(&["/w/a.md", "/elsewhere/b.md"]), vec![("Insert Paths".into(), json!("/w/a.md /elsewhere/b.md"))]);
    }
}
