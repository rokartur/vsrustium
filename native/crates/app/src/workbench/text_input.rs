//! Keyboard and text input of the code editor: keys no keybinding took, and the platform text input
//! (IME, dead keys) through GPUI's input handler. Keybindings run before this (workbench/keyboard.rs).

use std::ops::Range;

use gpui::{Bounds, Context, EntityInputHandler, KeyDownEvent, Pixels, UTF16Selection, Window};

use super::Workbench;
use serde_json::json;

use crate::editor::{ContentChanged, Position, utf16_column};

impl Workbench {
    /// Enter reaches the editor as typed text, the line break the browser's textarea would insert.
    pub(super) fn editor_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if event.keystroke.key != "enter" || modifiers.platform || modifiers.control || modifiers.alt || self.marked_text.is_some() {
            return;
        }
        cx.stop_propagation();
        self.execute_command("type", vec![json!({ "text": "\n" })], window, cx);
    }

    /// Tells the host about a content change or a cursor move, and reveals the cursor (cursor.ts `revealAll`).
    pub(super) fn editor_edited(&mut self, change: Option<ContentChanged>, moved: bool, source: &str, cx: &mut Context<Self>) {
        let viewport = self.editors.viewport;
        let editor = self.editors.active_mut().expect("only the active editor is edited");
        let scroll = editor.scroll;
        editor.reveal_cursors(source, viewport);
        let scrolled = editor.scroll != scroll;
        self.find_editor_edited(change.as_ref());
        match &change {
            Some(change) => self.host.send_content_change(&self.editors, self.editors.active_index().expect("only the active editor is edited"), change),
            None if moved => self.host.send_selection(self.editors.active().expect("still open"), source),
            None => {}
        }
        // The edit can unfold a region or change the line count, which changes the visible lines.
        if change.is_some() {
            self.host.send_visible_ranges(&self.editors);
        }
        if scrolled {
            self.editor_scrolled(cx);
        }
        cx.notify();
    }

    /// shortcut: save writes the text as it is; no save participants (`$participateInSave`), no format on save,
    /// no conflict check against the file on disk.
    pub(super) fn save_editor(&mut self, index: usize, cx: &mut Context<Self>) {
        let editor = &mut self.editors.editors[index];
        let was_dirty = editor.is_dirty();
        if let Err(error) = editor.save() {
            // shortcut: a save error is logged, not shown as the reference's notification.
            eprintln!("saving {}: {error}", editor.document.path.display());
            return;
        }
        self.host.send_saved(&self.editors, &self.editors.editors[index], was_dirty);
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

/// The platform's text input sees the cursor's line as the whole text.
/// shortcut: the IME sees one line, so reconversion across lines and candidate window placement
/// (`bounds_for_range`) are not supported; the candidate window opens at the system's default spot.
impl EntityInputHandler for Workbench {
    fn text_for_range(&mut self, range: Range<usize>, adjusted_range: &mut Option<Range<usize>>, window: &mut Window, _: &mut Context<Self>) -> Option<String> {
        if let Some(id) = self.focused_input(window) {
            return self.input_text_for_range(id, range, adjusted_range);
        }
        let editor = self.editors.active()?;
        let units: Vec<u16> = editor.document.lines[editor.selection().head.line].encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted_range = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _: bool, window: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        if let Some(id) = self.focused_input(window) {
            return self.input_selected_range(id);
        }
        let editor = self.editors.active()?;
        let selection = editor.selection();
        let line = &editor.document.lines[selection.head.line];
        let head = utf16_column(line, selection.head.column);
        let anchor = if selection.anchor.line == selection.head.line { utf16_column(line, selection.anchor.column) } else { head };
        Some(UTF16Selection { range: anchor.min(head)..anchor.max(head), reversed: head < anchor })
    }

    fn marked_text_range(&self, window: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        if let Some(id) = self.focused_input(window) {
            return self.input_view(id).marked.clone();
        }
        self.marked_text.clone()
    }

    fn unmark_text(&mut self, window: &mut Window, _: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            self.input_view_mut(id).marked = None;
            return;
        }
        self.marked_text = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            return self.input_replace(id, range, text, false, window, cx);
        }
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
    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new_text: &str, _: Option<Range<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.focused_input(window) {
            return self.input_replace(id, range, new_text, true, window, cx);
        }
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
