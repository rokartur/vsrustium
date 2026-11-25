//! The editor part's context menus: the text's (contextmenu.ts) and the tabs' (multiEditorTabsControl.ts).

use gpui::{Bounds, Context, Pixels, Point, Size, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::{Position, column_at_visible};
use crate::menus::MenuId;

impl Workbench {
    /// `ContextMenuController._onContextMenu`: focus the editor, put the cursor at the pointer unless a selection
    /// holds it, then show `MenuId.EditorContext` with the model's uri as argument.
    pub(super) fn show_editor_context_menu(&mut self, line: usize, visible: f32, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor_focus, cx);
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let position = Position::new(line, column_at_visible(&editor.document.lines[line], visible, editor.document.indentation.tab_size));
        if !editor.selections.iter().any(|selection| selection.start() <= position && position <= selection.end()) {
            editor.move_to(position, false);
            self.editor_edited(None, true, "mouse", cx);
        }
        let uri = document_uri(self.editors.active().expect("checked above"));
        let context = self.key_context(window);
        self.show_context_menu(MenuId::EditorContext, &context, point_anchor(pointer), vec![uri], window, cx);
    }

    /// editorLineNumberMenu.ts `doShow`: `MenuId.EditorLineNumberContext` with `editorLineNumber` and the
    /// `{ lineNumber, uri }` argument; a click on a line no selection touches selects that line first.
    pub(super) fn show_line_number_context_menu(&mut self, line: usize, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("the gutter is clickable only with an editor open");
        let line_end = Position::new(line, editor.document.lines[line].chars().count());
        let touched = editor.selections.iter().any(|selection| !selection.is_empty() && selection.start() <= line_end && Position::new(line, 0) <= selection.end());
        if !touched {
            let before = editor.selections.clone();
            editor.set_selections(vec![crate::editor::Selection { anchor: Position::new(line, 0), head: line_end }]);
            self.cursors_changed(before, crate::editor::Reveal::Nothing, "api", cx);
        }
        let editor = self.editors.active().expect("checked above");
        let mut context = self.key_context(window);
        context.insert("editorLineNumber".into(), json!(line + 1));
        let arg = json!({ "lineNumber": line + 1, "uri": document_uri(editor) });
        self.show_context_menu(MenuId::EditorLineNumberContext, &context, point_anchor(pointer), vec![arg], window, cx);
    }

    /// `onTabContextMenu`: `MenuId.EditorTitleContext` with the context keys of the right-clicked editor, which
    /// stays inactive; the commands get its resource and `{ groupId, editorIndex }`. Focus goes back to the group.
    pub(super) fn show_tab_context_menu(&mut self, index: usize, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let editors = &self.editors.editors;
        let mut context = self.key_context(window);
        let editor_keys = [
            ("activeEditorIsNotPreview", json!(!editors[index].preview)),
            ("activeEditorIsLastInGroup", json!(index == editors.len() - 1)),
            ("activeEditorCannotClose", json!(false)),
            ("groupEditorsCount", json!(editors.len())),
            ("multipleEditorsSelectedInGroup", json!(false)),
            ("editorTabsVisible", json!(true)),
            ("config.workbench.editor.enablePreview", self.setting("workbench.editor.enablePreview")),
        ];
        context.extend(editor_keys.map(|(key, value)| (key.to_string(), value)));
        let args = vec![document_uri(&editors[index]), json!({ "groupId": 0, "editorIndex": index })];
        window.focus(&self.editor_focus, cx);
        self.show_context_menu(MenuId::EditorTitleContext, &context, point_anchor(pointer), args, window, cx);
    }

    /// `resolveCommandsContext`: the editor a tab context menu passed, else the active one.
    pub(super) fn context_editor(&self, args: &[Value]) -> Option<usize> {
        match args.iter().find_map(|arg| arg.get("editorIndex")?.as_u64()) {
            Some(index) => (index < self.editors.editors.len() as u64).then_some(index as usize),
            None => self.editors.active_index(),
        }
    }
}

fn document_uri(editor: &crate::editor::OpenEditor) -> Value {
    editor.document.uri()
}

fn point_anchor(pointer: Point<Pixels>) -> Bounds<Pixels> {
    Bounds { origin: pointer, size: Size::default() }
}

#[cfg(test)]
mod tests;
