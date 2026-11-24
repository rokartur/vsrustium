//! What the main thread tells the extension host about open text documents, the visible editor and the
//! tabs: mainThreadDocumentsAndEditors.ts (`$acceptDocumentsAndEditorsDelta`), mainThreadEditorTabs.ts
//! (`$acceptTabOperation`), mainThreadEditors.ts (`$acceptEditorPropertiesChanged`) and the
//! `onLanguage` activation languageService.ts sends when a model of a new language is created.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::editor::{ContentChanged, EditReason, EditorGroup, OpenEditor, Position, Selection};
use crate::theme::ThemeKind;

#[derive(Debug)]
pub struct HostMessage {
    pub proxy: &'static str,
    pub method: &'static str,
    pub args: Vec<Value>,
}

pub struct HostSync {
    sender: Sender<HostMessage>,
    /// The documents the host has, in the order they were added: their URI string and `UriComponents`.
    documents: Vec<(String, Value)>,
    /// The snapshot id of the visible editor the host has.
    editor: Option<String>,
    /// The lines of the visible editor the host has as its `visibleRanges`.
    visible_lines: Option<Range<usize>>,
    /// The tabs of group 0 the host has; session.rs starts it with an empty group.
    tabs: Vec<Value>,
    activation_events: HashSet<String>,
}

/// `TabModelOperationKind`.
const TAB_OPEN: u8 = 0;
const TAB_CLOSE: u8 = 1;
const TAB_UPDATE: u8 = 2;
const TAB_MOVE: u8 = 3;

/// The one editor group's code editor widget; it is reused for every model the group shows.
const CODE_EDITOR_ID: &str = "vs.editor.ICodeEditor:1";

impl HostSync {
    pub fn new(sender: Sender<HostMessage>) -> Self {
        HostSync { sender, documents: Vec::new(), editor: None, visible_lines: None, tabs: Vec::new(), activation_events: HashSet::new() }
    }

    /// Sends what changed in `group` since the last call: activation for new languages, the document
    /// and editor delta, then the tab model.
    pub fn send_changes(&mut self, group: &EditorGroup) {
        let open: Vec<&OpenEditor> = group.editors.iter().chain(&group.models).collect();
        let added: Vec<&OpenEditor> = open.iter().copied().filter(|editor| !self.documents.iter().any(|(uri, _)| *uri == editor.document.uri_string())).collect();
        let removed: Vec<(String, Value)> =
            self.documents.iter().filter(|(uri, _)| !open.iter().any(|editor| editor.document.uri_string() == *uri)).cloned().collect();
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
            delta.insert("removedDocuments".into(), removed.iter().map(|(_, uri)| uri.clone()).collect());
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
                let visible_lines = editor.completely_visible_lines(group.viewport);
                delta.insert("addedEditors".into(), json!([text_editor_add_data(editor, visible_lines.clone())]));
                self.visible_lines = Some(visible_lines);
            }
            delta.insert("newActiveEditor".into(), json!(active_id));
        }
        if !delta.is_empty() {
            self.send("ExtHostDocumentsAndEditors", "$acceptDocumentsAndEditorsDelta", vec![Value::Object(delta)]);
        }
        self.documents.retain(|document| !removed.contains(document));
        self.documents.extend(added.iter().map(|editor| (editor.document.uri_string(), editor.document.uri())));
        self.editor = active_id;
        self.send_tabs(group);
    }

    /// One `$acceptTabOperation` per tab that closed, opened, moved or changed, as mainThreadEditorTabs.ts
    /// sends them: a whole `$acceptEditorTabModel` fires no `onDidChangeTabs`, which TypeScript's
    /// TabResourceTracker needs before it reports diagnostics.
    fn send_tabs(&mut self, group: &EditorGroup) {
        let tabs: Vec<Value> = group.editors.iter().enumerate().map(|(index, editor)| tab_dto(editor, group.active_index() == Some(index))).collect();
        let mut sent = std::mem::take(&mut self.tabs);
        for index in (0..sent.len()).rev() {
            if !tabs.iter().any(|tab| tab["id"] == sent[index]["id"]) {
                let closed = sent.remove(index);
                self.send_tab_operation(TAB_CLOSE, index, closed, None);
            }
        }
        for (index, tab) in tabs.iter().enumerate() {
            match sent.iter().position(|old| old["id"] == tab["id"]) {
                None => {
                    sent.insert(index, tab.clone());
                    self.send_tab_operation(TAB_OPEN, index, tab.clone(), None);
                }
                Some(old_index) if old_index != index => {
                    let moved = sent.remove(old_index);
                    sent.insert(index, tab.clone());
                    self.send_tab_operation(TAB_MOVE, index, moved.clone(), Some(old_index));
                    if moved != *tab {
                        self.send_tab_operation(TAB_UPDATE, index, tab.clone(), None);
                    }
                }
                Some(_) if sent[index] != *tab => {
                    sent[index] = tab.clone();
                    self.send_tab_operation(TAB_UPDATE, index, tab.clone(), None);
                }
                Some(_) => {}
            }
        }
        self.tabs = sent;
    }

    fn send_tab_operation(&self, kind: u8, index: usize, tab: Value, old_index: Option<usize>) {
        let mut operation = json!({ "groupId": 0, "index": index, "tabDto": tab, "kind": kind });
        if let Some(old_index) = old_index {
            operation["oldIndex"] = json!(old_index);
        }
        self.send("ExtHostEditorTabs", "$acceptTabOperation", vec![operation]);
    }

    /// Tells the host the active editor's text changed (mainThreadDocuments.ts ModelTracker), then the
    /// dirty state and tabs when it flipped, then where the cursor went.
    pub fn send_content_change(&mut self, group: &EditorGroup, change: &ContentChanged) {
        let editor = group.active().expect("only the active editor is edited");
        let document = &editor.document;
        let uri = document.uri();
        let changes: Vec<Value> = change
            .changes
            .iter()
            .map(|change| {
                json!({
                    "range": { "startLineNumber": change.start_line, "startColumn": change.start_column, "endLineNumber": change.end_line, "endColumn": change.end_column },
                    "rangeOffset": change.range_offset,
                    "rangeLength": change.range_length,
                    "text": change.text,
                })
            })
            .collect();
        let event = json!({
            "changes": changes,
            "eol": document.eol,
            "versionId": change.version_id,
            "isUndoing": change.is_undoing,
            "isRedoing": change.is_redoing,
            "isFlush": false,
            "isEolChange": false,
            "detailedReason": detailed_reason(change.reason),
        });
        let dirty = editor.is_dirty();
        self.send("ExtHostDocuments", "$acceptModelChanged", vec![uri.clone(), event, json!(dirty)]);
        if dirty != change.was_dirty {
            self.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![uri, json!(dirty)]);
            self.send_tabs(group);
        }
        self.send_selection(editor, "keyboard");
    }

    /// The active editor was written to disk while `was_dirty`.
    pub fn send_saved(&mut self, group: &EditorGroup, was_dirty: bool) {
        let editor = group.active().expect("only the active editor is saved");
        let uri = editor.document.uri();
        if was_dirty {
            self.send("ExtHostDocuments", "$acceptDirtyStateChanged", vec![uri.clone(), json!(false)]);
            self.send_tabs(group);
        }
        self.send("ExtHostDocuments", "$acceptModelSaved", vec![uri]);
    }

    /// mainThreadEditor.ts `_updatePropertiesNow` on a scroll or layout: the visible lines, when they changed.
    pub fn send_visible_ranges(&mut self, group: &EditorGroup) {
        let Some(editor) = group.active() else { return };
        let lines = editor.completely_visible_lines(group.viewport);
        if self.visible_lines.as_ref() == Some(&lines) {
            return;
        }
        let ranges = visible_ranges(editor, lines.clone());
        self.visible_lines = Some(lines);
        self.send("ExtHostEditors", "$acceptEditorPropertiesChanged", vec![json!(snapshot_id(editor)), json!({ "options": null, "selections": null, "visibleRanges": ranges })]);
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
        "uri": document.uri(),
        "versionId": editor.version_id,
        "lines": document.lines,
        "EOL": document.eol,
        "languageId": document.language_id,
        "isDirty": editor.is_dirty(),
        "encoding": document.encoding,
    })
}

fn text_editor_add_data(editor: &OpenEditor, visible_lines: Range<usize>) -> Value {
    let document = &editor.document;
    let indentation = document.indentation;
    json!({
        "id": snapshot_id(editor),
        "documentUri": document.uri(),
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
        "visibleRanges": visible_ranges(editor, visible_lines),
        "editorPosition": 0,
    })
}

/// `getVisibleRanges` without folding: one range over whole lines.
fn visible_ranges(editor: &OpenEditor, lines: Range<usize>) -> Value {
    let last = lines.end - 1;
    json!([{ "startLineNumber": lines.start + 1, "startColumn": 1, "endLineNumber": last + 1, "endColumn": utf16_len(&editor.document.lines[last]) + 1 }])
}

/// `ISelection`, 1-based with UTF-16 columns as the text model counts them.
fn selection(editor: &OpenEditor) -> Value {
    let lines = &editor.document.lines;
    let column = |position: Position| utf16_len(&lines[position.line].chars().take(position.column).collect::<String>()) + 1;
    let Selection { anchor, head } = editor.selection;
    json!({ "selectionStartLineNumber": anchor.line + 1, "selectionStartColumn": column(anchor), "positionLineNumber": head.line + 1, "positionColumn": column(head) })
}

/// `TextModelEditSource.metadata` (textModelEditSource.ts).
fn detailed_reason(reason: EditReason) -> Value {
    match reason {
        EditReason::Cursor { kind, detailed_source } => json!({ "source": "cursor", "kind": kind, "detailedSource": detailed_source }),
        EditReason::Unknown(name) => json!({ "source": "unknown", "name": name }),
        EditReason::ApplyEdits => json!({ "source": "applyEdits" }),
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn tab_dto(editor: &OpenEditor, active: bool) -> Value {
    let uri = editor.document.uri();
    // Every scheme the file service reads opens as a file editor input; untitled text has its own.
    let type_id = if editor.document.scheme == "untitled" { "workbench.editors.untitledEditorInput" } else { "workbench.editors.files.fileEditorInput" };
    json!({
        "id": format!("0~default-{type_id}-{} ", editor.document.uri_string()),
        "label": editor.document.name(),
        // TabInputKind.TextInput
        "input": { "kind": 1, "uri": uri },
        "editorId": "default",
        "isActive": active,
        "isPinned": false,
        "isPreview": editor.preview,
        "isDirty": editor.is_dirty(),
    })
}


#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::editor::{Indentation, TextDocument, Viewport};

    fn document(path: &str, language_id: &str) -> TextDocument {
        TextDocument {
            scheme: "file".into(),
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
        group.viewport = Viewport { width: 400., height: 360., char_width: 7.2 };

        group.open(document("/w/a b.txt", "plaintext"), false);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$activateByEvent", "$activateByEvent", "$acceptDocumentsAndEditorsDelta", "$acceptTabOperation"]);
        assert_eq!(sent[0].args, [json!("onLanguage:plaintext"), json!(0)]);
        let delta = &sent[2].args[0];
        assert_eq!(delta["addedDocuments"][0]["lines"], json!(["héllo", ""]));
        assert_eq!(delta["addedEditors"][0]["id"], "vs.editor.ICodeEditor:1,$model1");
        assert_eq!(delta["addedEditors"][0]["visibleRanges"][0]["endLineNumber"], 2);
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");
        assert!(delta.get("removedEditors").is_none());
        let operation = &sent[3].args[0];
        assert_eq!((operation["kind"].clone(), operation["index"].clone()), (json!(TAB_OPEN), json!(0)));
        let tab = &operation["tabDto"];
        assert_eq!(tab["id"], "0~default-workbench.editors.files.fileEditorInput-file:///w/a%20b.txt ");
        assert_eq!(tab["isActive"], true);

        // A second document of the same language needs no activation; the editor switches models.
        group.open(document("/w/b.txt", "plaintext"), true);
        group.active_mut().unwrap().move_to(Position::new(0, 2), false);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$acceptDocumentsAndEditorsDelta", "$acceptTabOperation", "$acceptTabOperation"]);
        let delta = &sent[0].args[0];
        assert_eq!(delta["removedEditors"], json!(["vs.editor.ICodeEditor:1,$model1"]));
        assert_eq!(delta["addedEditors"][0]["selections"][0]["positionColumn"], 3);
        assert_eq!((sent[1].args[0]["kind"].clone(), sent[1].args[0]["tabDto"]["isActive"].clone()), (json!(TAB_UPDATE), json!(false)));
        assert_eq!((sent[2].args[0]["kind"].clone(), sent[2].args[0]["index"].clone()), (json!(TAB_OPEN), json!(1)));
        assert_eq!(sent[2].args[0]["tabDto"]["isPreview"], true);

        group.close(1);
        sync.send_changes(&group);
        let sent = drain(&receiver);
        assert_eq!((sent[1].args[0]["kind"].clone(), sent[1].args[0]["index"].clone()), (json!(TAB_CLOSE), json!(1)));
        assert_eq!((sent[2].args[0]["kind"].clone(), sent[2].args[0]["tabDto"]["isActive"].clone()), (json!(TAB_UPDATE), json!(true)));
        let delta = &sent[0].args[0];
        assert_eq!(delta["removedDocuments"][0]["path"], "/w/b.txt");
        assert_eq!(delta["newActiveEditor"], "vs.editor.ICodeEditor:1,$model1");

        group.close(0);
        sync.send_changes(&group);
        let delta = &drain(&receiver)[0].args[0];
        assert_eq!(delta["newActiveEditor"], Value::Null);
        assert!(delta.get("addedEditors").is_none());
    }

    #[test]
    fn sends_content_changes_with_the_dirty_state_when_it_flips() {
        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let mut group = EditorGroup::default();
        group.open(document("/w/a.txt", "plaintext"), true);
        sync.send_changes(&group);
        drain(&receiver);
        let editor = group.active_mut().unwrap();
        editor.move_to(Position::new(0, 5), false);
        let change = editor.type_text("!").unwrap();
        sync.send_content_change(&group, &change);
        let sent = drain(&receiver);
        let methods: Vec<_> = sent.iter().map(|message| message.method).collect();
        assert_eq!(methods, ["$acceptModelChanged", "$acceptDirtyStateChanged", "$acceptTabOperation", "$acceptEditorPropertiesChanged"]);
        let event = &sent[0].args[1];
        assert_eq!(event["changes"][0]["range"], json!({ "startLineNumber": 1, "startColumn": 6, "endLineNumber": 1, "endColumn": 6 }));
        assert_eq!((event["changes"][0]["rangeOffset"].clone(), event["versionId"].clone()), (json!(5), json!(2)));
        assert_eq!(event["detailedReason"], json!({ "source": "cursor", "kind": "type", "detailedSource": "keyboard" }));
        assert_eq!(sent[0].args[2], true);
        let tab = &sent[2].args[0]["tabDto"];
        assert_eq!((tab["isDirty"].clone(), tab["isPreview"].clone()), (json!(true), json!(false)));
        assert_eq!(sent[3].args[1]["selections"]["selections"][0]["positionColumn"], 7);

        // Still dirty: no dirty state or tab update.
        let change = group.active_mut().unwrap().type_text("?").unwrap();
        sync.send_content_change(&group, &change);
        assert_eq!(drain(&receiver).len(), 2);

        let change = group.active_mut().unwrap().undo().unwrap();
        sync.send_content_change(&group, &change);
        let sent = drain(&receiver);
        assert_eq!((sent[0].args[1]["isUndoing"].clone(), sent[0].args[1]["detailedReason"].clone()), (json!(true), json!({ "source": "applyEdits" })));
        assert_eq!(sent[1].args[1], false);
    }

    /// The real host on Bun takes every message of an open, an edit and a save without an error reply,
    /// and TypeScript reports diagnostics for the text the edit made.
    #[test]
    #[ignore = "starts the extension host: VSCODE_GPUI_EXTENSIONS=<built extensions> cargo test -p app -- --ignored live_host"]
    fn live_host_takes_edits_and_typescript_sees_the_new_text() {
        use exthost::rpc::Arg;
        use exthost::scan::scan_builtin_extensions;
        use exthost::session::{Session, SessionPaths, init_data, workspace_data};
        use exthost::{ExtensionHost, HostOptions, Runtime};
        use std::time::{Duration, Instant};

        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let folder = std::env::temp_dir().join(format!("code-gpui-live-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("src")).unwrap();
        std::fs::write(folder.join("src/main.ts"), "export const answer = 42;\n").unwrap();
        // Built extensions, as main.rs reads them; port/tools/pixel-check points this at the reference checkout.
        let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
        let extensions = scan_builtin_extensions(&extensions_dir).unwrap();
        let user_data = folder.join(".user-data");
        let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace_folder: Some(&folder) };
        let init = init_data(&paths, &extensions, "1.147.0");
        let entry = app_root.join("out-exthost/bootstrap-exthost.js");
        let host = ExtensionHost::start(HostOptions { runtime: Runtime::Bun, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init).unwrap();
        let mut session = Session::new(host);
        session.start(&extensions, Some(workspace_data(&init, &folder))).unwrap();

        let (sender, receiver) = channel();
        let mut sync = HostSync::new(sender);
        let forward = |session: &mut Session| {
            for message in receiver.try_iter() {
                let args = message.args.into_iter().map(Arg::Json).collect();
                session.call(message.proxy, message.method, args, Duration::from_secs(30)).unwrap_or_else(|error| panic!("{error:#}"));
            }
        };
        let diagnostics = |session: &Session| session.log.host_calls.get("MainThreadDiagnostics.$changeMany").copied().unwrap_or(0);
        let pump_until = |session: &mut Session, seconds: u64, done: &dyn Fn(&Session) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while !done(session) && Instant::now() < deadline {
                session.pump(Duration::from_millis(100)).unwrap();
            }
        };

        let languages = crate::languages::Languages::new(&extensions);
        let mut group = EditorGroup::default();
        group.open(TextDocument::read(&folder.join("src/main.ts"), &languages).unwrap(), false);
        sync.send_changes(&group);
        forward(&mut session);
        pump_until(&mut session, 20, &|session| session.log.activated.iter().any(|id| id == "vscode.typescript-language-features"));
        assert!(session.log.activated.iter().any(|id| id == "vscode.typescript-language-features"), "{:#?}", session.log.activation_errors);
        // Whatever TypeScript reports for the clean file comes first.
        pump_until(&mut session, 10, &|_| false);
        let before = diagnostics(&session);

        let editor = group.active_mut().unwrap();
        editor.move_to(Position::new(1, 0), false);
        let change = editor.type_text("const broken: number = 'text';\n").unwrap();
        sync.send_content_change(&group, &change);
        forward(&mut session);
        pump_until(&mut session, 60, &|session| diagnostics(session) > before);
        assert!(diagnostics(&session) > before, "no diagnostics after the edit; console: {:#?}", session.log.console);

        let was_dirty = group.active().unwrap().is_dirty();
        group.active_mut().unwrap().save().unwrap();
        sync.send_saved(&group, was_dirty);
        forward(&mut session);
        assert!(!session.log.console.iter().any(|line| line.starts_with("[unexpected]")), "{:#?}", session.log.console);
        std::fs::remove_dir_all(folder).unwrap();
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
