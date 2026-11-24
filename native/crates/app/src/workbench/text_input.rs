//! Keyboard and text input of the code editor: the macOS default keybindings of the cursor, edit and
//! clipboard commands (coreCommands.ts, the editor contributions' `primary`/`mac` keys), and the
//! platform text input (IME, dead keys) through GPUI's input handler.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, Pixels, UTF16Selection, Window,
};

use super::Workbench;
use serde_json::{Value, json};

use crate::editor::{ContentChanged, Copied, Position, commands, utf16_column};

impl Workbench {
    /// Runs the command bound to the key; unbound keys go on to the input handler as text.
    /// shortcut: Page Up/Down and Ctrl+Emacs keys are not bound yet.
    pub(super) fn editor_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if commands::for_key(&event.keystroke.unparse()).any(|command| self.run_editor_command(command, "keyboard", cx)) {
            cx.stop_propagation();
            self.marked_text = None;
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.function {
            return;
        }
        let (cmd, alt, select) = (modifiers.platform, modifiers.alt, modifiers.shift);
        let editor = self.editors.active_mut().expect("the text area has focus only with an editor open");
        let selections_before = editor.selections.clone();
        let change = match (cmd, alt, event.keystroke.key.as_str()) {
            (false, false, "left" | "right") => {
                editor.move_horizontally(event.keystroke.key == "right", select);
                None
            }
            (false, false, "up" | "down") => {
                editor.move_vertically(event.keystroke.key == "down", select);
                None
            }
            (false, true, "left" | "right") => {
                editor.move_word(event.keystroke.key == "right", select);
                None
            }
            (true, false, "left" | "right") | (false, false, "home" | "end") => {
                editor.move_to_line_edge(matches!(event.keystroke.key.as_str(), "right" | "end"), select);
                None
            }
            (true, false, "up" | "down") => {
                editor.move_to_document_edge(event.keystroke.key == "down", select);
                None
            }
            (true, false, "a") if !select => {
                editor.select_all();
                None
            }
            (false, false, "backspace") => editor.delete_left(),
            (false, false, "delete") => editor.delete_right(),
            (false, true, "backspace") => editor.delete_word_left(),
            (false, true, "delete") => editor.delete_word_right(),
            (true, false, "backspace") => editor.delete_all_left(),
            (true, false, "delete") => editor.delete_all_right(),
            (false, false, "enter") => editor.enter(),
            (false, false, "tab") if select => editor.outdent(),
            (false, false, "tab") => editor.tab(),
            (true, false, "z") if select => editor.redo(),
            (true, false, "z") => editor.undo(),
            (true, false, "y") => editor.redo(),
            (true, false, "c") if !select => {
                cx.write_to_clipboard(clipboard_item(editor.copy_text()));
                None
            }
            (true, false, "x") if !select => {
                cx.write_to_clipboard(clipboard_item(editor.copy_text()));
                editor.cut()
            }
            (true, false, "v") if !select => {
                let Some(ClipboardEntry::String(clipboard)) = cx.read_from_clipboard().and_then(|item| item.entries().first().cloned()) else { return };
                let metadata = clipboard.metadata_json::<Value>().unwrap_or_default();
                let multicursor_text: Option<Vec<String>> = serde_json::from_value(metadata["multicursorText"].clone()).ok();
                editor.paste(clipboard.text(), metadata["isFromEmptySelection"] == json!(true), multicursor_text.as_deref())
            }
            (true, false, "s") if !select => {
                cx.stop_propagation();
                self.save_active_editor(cx);
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.marked_text = None;
        let moved = self.editors.active().expect("still open").selections != selections_before;
        self.editor_edited(change, moved, "keyboard", cx);
    }

    /// Tells the host about a content change or a cursor move, and reveals the cursor (cursor.ts `revealAll`).
    pub(super) fn editor_edited(&mut self, change: Option<ContentChanged>, moved: bool, source: &str, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("only the active editor is edited");
        let scroll = editor.scroll;
        editor.reveal_cursors(source, viewport);
        let scrolled = editor.scroll != scroll;
        match &change {
            Some(change) => self.host.send_content_change(&self.editors, change),
            None if moved => self.host.send_selection(self.editors.active().expect("still open"), source),
            None => {}
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// shortcut: save writes the text as it is; no save participants (`$participateInSave`), no format on save,
    /// no conflict check against the file on disk.
    fn save_active_editor(&mut self, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("only the active editor is saved");
        let was_dirty = editor.is_dirty();
        if let Err(error) = editor.save() {
            // shortcut: a save error is logged, not shown as the reference's notification.
            eprintln!("saving {}: {error}", editor.document.path.display());
            return;
        }
        self.host.send_saved(&self.editors, self.editors.active().expect("just saved"), was_dirty);
        cx.notify();
    }

    /// A UTF-16 range of the primary cursor's line, the text the input handler works on, as the chars
    /// before and after the cursor it replaces (`replacePrevCharCnt`, `replaceNextCharCnt`).
    fn input_range(&self, range: Range<usize>) -> (usize, usize) {
        let editor = self.editors.active().expect("input goes to the active editor");
        let head = editor.selection().head;
        let line = &editor.document.lines[head.line];
        let (start, end) = (column_at_utf16(line, range.start), column_at_utf16(line, range.end));
        (head.column.saturating_sub(start), end.saturating_sub(head.column))
    }
}

/// The copied text with the `ClipboardStoredMetadata` paste reads back.
fn clipboard_item(copied: Copied) -> ClipboardItem {
    let metadata = json!({ "isFromEmptySelection": copied.from_empty_selection, "multicursorText": copied.multicursor_text });
    ClipboardItem::new_string_with_json_metadata(copied.text, metadata)
}

/// The platform's text input sees the cursor's line as the whole text.
/// shortcut: the IME sees one line, so reconversion across lines and candidate window placement
/// (`bounds_for_range`) are not supported; the candidate window opens at the system's default spot.
impl EntityInputHandler for Workbench {
    fn text_for_range(&mut self, range: Range<usize>, adjusted_range: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let editor = self.editors.active()?;
        let units: Vec<u16> = editor.document.lines[editor.selection().head.line].encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted_range = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let editor = self.editors.active()?;
        let selection = editor.selection();
        let line = &editor.document.lines[selection.head.line];
        let head = utf16_column(line, selection.head.column);
        let anchor = if selection.anchor.line == selection.head.line { utf16_column(line, selection.anchor.column) } else { head };
        Some(UTF16Selection { range: anchor.min(head)..anchor.max(head), reversed: head < anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.clone()
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_text = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.or(self.marked_text.take()).map(|range| self.input_range(range));
        let editor = self.editors.active_mut().expect("input goes to the active editor");
        let change = match range {
            None => editor.type_text(text),
            Some((previous, next)) => editor.type_replacing(text, previous, next, "type"),
        };
        self.editor_edited(change, false, "keyboard", cx);
    }

    /// IME composition: the marked text replaces the previous one and stays marked.
    /// shortcut: the caret goes to the end of the marked text, whatever selection the IME asks for.
    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new_text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let (previous, next) = range.or(self.marked_text.clone()).map_or((0, 0), |range| self.input_range(range));
        let editor = self.editors.active_mut().expect("input goes to the active editor");
        let primary = editor.selection();
        let start = if primary.is_empty() { Position::new(primary.head.line, primary.head.column.saturating_sub(previous)) } else { primary.start() };
        let change = editor.type_replacing(new_text, previous, next, "compositionType");
        let marked_start = utf16_column(&editor.document.lines[start.line], start.column);
        self.marked_text = (!new_text.is_empty()).then(|| marked_start..marked_start + new_text.encode_utf16().count());
        self.editor_edited(change, false, "keyboard", cx);
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// The char column at a UTF-16 offset of the line; an offset inside a surrogate pair rounds up.
fn column_at_utf16(line: &str, offset: usize) -> usize {
    let mut units = 0;
    for (column, c) in line.chars().enumerate() {
        if units >= offset {
            return column;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}
