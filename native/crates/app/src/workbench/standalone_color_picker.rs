//! standaloneColorPickerActions.ts, standaloneColorPickerController.ts, standaloneColorPickerWidget.ts and
//! standaloneColorPickerParticipant.ts: Show or Focus Standalone Color Picker opens the picker at the cursor, on
//! the document color the selection is in (Replace) or on black with the default presentations (Insert).
// shortcut: the widget takes no keyboard focus; "focus" only sets `standaloneColorPickerFocused`, so typed text
// still goes to the editor (and hides the picker, as an edit would); give the widget a focus handle with the
// editor's other overlay inputs.

use futures::channel::oneshot;
use gpui::{AnyElement, Context, deferred, div, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::color_detector::ColorData;
use super::color_picker::HoverColorPicker;
use crate::commands::Outcome;
use crate::editor::Position;

/// The standalone picker: the shared picker state, whether a document color was found, and where it shows.
pub(super) struct StandalonePicker {
    pub picker: HoverColorPicker,
    pub found_in_editor: bool,
    pub position: Position,
    pub focused: bool,
    /// `_selectionSetInEditor`: the next cursor change is the picker's own selection.
    selection_set: bool,
    selections: Vec<(Position, Position)>,
}

impl Workbench {
    pub(super) fn register_standalone_color_picker(&mut self) {
        self.commands.register("editor.action.showOrFocusStandaloneColorPicker", |workbench, _, _, cx| {
            workbench.show_or_focus_standalone_color_picker(cx);
            Outcome::done()
        });
        self.commands.register("editor.action.hideColorPicker", |workbench, _, _, cx| {
            workbench.hide_standalone_color_picker(cx);
            Outcome::done()
        });
        self.commands.register("editor.action.insertColorWithStandaloneColorPicker", |workbench, _, _, cx| {
            workbench.insert_standalone_color(cx);
            Outcome::done()
        });
    }

    /// `standaloneColorPickerVisible` and `standaloneColorPickerFocused`.
    pub(super) fn standalone_color_picker_context(&self, context: &mut std::collections::HashMap<String, serde_json::Value>) {
        let standalone = self.color_detector.standalone.as_ref();
        context.insert("standaloneColorPickerVisible".into(), json!(standalone.is_some()));
        context.insert("standaloneColorPickerFocused".into(), json!(standalone.is_some_and(|standalone| standalone.focused)));
    }

    /// `showOrFocus`: a new widget when none shows, else the focus.
    fn show_or_focus_standalone_color_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(standalone) = self.color_detector.standalone.as_mut() {
            standalone.focused = true;
            cx.notify();
            return;
        }
        let Some(editor) = self.editors.active() else { return };
        let selection = editor.selection();
        let (start, end) = (selection.start(), selection.end());
        let position = selection.head;
        let selections: Vec<_> = editor.selections.iter().map(|selection| (selection.start(), selection.end())).collect();
        // `createColorHover`: the document color containing the selection, else black at the selection with the
        // default provider.
        let found = self.color_detector.colors.iter().rev().find(|color| color.start <= start && end <= color.end).cloned();
        let found_in_editor = found.is_some();
        let data = found.unwrap_or(ColorData { start, end, color: [0., 0., 0., 1.], handle: None });
        let pending: oneshot::Receiver<HoverColorPicker> = self.create_color_hover(editor, &data, cx);
        cx.spawn(async move |workbench, cx| {
            let Ok(mut picker) = pending.await else { return };
            picker.standalone = true;
            let _ = workbench.update(cx, |workbench, cx| {
                let selection_set = found_in_editor;
                if found_in_editor && let Some(editor) = workbench.editors.active_mut() {
                    editor.set_selections(vec![crate::editor::Selection { anchor: data.start, head: data.end }]);
                }
                workbench.color_detector.standalone = Some(StandalonePicker { picker, found_in_editor, position, focused: false, selection_set, selections });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn hide_standalone_color_picker(&mut self, cx: &mut Context<Self>) {
        if self.color_detector.standalone.take().is_some() {
            cx.notify();
        }
    }

    /// `insertColor`: `updateEditor` at every selection the editor had, then hide.
    pub(super) fn insert_standalone_color(&mut self, cx: &mut Context<Self>) {
        let Some(standalone) = self.color_detector.standalone.take() else { return };
        let selections = standalone.selections.clone();
        self.color_detector.standalone = Some(standalone);
        self.apply_picker_presentation(Some(selections), cx);
        self.color_detector.standalone = None;
        cx.notify();
    }

    /// The widget's `onDidChangeCursorPosition`: a move it did not make hides it.
    pub(super) fn standalone_color_picker_cursor_changed(&mut self, cx: &mut Context<Self>) {
        let Some(standalone) = self.color_detector.standalone.as_mut() else { return };
        if standalone.selection_set {
            standalone.selection_set = false;
            if let Some(editor) = self.editors.active() {
                let selections = editor.selections.iter().map(|selection| (selection.start(), selection.end())).collect();
                self.color_detector.standalone.as_mut().expect("checked above").selections = selections;
            }
            return;
        }
        self.hide_standalone_color_picker(cx);
    }

    /// The widget at the cursor, above it unless `editor.hover.above` is off or there is no room.
    pub(super) fn standalone_color_picker_widget(&self, editor: &crate::editor::OpenEditor, content_left: f32, char_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let standalone = self.color_detector.standalone.as_ref().filter(|standalone| standalone.picker.model_id == editor.model_id)?;
        let view_line = editor.position_view_line(standalone.position, false);
        let left = editor.segment_visible_column(editor.segment(view_line), standalone.position.column) * char_width - editor.scroll.left + content_left;
        let anchor_top = editor.line_top(view_line, self.editors.viewport) - editor.scroll.top;
        let height = super::color_picker::STANDALONE_HEIGHT + 2.;
        let editor_bounds = self.text_area_bounds.get();
        let space_above = editor_bounds.origin.y + anchor_top;
        let above = self.hover_settings(editor).above && space_above >= height;
        let top = if above { anchor_top - height } else { anchor_top + self.editors.viewport.line_height };
        let widget = div()
            .id("standalone-color-picker")
            .absolute()
            .left(px(left))
            .top(px(top))
            .border_1()
            .border_color(self.color("editorHoverWidget.border"))
            .bg(self.color("editorHoverWidget.background"))
            .text_color(self.foreground_or("editorHoverWidget.foreground"))
            .cursor_default()
            .occlude()
            .on_any_mouse_down(cx.listener(|workbench, _, _, cx| {
                cx.stop_propagation();
                if let Some(standalone) = workbench.color_detector.standalone.as_mut() {
                    standalone.focused = true;
                }
            }))
            .child(self.color_picker_row(cx));
        Some(deferred(widget).with_priority(1).into_any_element())
    }
}
