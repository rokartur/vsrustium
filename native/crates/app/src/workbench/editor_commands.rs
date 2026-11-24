//! The M5 editor commands in the command registry under their upstream ids (coreCommands.ts,
//! linesOperations.ts, clipboard.ts), so keybindings and `vscode.commands.executeCommand` run them.

use gpui::{ClipboardEntry, ClipboardItem, Context, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::{ContentChanged, OpenEditor};

type EditorCommand = fn(&mut OpenEditor) -> Option<ContentChanged>;

/// Cursor and edit commands: `EditorCommand`s, which run on the focused editor or else the active one.
const EDITOR_COMMANDS: [(&str, EditorCommand); 28] = [
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
];

/// Commands of the editor with text focus only (`MultiCommand`s and `EditorHandlerCommand`s): elsewhere they
/// belong to the focused input, which the port has none of yet.
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
                false => Outcome::done(),
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
        self.commands.register("editor.action.clipboardCopyAction", |workbench, _, window, cx| workbench.copy_to_clipboard(false, window, cx));
        self.commands.register("editor.action.clipboardCutAction", |workbench, _, window, cx| workbench.copy_to_clipboard(true, window, cx));
        self.commands.register("editor.action.clipboardPasteAction", |workbench, _, window, cx| {
            if !workbench.editor_focus.is_focused(window) {
                return Outcome::done();
            }
            let Some(ClipboardEntry::String(clipboard)) = cx.read_from_clipboard().and_then(|item| item.entries().first().cloned()) else { return Outcome::done() };
            let whole_line = clipboard.metadata_json::<bool>().unwrap_or(false);
            workbench.run_on_active_editor(|editor: &mut OpenEditor| editor.paste(clipboard.text(), whole_line), cx)
        });
    }

    fn copy_to_clipboard(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active().filter(|_| self.editor_focus.is_focused(window)) else { return Outcome::done() };
        let (text, whole_line) = editor.copy_text();
        cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(text, whole_line));
        match cut {
            true => self.run_on_active_editor(OpenEditor::cut, cx),
            false => Outcome::done(),
        }
    }

    /// Runs an editor command, then tells the host and scrolls the cursor into view as a key press does.
    fn run_on_active_editor(&mut self, command: impl FnOnce(&mut OpenEditor) -> Option<ContentChanged>, cx: &mut Context<Self>) -> Outcome {
        let Some(editor) = self.editors.active_mut() else { return Outcome::done() };
        let selection_before = editor.selection;
        let change = command(editor);
        let moved = editor.selection != selection_before;
        self.marked_text = None;
        self.editor_edited(change, moved, "keyboard", cx);
        Outcome::Ready(Ok(Value::Null))
    }
}
