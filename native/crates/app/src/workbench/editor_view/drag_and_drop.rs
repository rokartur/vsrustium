//! dnd.ts and mouseHandler.ts `isDragAndDrop`: a press on the primary selection drags its text, the drop target
//! shows as a dotted bar (`.dnd-target`), and Alt copies instead of moving.

use gpui::{Context, CursorStyle, Div, Modifiers, Rgba, div, prelude::*, px, rgb};

use super::Workbench;
use crate::editor::{Position, Selection, column_at_visible, drag_and_drop, visible_column};
use crate::theme::ThemeKind;

#[derive(Default)]
pub struct TextDrag {
    /// `_dragSelection`: taken at the first move over a selection.
    dragged: Option<Selection>,
    /// The `dnd-target` decoration, at a position outside the dragged selection.
    pub target: Option<Position>,
    /// `mouseStyle: 'copy'`: the trigger modifier (Alt on macOS) is held.
    copy: bool,
}

impl Workbench {
    /// mouseHandler.ts `start`: a single press on the text inside the non-empty primary selection, without Alt,
    /// starts a drag instead of a click.
    pub(in crate::workbench) fn start_text_drag(&mut self, line: usize, visible: f32, modifiers: Modifiers, click_count: usize) -> bool {
        let editor = self.editors.active().expect("text is clickable only with an editor open");
        let text = &editor.document().lines[line];
        let tab_size = editor.document().indentation.tab_size;
        let on_text = visible < visible_column(text, text.chars().count(), tab_size) as f32;
        let position = Position::new(line, column_at_visible(text, visible, tab_size));
        let primary = editor.selection();
        let enabled = self.setting("editor.dragAndDrop") == true && self.setting("editor.columnSelection") != true && self.active_editor_readonly().is_none();
        if !(enabled && !modifiers.alt && click_count < 2 && on_text && !primary.is_empty() && drag_and_drop::contains(primary, position)) {
            return false;
        }
        self.text_drag = Some(TextDrag::default());
        true
    }

    /// dnd.ts `_onEditorMouseDrag`.
    pub(in crate::workbench) fn drag_text(&mut self, line: usize, visible: f32, alt: bool, cx: &mut Context<Self>) {
        let editor = self.editors.active().expect("text is draggable only with an editor open");
        let position = self.position_at(line, visible);
        let drag = self.text_drag.as_mut().expect("a drag is under way");
        let Some(dragged) = drag.dragged.or_else(|| editor.dragged_selection_at(position)) else { return };
        drag.dragged = Some(dragged);
        drag.copy = alt;
        drag.target = (!drag_and_drop::contains(dragged, position)).then_some(position);
        cx.notify();
    }

    /// dnd.ts `_onEditorMouseDrop`: a release without a drag places the cursor; a drag moves or copies the text,
    /// with the trigger modifier also right beside the selection.
    pub(in crate::workbench) fn drop_text(&mut self, line: usize, visible: f32, modifiers: Modifiers, cx: &mut Context<Self>) {
        let position = self.position_at(line, visible);
        let drag = self.text_drag.take().expect("a drag is under way");
        let editor = self.editors.active_mut().expect("text is draggable only with an editor open");
        let before = editor.selections.clone();
        let change = match drag.dragged {
            None => {
                editor.release_press_on_selection(position, modifiers.shift);
                None
            }
            Some(dragged) => {
                let beside = position == dragged.start() || position == dragged.end();
                if drag_and_drop::contains(dragged, position) && !(modifiers.alt && beside) {
                    return cx.notify();
                }
                editor.drop_dragged_text(dragged, position, modifiers.alt)
            }
        };
        let moved = editor.selections != before;
        self.editor_edited(change, moved, "mouse", cx);
    }

    /// `_onEditorMouseDropCanceled`: a key press or the release away from the text ends the drag without a drop.
    pub(in crate::workbench) fn cancel_text_drag(&mut self, cx: &mut Context<Self>) {
        if self.text_drag.take().is_some() {
            cx.notify();
        }
    }

    /// dnd.ts `onEditorKeyDown`/`onEditorKeyUp`: the trigger modifier switches the drag to a copy.
    pub(in crate::workbench) fn text_drag_modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.text_drag {
            drag.copy = modifiers.alt;
            cx.notify();
        }
    }

    /// dnd.css `.mouse-default`, `.mouse-copy`: the cursor over the lines while text is dragged.
    pub(in crate::workbench) fn text_cursor_style(&self) -> CursorStyle {
        match &self.text_drag {
            Some(TextDrag { dragged: Some(_), copy: true, .. }) => CursorStyle::DragCopy,
            Some(TextDrag { dragged: Some(_), .. }) => CursorStyle::Arrow,
            _ => CursorStyle::IBeam,
        }
    }

    /// dnd.css `.dnd-target`: an empty span's `border-right: 2px dotted`, drawn as 2px dots every 4px.
    pub(in crate::workbench) fn drop_target_marker(&self, x: f32) -> Div {
        let color: Rgba = match self.theme.kind {
            ThemeKind::Dark => rgb(0xAEAFAD),
            ThemeKind::HighContrastDark => rgb(0xFFFFFF),
            ThemeKind::Light | ThemeKind::HighContrastLight => rgb(0x000000),
        };
        let dots = (0..(self.editors.viewport.line_height / 4.).ceil() as usize).map(|dot| div().absolute().top(px(dot as f32 * 4.)).size(px(2.)).rounded_full().bg(color));
        div().absolute().top_0().left(px(x)).w(px(2.)).h_full().children(dots)
    }

    fn position_at(&self, line: usize, visible: f32) -> Position {
        let editor = self.editors.active().expect("text is draggable only with an editor open");
        Position::new(line, column_at_visible(&editor.document().lines[line], visible, editor.document().indentation.tab_size))
    }
}
