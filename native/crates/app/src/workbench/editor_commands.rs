//! The editor commands in the command registry under their upstream ids (coreCommands.ts,
//! linesOperations.ts, clipboard.ts, multicursor.ts, folding.ts), so keybindings and
//! `vscode.commands.executeCommand` run them.

use config::configuration::Overrides;
use gpui::{ClipboardEntry, ClipboardItem, Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{CaseTransform, ContentChanged, Copied, LineCommentKind, OpenEditor, Selection, commands};

type EditorCommand = fn(&mut OpenEditor) -> Option<ContentChanged>;

/// Cursor and edit commands: `EditorCommand`s, which run on the focused editor or else the active one.
const EDITOR_COMMANDS: [(&str, EditorCommand); 44] = [
    ("cursorLeft", |editor| { editor.move_horizontally(false, false); None }),
    ("cursorLeftSelect", |editor| { editor.move_horizontally(false, true); None }),
    ("cursorRight", |editor| { editor.move_horizontally(true, false); None }),
    ("cursorRightSelect", |editor| { editor.move_horizontally(true, true); None }),
    ("cursorUp", |editor| { editor.move_vertically(false, false); None }),
    ("cursorUpSelect", |editor| { editor.move_vertically(false, true); None }),
    ("cursorDown", |editor| { editor.move_vertically(true, false); None }),
    ("cursorDownSelect", |editor| { editor.move_vertically(true, true); None }),
    ("cursorWordLeft", |editor| { editor.move_word(false, false); None }),
    ("cursorWordLeftSelect", |editor| { editor.move_word(false, true); None }),
    ("cursorWordEndRight", |editor| { editor.move_word(true, false); None }),
    ("cursorWordEndRightSelect", |editor| { editor.move_word(true, true); None }),
    ("cursorHome", |editor| { editor.move_to_line_edge(false, false); None }),
    ("cursorHomeSelect", |editor| { editor.move_to_line_edge(false, true); None }),
    ("cursorEnd", |editor| { editor.move_to_line_edge(true, false); None }),
    ("cursorEndSelect", |editor| { editor.move_to_line_edge(true, true); None }),
    ("cursorTop", |editor| { editor.move_to_document_edge(false, false); None }),
    ("cursorTopSelect", |editor| { editor.move_to_document_edge(false, true); None }),
    ("cursorBottom", |editor| { editor.move_to_document_edge(true, false); None }),
    ("cursorBottomSelect", |editor| { editor.move_to_document_edge(true, true); None }),
    ("deleteLeft", OpenEditor::delete_left),
    ("deleteRight", OpenEditor::delete_right),
    ("deleteWordLeft", OpenEditor::delete_word_left),
    ("deleteWordRight", OpenEditor::delete_word_right),
    ("deleteAllLeft", OpenEditor::delete_all_left),
    ("deleteAllRight", OpenEditor::delete_all_right),
    ("tab", OpenEditor::tab),
    ("outdent", OpenEditor::outdent),
    ("editor.action.copyLinesUpAction", |editor| editor.copy_lines(false)),
    ("editor.action.copyLinesDownAction", |editor| editor.copy_lines(true)),
    ("editor.action.duplicateSelection", OpenEditor::duplicate_selection),
    ("editor.action.moveLinesUpAction", |editor| editor.move_lines(false)),
    ("editor.action.moveLinesDownAction", |editor| editor.move_lines(true)),
    ("editor.action.sortLinesAscending", |editor| editor.sort_lines(false)),
    ("editor.action.sortLinesDescending", |editor| editor.sort_lines(true)),
    ("editor.action.removeDuplicateLines", OpenEditor::delete_duplicate_lines),
    ("editor.action.reverseLines", OpenEditor::reverse_lines),
    ("editor.action.deleteLines", OpenEditor::delete_lines),
    ("editor.action.indentLines", OpenEditor::indent_lines),
    ("editor.action.outdentLines", OpenEditor::outdent),
    ("editor.action.insertLineBefore", OpenEditor::insert_line_before),
    ("editor.action.insertLineAfter", OpenEditor::insert_line_after),
    ("editor.action.joinLines", OpenEditor::join_lines),
    ("editor.action.transpose", OpenEditor::transpose),
];

/// Commands of the editor with text focus only (`MultiCommand`s and `EditorHandlerCommand`s): elsewhere they
/// belong to the focused input element, the quick input's.
const FOCUSED_EDITOR_COMMANDS: [(&str, EditorCommand); 3] = [
    ("undo", OpenEditor::undo),
    ("redo", OpenEditor::redo),
    ("editor.action.selectAll", |editor| { editor.select_all(); None }),
];

impl Workbench {
    pub(super) fn register_editor_commands(&mut self) {
        for (id, command) in EDITOR_COMMANDS {
            self.commands.register(id, move |workbench, _, _, cx| workbench.run_on_active_editor(command, cx));
        }
        for (id, command) in FOCUSED_EDITOR_COMMANDS {
            self.commands.register(id, move |workbench, _, window, cx| match workbench.editor_focus.is_focused(window) {
                true => workbench.run_on_active_editor(command, cx),
                false => workbench.input_text_command(id, window, cx).or_else(|| workbench.explorer_undo(id, window, cx)).unwrap_or(Outcome::done()),
            });
        }
        self.commands.register("type", |workbench, args, window, cx| {
            let Some(text) = args.first().and_then(|args| args["text"].as_str()).map(str::to_owned) else { return Outcome::done() };
            if !workbench.editor_focus.is_focused(window) {
                return Outcome::done();
            }
            // The editor's own Enter: a typed line break keeps the line's indentation.
            match text.as_str() {
                "\n" => workbench.run_on_active_editor(OpenEditor::enter, cx),
                _ => workbench.run_on_active_editor(move |editor: &mut OpenEditor| editor.type_text(&text), cx),
            }
        });
        for &id in commands::IDS {
            self.commands.register(id, move |workbench, _, _, cx| {
                if workbench.editors.active().is_some() {
                    workbench.run_editor_command(id, "keyboard", cx);
                }
                Outcome::done()
            });
        }
        for (id, transform) in CaseTransform::ALL {
            self.commands.register(id, move |workbench, _, _, cx| workbench.run_on_active_editor(|editor: &mut OpenEditor| editor.transform_case(transform), cx));
        }
        let comment_actions = [
            ("editor.action.commentLine", Some(LineCommentKind::Toggle)),
            ("editor.action.addCommentLine", Some(LineCommentKind::ForceAdd)),
            ("editor.action.removeCommentLine", Some(LineCommentKind::ForceRemove)),
            ("editor.action.blockComment", None),
        ];
        for (id, kind) in comment_actions {
            self.commands.register(id, move |workbench, _, _, cx| workbench.run_comment_action(kind, cx));
        }
        self.commands.register("editor.action.clipboardCopyAction", |workbench, _, window, cx| workbench.copy_to_clipboard(false, window, cx));
        self.commands.register("editor.action.clipboardCutAction", |workbench, _, window, cx| workbench.copy_to_clipboard(true, window, cx));
        self.commands.register("editor.action.clipboardPasteAction", |workbench, _, window, cx| {
            if !workbench.editor_focus.is_focused(window) {
                return workbench.input_text_command("editor.action.clipboardPasteAction", window, cx).unwrap_or(Outcome::done());
            }
            let Some(ClipboardEntry::String(clipboard)) = cx.read_from_clipboard().and_then(|item| item.entries().first().cloned()) else { return Outcome::done() };
            let metadata = clipboard.metadata_json::<Value>().unwrap_or_default();
            let multicursor_text: Option<Vec<String>> = serde_json::from_value(metadata["multicursorText"].clone()).ok();
            let from_empty_selection = metadata["isFromEmptySelection"] == json!(true);
            workbench.run_on_active_editor(|editor: &mut OpenEditor| editor.paste(clipboard.text(), from_empty_selection, multicursor_text.as_deref()), cx)
        });
    }

    fn copy_to_clipboard(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active().filter(|_| self.editor_focus.is_focused(window)) else {
            let command = if cut { "editor.action.clipboardCutAction" } else { "editor.action.clipboardCopyAction" };
            return self.input_text_command(command, window, cx).unwrap_or(Outcome::done());
        };
        cx.write_to_clipboard(clipboard_item(editor.copy_text()));
        match cut {
            true => self.run_on_active_editor(OpenEditor::cut, cx),
            false => Outcome::done(),
        }
    }

    /// Runs an editor command, then tells the host and scrolls the cursor into view as a key press does.
    pub(super) fn run_on_active_editor(&mut self, command: impl FnOnce(&mut OpenEditor) -> Option<ContentChanged>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let selections_before = editor.selections.clone();
        let change = command(editor);
        self.editor_command_ran(change, selections_before, cx)
    }

    fn editor_command_ran(&mut self, change: Option<ContentChanged>, selections_before: Vec<Selection>, cx: &mut Context<Self>) -> Outcome {
        let moved = self.editors.active().is_some_and(|editor| editor.selections != selections_before);
        self.marked_text = None;
        self.editor_edited(change, moved, "keyboard", cx);
        Outcome::Ready(Ok(Value::Null))
    }

    /// comment.ts `CommentLineAction` and `BlockCommentAction` with the `editor.comments` options, on the
    /// language at each cursor.
    fn run_comment_action(&mut self, kind: Option<LineCommentKind>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let overrides = Overrides { resource: Some(&editor.document.path), override_identifier: Some(&editor.document.language_id) };
        let settings = self.settings.lock().unwrap();
        let insert_space = settings.get_value(Some("editor.comments.insertSpace"), &overrides) != Some(json!(false));
        let ignore_empty_lines = settings.get_value(Some("editor.comments.ignoreEmptyLines"), &overrides) != Some(json!(false));
        drop(settings);
        let selections_before = editor.selections.clone();
        let change = match kind {
            Some(kind) => editor.comment_lines(kind, insert_space, ignore_empty_lines, &self.languages),
            None => editor.block_comment(insert_space, &self.languages),
        };
        self.editor_command_ran(change, selections_before, cx)
    }
}

/// The copied text with the `ClipboardStoredMetadata` paste reads back.
fn clipboard_item(copied: Copied) -> ClipboardItem {
    let metadata = json!({ "isFromEmptySelection": copied.from_empty_selection, "multicursorText": copied.multicursor_text });
    ClipboardItem::new_string_with_json_metadata(copied.text, metadata)
}
