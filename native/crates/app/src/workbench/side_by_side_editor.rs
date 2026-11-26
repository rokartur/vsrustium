//! The side by side editor (sideBySideEditor.ts, sideBySideEditorInput.ts): one editor of a group that draws two
//! code editors, the secondary side left of the primary (above it with `workbench.editor.splitInGroupLayout`
//! vertical). The primary side is the group's entry; the secondary side is the one editor of a group outside the grid
//! (`EditorGroup.side_by_sides`), drawn swapped in, so a press in it makes that group the active one.

use std::path::Path;

use gpui::{AnyElement, Context, Div, IntoElement, ParentElement, Styled, Window, div, px};
use serde_json::json;

use super::Workbench;
use crate::editor::OpenEditor;

impl Workbench {
    /// Makes the active editor a side by side editor with the `vscode://` resource `secondary` (a
    /// `vscode://defaultsettings` one) as its secondary side; the primary keeps the focus.
    pub(super) fn show_side_by_side(&mut self, secondary: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.editors.active_tab().expect("the primary side is open").model_id;
        if self.editors.side_by_sides.contains_key(&entry) {
            return;
        }
        let side = self.editors.add_side_group(entry);
        drop(self.with_group(side, |workbench| workbench.open_preferences_resource(secondary, window, cx)));
        self.editors_changed(cx);
    }

    /// editorCommands.ts `toggleSplitEditorInGroupLayout`: `workbench.editor.splitInGroupLayout` flips between
    /// horizontal and vertical in the user settings.
    pub(super) fn register_side_by_side_commands(&mut self) {
        self.commands.register("workbench.action.toggleSplitEditorInGroupLayout", |workbench, _, _, cx| {
            let vertical = workbench.setting("workbench.editor.splitInGroupLayout") == json!("vertical");
            let layout = if vertical { "horizontal" } else { "vertical" };
            workbench.update_user_setting("workbench.editor.splitInGroupLayout", json!(layout));
            cx.notify();
            crate::commands::Outcome::done()
        });
    }

    /// An active secondary side gives way to the group that shows it.
    pub(crate) fn leave_side_group(&mut self) {
        if let Some(owner) = self.editors.side_group_owner(self.editors.id) {
            self.activate_group(owner);
        }
    }

    /// The active side by side editor's secondary side, drawn from its group.
    pub(super) fn side_by_side_secondary(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let side = self.editors.side_group()?;
        let body = self.swapped_group_body(
            side,
            |workbench, window, cx| {
                let editor = workbench.editors.active().expect("a secondary group shows its editor");
                let area = workbench.text_area(editor, window, cx).opacity(workbench.unfocused_editor_opacity(window));
                div().relative().size_full().flex().flex_col().children(workbench.unicode_banner(editor, cx)).child(area)
            },
            window,
            cx,
        );
        Some(body.into_any_element())
    }

    /// sideBySideEditor.ts `layout`: the two sides share the area evenly with the border between them
    /// (`sideBySideEditor.verticalBorder` left to right, `.horizontalBorder` top to bottom).
    pub(super) fn side_by_side_body(&self, secondary: AnyElement, primary: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let vertical = self.setting("workbench.editor.splitInGroupLayout") == json!("vertical");
        let border = self.color(if vertical { "sideBySideEditor.horizontalBorder" } else { "sideBySideEditor.verticalBorder" });
        let primary = div().relative().flex_1().min_w_0().min_h_0().flex().flex_col().children(self.unicode_banner(primary, cx)).child(self.text_area(primary, window, cx).opacity(self.unfocused_editor_opacity(window)));
        let secondary = div().flex_1().min_w_0().min_h_0().flex().flex_col().child(secondary);
        let split = div().flex_1().min_h_0().flex();
        match vertical {
            true => split.flex_col().child(secondary).child(div().h(px(1.)).flex_none().bg(border)).child(primary),
            false => split.flex_row().child(secondary).child(div().w(px(1.)).flex_none().bg(border)).child(primary),
        }
    }
}
