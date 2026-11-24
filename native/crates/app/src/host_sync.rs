//! What the main thread tells the extension host about open text documents, the visible editor and the
//! tabs: mainThreadDocumentsAndEditors.ts (`$acceptDocumentsAndEditorsDelta`), mainThreadEditorTabs.ts
//! (`$acceptEditorTabModel`), mainThreadEditors.ts (`$acceptEditorPropertiesChanged`) and the
//! `onLanguage` activation languageService.ts sends when a model of a new language is created.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use exthost::scan::file_uri;
use serde_json::{Value, json};

use crate::editor::{EditorGroup, OpenEditor};
use crate::theme::ThemeKind;

#[derive(Debug)]
pub struct HostMessage {
    pub proxy: &'static str,
    pub method: &'static str,
    pub args: Vec<Value>,
}

pub struct HostSync {
    sender: Sender<HostMessage>,
    /// The documents the host has, in the order they were added.
    documents: Vec<PathBuf>,
    /// The snapshot id of the visible editor the host has.
    editor: Option<String>,
    activation_events: HashSet<String>,
}

/// The one editor group's code editor widget; it is reused for every model the group shows.
const CODE_EDITOR_ID: &str = "vs.editor.ICodeEditor:1";

impl HostSync {
    pub fn new(sender: Sender<HostMessage>) -> Self {
        HostSync { sender, documents: Vec::new(), editor: None, activation_events: HashSet::new() }
    }

    /// Sends what changed in `group` since the last call: activation for new languages, the document
    /// and editor delta, then the tab model.
    pub fn send_changes(&mut self, group: &EditorGroup) {
        let open: Vec<&OpenEditor> = group.editors.iter().collect();
        let added: Vec<&OpenEditor> = open.iter().copied().filter(|editor| !self.documents.contains(&editor.document.path)).collect();
        let removed: Vec<PathBuf> = self.documents.iter().filter(|path| group.find(path).is_none()).cloned().collect();
        for editor in &added {
            for event in [format!("onLanguage:{}", editor.document.language_id), "onLanguage".to_string()] {
                if self.activation_events.insert(event.clone()) {
                    // ActivationKind.Normal
                    self.send("ExtHostExtensionService", "$activateByEvent", vec![json!(event), json!(0)]);
                }
            }
        }

        let mut delta = serde_json::Map::new();
        if !removed.is_empty() {
            delta.insert("removedDocuments".into(), removed.iter().map(|path| file_uri(path)).collect());
        }
        if !added.is_empty() {
            delta.insert("addedDocuments".into(), added.iter().map(|editor| model_added_data(editor)).collect());
        }
        let active = group.active();
        let active_id = active.map(snapshot_id);
        if active_id != self.editor {
            if let Some(previous) = &self.editor {
                delta.insert("removedEditors".into(), json!([previous]));
            }
            if let Some(editor) = active {
                delta.insert("addedEditors".into(), json!([text_editor_add_data(editor)]));
            }
            delta.insert("newActiveEditor".into(), json!(active_id));
        }
        if !delta.is_empty() {
            self.send("ExtHostDocumentsAndEditors", "$acceptDocumentsAndEditorsDelta", vec![Value::Object(delta)]);
        }
        self.documents.retain(|path| !removed.contains(path));
        self.documents.extend(added.iter().map(|editor| editor.document.path.clone()));
        self.editor = active_id;

        let tabs: Vec<Value> = open.iter().enumerate().map(|(index, editor)| tab_dto(editor, group.active_index() == Some(index))).collect();
        self.send("ExtHostEditorTabs", "$acceptEditorTabModel", vec![json!([{ "isActive": true, "viewColumn": 0, "groupId": 0, "tabs": tabs }])]);
    }

    /// Tells the host where the cursor of the visible editor moved.
    pub fn send_selection(&self, editor: &OpenEditor, source: &str) {
        let selections = json!({ "selections": [selection(editor)], "source": source });
        self.send("ExtHostEditors", "$acceptEditorPropertiesChanged", vec![json!(snapshot_id(editor)), json!({ "options": null, "selections": selections, "visibleRanges": null })]);
    }

    /// mainThreadTheming.ts: the theme type behind `window.activeColorTheme.kind`, sent once at startup.
    pub fn send_color_theme(&self, kind: ThemeKind) {
        let scheme = match kind {
            ThemeKind::Dark => "dark",
            ThemeKind::Light => "light",
            ThemeKind::HighContrastDark => "hcDark",
            ThemeKind::HighContrastLight => "hcLight",
        };
        self.send("ExtHostTheming", "$onColorThemeChange", vec![json!(scheme)]);
    }

    fn send(&self, proxy: &'static str, method: &'static str, args: Vec<Value>) {
        // The session thread is gone once the host exits; the workbench keeps working without it.
        let _ = self.sender.send(HostMessage { proxy, method, args });
    }
}

/// `TextEditorSnapshot.id`: the code editor's id and the model's id.
fn snapshot_id(editor: &OpenEditor) -> String {
    format!("{CODE_EDITOR_ID},$model{}", editor.model_id)
}

fn model_added_data(editor: &OpenEditor) -> Value {
    let document = &editor.document;
    json!({
        "uri": file_uri(&document.path),
        "versionId": 1,
        "lines": document.lines,
        "EOL": document.eol,
        "languageId": document.language_id,
        "isDirty": false,
        "encoding": document.encoding,
    })
}

fn text_editor_add_data(editor: &OpenEditor) -> Value {
    let document = &editor.document;
    let indentation = document.indentation;
    let last_line = document.lines.len();
    json!({
        "id": snapshot_id(editor),
        "documentUri": file_uri(&document.path),
        // TextEditorCursorStyle.Line, RenderLineNumbersType.On; indentSize follows tabSize by default.
        "options": {
            "tabSize": indentation.tab_size,
            "indentSize": indentation.tab_size,
            "originalIndentSize": "tabSize",
            "insertSpaces": indentation.insert_spaces,
            "cursorStyle": 1,
            "lineNumbers": 1,
        },
        "selections": [selection(editor)],
        // shortcut: reports the whole document as visible; send the scrolled range once the host reads visibleRanges for something.
        "visibleRanges": [{ "startLineNumber": 1, "startColumn": 1, "endLineNumber": last_line, "endColumn": utf16_len(&document.lines[last_line - 1]) + 1 }],
        "editorPosition": 0,
    })
}

/// An empty `ISelection` at the cursor, 1-based with UTF-16 columns as the text model counts them.
fn selection(editor: &OpenEditor) -> Value {
    let (line, column) = editor.cursor;
    let prefix: String = editor.document.lines[line].chars().take(column).collect();
    let (line, column) = (line + 1, utf16_len(&prefix) + 1);
    json!({ "selectionStartLineNumber": line, "selectionStartColumn": column, "positionLineNumber": line, "positionColumn": column })
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn tab_dto(editor: &OpenEditor, active: bool) -> Value {
    let uri = file_uri(&editor.document.path);
    json!({
        "id": format!("0~default-workbench.editors.files.fileEditorInput-{} ", uri_string(&editor.document.path)),
        "label": editor.document.name(),
        // TabInputKind.TextInput
        "input": { "kind": 1, "uri": uri },
        "editorId": "default",
        "isActive": active,
        "isPinned": false,
        "isPreview": editor.preview,
        "isDirty": false,
    })
}

/// `URI.file(path).toString()`: the path percent-encoded except for unreserved characters and `/`.
fn uri_string(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::editor::{Indentation, TextDocument};

    fn document(path: &str, language_id: &str) -> TextDocument {
        TextDocument {
            path: PathBuf::from(path),
            lines: vec!["héllo".into(), String::new()],
            eol: "\n",
            encoding: "utf8",
            language_id: language_id.into(),
            indentation: Indentation { tab_size: 4, insert_spaces: true },
            off_side: false,
            tokens: Default::default(),
        }
    }

    fn drain(receiver: &Receiver<HostMessage>) -> Vec<HostMessage> {
        receiver.try_iter().collect()
    }

    #[test]
    fn sends_documents_editors_tabs_and_language_activation_as_they_change() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();

        group.open(document("/w/a b.txt", "plaintext"), false);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$activateByEvent", "$activateByEvent", "$acceptDocumentsAndEditorsDelta", "$acceptEditorTabModel"]);
        assert_eq!(sent[0].args, [json!("onLanguage:plaintext"), json!(0)]);
        let delta = &sent[2].args[0];
        assert_eq!(delta["addedDocuments"][0]["lines"], json!(["héllo", ""]));
        assert_eq!(delta["addedEditors"][0]["id"], "vs.editor.ICodeEditor:1,$model1");
        assert_eq!(delta["addedEditors"][0]["visibleRanges"][0]["endLineNumber"], 2);
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");
        assert!(delta.get("removedEditors").is_none());
        let tab = &sent[3].args[0][0]["tabs"][0];
        assert_eq!(tab["id"], "0~default-workbench.editors.files.fileEditorInput-file:///w/a%20b.txt ");
        assert_eq!(tab["isActive"], true);

        // A second document of the same language needs no activation; the editor switches models.
        group.open(document("/w/b.txt", "plaintext"), true);
        group.active_mut().unwrap().cursor = (0, 2);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        assert_eq!(sent.len(), 2);
        let delta = &sent[0].args[0];
        assert_eq!(delta["removedEditors"], json!(["vs.editor.ICodeEditor:1,$model1"]));
        assert_eq!(delta["addedEditors"][0]["selections"][0]["positionColumn"], 3);
        assert_eq!(sent[1].args[0][0]["tabs"][1]["isPreview"], true);

        group.close(1);
        sync.send_changes(&group);
        let delta = &drain(&receiver)[0].args[0];
        assert_eq!(delta["removedDocuments"][0]["path"], "/w/b.txt");
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");

        group.close(0);
        sync.send_changes(&group);
        let delta = &drain(&receiver)[0].args[0];
        assert_eq!(delta["newActiveEditor"], Value::Null);
        assert!(delta.get("addedEditors").is_none());
    }

    #[test]
    fn sends_the_color_theme_type_the_host_reads() {
        let (sender, receiver) = channel();
        HostSync::new(sender).send_color_theme(ThemeKind::HighContrastLight);
        let sent = drain(&receiver);
        assert_eq!((sent[0].proxy, sent[0].method), ("ExtHostTheming", "$onColorThemeChange"));
        assert_eq!(sent[0].args, [json!("hcLight")]);
    }
}
