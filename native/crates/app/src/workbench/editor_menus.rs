//! The editor part's context menus: the text's and the scrollbar's (contextmenu.ts) and the tabs'
//! (multiEditorTabsControl.ts).

use gpui::{Bounds, Context, Pixels, Point, Size, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::editor::minimap::{MinimapSide, MinimapSize};
use crate::editor::{Position, column_at_visible};
use crate::menus::{MenuAction, MenuId};

impl Workbench {
    /// `ContextMenuController._onContextMenu`: focus the editor, put the cursor at the pointer unless a selection
    /// holds it, then show `MenuId.EditorContext` with the model's uri as argument.
    pub(super) fn show_editor_context_menu(&mut self, line: usize, visible: f32, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor_focus, cx);
        let editor = self.editors.active_mut().expect("text is clickable only with an editor open");
        let position = Position::new(line, column_at_visible(&editor.document().lines[line], visible, editor.document().indentation.tab_size));
        if !editor.selections.iter().any(|selection| selection.start() <= position && position <= selection.end()) {
            editor.move_to(position, false);
            self.editor_edited(None, true, "mouse", cx);
        }
        let uri = document_uri(self.editors.active().expect("checked above"));
        let context = self.key_context(window);
        self.show_context_menu(MenuId::EditorContext, &context, point_anchor(pointer), vec![uri], window, cx);
    }

    /// `ShowContextMenu` (Shift+F10): `MenuId.EditorContext` below the cursor, which stays where it is; not with
    /// `editor.contextmenu` off.
    pub(super) fn show_editor_context_menu_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.setting("editor.contextmenu") == json!(false) {
            return;
        }
        let Some(anchor) = self.cursor_anchor(window) else { return };
        let uri = document_uri(self.editors.active().expect("an anchor needs an editor"));
        let context = self.key_context(window);
        self.show_context_menu(MenuId::EditorContext, &context, point_anchor(anchor), vec![uri], window, cx);
    }

    /// editorLineNumberMenu.ts `doShow`: `MenuId.EditorLineNumberContext` with `editorLineNumber` and the
    /// `{ lineNumber, uri }` argument; a click on a line no selection touches selects that line first.
    pub(super) fn show_line_number_context_menu(&mut self, line: usize, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editors.active_mut().expect("the gutter is clickable only with an editor open");
        let line_end = Position::new(line, editor.document().lines[line].chars().count());
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
            ("activeEditorIsPinned", json!(editors[index].sticky)),
            ("activeEditorIsDirty", json!(self.tab_shows_dirty(index))),
            ("splitEditorsVertically", json!(self.setting("workbench.editor.openSideBySideDirection") == "down")),
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

    /// `_showScrollbarContextMenu`: on the scrollbar or the minimap, the `editor.minimap.*` toggles and choices, each
    /// writing its setting; all but Minimap are disabled while the minimap is off. Focus comes back to the editor.
    // shortcut: the context menu widget draws no submenus, so Vertical size, Slider and Side list their choices as
    // "Label: Choice" groups between separators; upgrade when the widget opens `MenuAction::Submenu`.
    pub(super) fn show_scrollbar_context_menu(&mut self, pointer: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.minimap_settings();
        // diffEditorEditors.ts `_adjustOptionsForSubEditor` turns the minimap off in both sides.
        let enabled = settings.enabled && self.editors.active_diff().is_none();
        let render_characters = settings.layout.render_characters;
        let size = match settings.layout.size {
            MinimapSize::Proportional => "proportional",
            MinimapSize::Fill => "fill",
            MinimapSize::Fit => "fit",
        };
        let slider = if settings.slider_always { "always" } else { "mouseover" };
        let side = if settings.layout.side == MinimapSide::Left { "left" } else { "right" };
        let mut actions = vec![
            self.setting_action("Minimap", "editor.minimap.enabled", json!(!enabled), true, enabled),
            MenuAction::Separator,
            self.setting_action("Render Characters", "editor.minimap.renderCharacters", json!(!render_characters), enabled, render_characters),
        ];
        let choices: [(&str, &'static str, &str, &[(&str, &str)]); 3] = [
            ("Vertical size", "editor.minimap.size", size, &[("Proportional", "proportional"), ("Fill", "fill"), ("Fit", "fit")]),
            ("Slider", "editor.minimap.showSlider", slider, &[("Mouse Over", "mouseover"), ("Always", "always")]),
            ("Side", "editor.minimap.side", side, &[("Right", "right"), ("Left", "left")]),
        ];
        for (label, key, current, options) in choices {
            if !enabled {
                actions.push(MenuAction::Command { id: String::new(), title: label.into(), enabled: false, checked: false, alt: None });
                continue;
            }
            actions.push(MenuAction::Separator);
            for (title, value) in options {
                actions.push(self.setting_action(&format!("{label}: {title}"), key, json!(value), true, current == *value));
            }
        }
        window.focus(&self.editor_focus, cx);
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, point_anchor(pointer), Vec::new(), window, cx);
    }

    /// A menu item writing `value` to `key` (`createAction` with `updateValue`), registered under an id of its own.
    fn setting_action(&mut self, title: &str, key: &'static str, value: Value, enabled: bool, checked: bool) -> MenuAction {
        let id = format!("_editor.scrollbarContextMenu.{key}={value}");
        if self.commands.get(&id).is_none() {
            self.commands.register(id.clone(), move |workbench, _, _, cx| {
                workbench.update_setting(key, value.clone());
                cx.notify();
                Outcome::done()
            });
        }
        MenuAction::Command { id, title: title.into(), enabled, checked, alt: None }
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
    editor.document().uri()
}

fn point_anchor(pointer: Point<Pixels>) -> Bounds<Pixels> {
    Bounds { origin: pointer, size: Size::default() }
}

#[cfg(test)]
mod tests;
