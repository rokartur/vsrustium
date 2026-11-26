//! The side by side editor (sideBySideEditor.ts, sideBySideEditorInput.ts): one editor of a group that draws two
//! code editors, the secondary side left of the primary (above it with `workbench.editor.splitInGroupLayout`
//! vertical). The primary side is the group's entry; the secondary side is the one editor of a group outside the grid
//! (`EditorGroup.side_by_sides`), drawn swapped in, so a press in it makes that group the active one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, prelude::FluentBuilder, px,
};
use serde_json::json;

use super::Workbench;
use crate::editor::OpenEditor;

/// editor.ts `DEFAULT_EDITOR_MIN_DIMENSIONS`: each side's `minimumSize` in the split view.
const MINIMUM_WIDTH: f32 = 220.;
const MINIMUM_HEIGHT: f32 = 70.;

/// The split view of each side by side editor (sideBySideEditor.ts `splitview`): the secondary side's share by the
/// entry's model id once its sash moved (`getSplitViewRatio`), else the sides are distributed.
#[derive(Default)]
pub(super) struct SideBySideSplits {
    ratios: HashMap<u32, f32>,
    /// The split's size along its orientation as last laid out, by entry.
    sizes: Rc<RefCell<HashMap<u32, f32>>>,
    drag: Option<SashDrag>,
    hovered: Option<u32>,
    hover: Option<Task<()>>,
}

#[derive(Clone, Copy)]
struct SashDrag {
    entry: u32,
    start: f32,
    start_size: f32,
    total: f32,
}

/// The secondary side's size after its sash moved `delta` from `start_size`: splitview.ts keeps both views at their
/// minimum, so in a split smaller than both minimums the sides stay distributed.
fn dragged_secondary_size(start_size: f32, delta: f32, total: f32, minimum: f32) -> f32 {
    if total < 2. * minimum {
        return (total / 2.).floor();
    }
    (start_size + delta).clamp(minimum, total - minimum)
}

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

    /// The workbench's action `id` acts on the group of a side by side editor (its active editor is the side by side
    /// input), so an active secondary side gives way first; the quick access pickers and Find in Files act on the
    /// focused code editor (`activeTextEditorControl`) and keep it. Returns the side left.
    pub(crate) fn leave_side_group_for(&mut self, id: &str) -> Option<u32> {
        let on_code_editor = matches!(
            id,
            "workbench.action.quickOpen" | "workbench.action.quickOpenWithModes" | "workbench.action.showCommands" | "workbench.action.gotoLine" | "workbench.action.gotoOffset" | "workbench.action.gotoSymbol" | "workbench.action.showAllSymbols" | "workbench.action.findInFiles" | "workbench.action.replaceInFiles"
        );
        if !id.starts_with("workbench.action.") || on_code_editor {
            return None;
        }
        let side = self.editors.id;
        self.editors.side_group_owner(side)?;
        self.leave_side_group();
        Some(side)
    }

    /// The side by side editor still shows `side` after an action: the side it had the focus in keeps it
    /// (`lastFocusedSide`).
    pub(crate) fn return_to_side_group(&mut self, side: u32) {
        if self.editors.side_group() == Some(side) {
            self.activate_group(side);
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

    /// sideBySideEditor.ts `layout`: the split view of the secondary side and the primary one, distributed until the
    /// sash between them moves; the primary side draws the border (`sideBySideEditor.verticalBorder` left to right,
    /// `.horizontalBorder` top to bottom) inside its size.
    pub(super) fn side_by_side_body(&self, secondary: AnyElement, primary: &OpenEditor, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let vertical = self.setting("workbench.editor.splitInGroupLayout") == json!("vertical");
        let entry = self.editors.active_tab().expect("a side by side editor is the active tab").model_id;
        let sizes = self.side_by_side_splits.sizes.clone();
        let measure = canvas(move |bounds, _, _| _ = sizes.borrow_mut().insert(entry, f32::from(if vertical { bounds.size.height } else { bounds.size.width })), |_, _, _, _| ()).absolute().inset_0();
        let border = self.color(if vertical { "sideBySideEditor.horizontalBorder" } else { "sideBySideEditor.verticalBorder" });
        let primary = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .map(|primary| match vertical {
                true => primary.border_t_1(),
                false => primary.border_l_1(),
            })
            .border_color(border)
            .children(self.unicode_banner(primary, cx))
            .child(self.text_area(primary, window, cx).opacity(self.unfocused_editor_opacity(window)));
        let ratio = self.side_by_side_splits.ratios.get(&entry).copied().unwrap_or(0.5);
        let secondary = div().flex_none().min_w_0().min_h_0().flex().flex_col().child(secondary);
        let secondary = match vertical {
            true => secondary.h(gpui::relative(ratio)),
            false => secondary.w(gpui::relative(ratio)),
        };
        let split = div().relative().flex_1().min_h_0().flex().child(measure);
        let split = match vertical {
            true => split.flex_col(),
            false => split.flex_row(),
        };
        split.child(secondary).child(primary).child(self.side_by_side_sash(entry, ratio, vertical, cx)).children(self.side_by_side_sash_drag(entry, vertical, cx))
    }

    /// The `.monaco-sash` between the sides: a drag resizes them, a double click distributes them again
    /// (`onDidSashReset`).
    fn side_by_side_sash(&self, entry: u32, ratio: f32, vertical: bool, cx: &mut Context<Self>) -> AnyElement {
        let size = self.sash_size();
        let hover = self.sash_hover_size();
        let splits = &self.side_by_side_splits;
        let active = splits.hovered == Some(entry) || splits.drag.is_some_and(|drag| drag.entry == entry);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let minimum = if vertical { MINIMUM_HEIGHT } else { MINIMUM_WIDTH };
        let laid_out = splits.sizes.borrow().get(&entry).copied().unwrap_or_default();
        let secondary_size = laid_out * ratio;
        // splitview.ts `updateSashEnablement` with mac cursors (sash.css): a side at its minimum leaves one direction.
        let cursor = match (vertical, laid_out < 2. * minimum, secondary_size <= minimum + 0.5, secondary_size >= laid_out - minimum - 0.5) {
            (_, true, _, _) => CursorStyle::Arrow,
            (false, _, true, _) => CursorStyle::ResizeRight,
            (false, _, _, true) => CursorStyle::ResizeLeft,
            (false, ..) => CursorStyle::ResizeColumn,
            (true, _, true, _) => CursorStyle::ResizeDown,
            (true, _, _, true) => CursorStyle::ResizeUp,
            (true, ..) => CursorStyle::ResizeRow,
        };
        let sash = div().id(("side-by-side-sash", entry as usize)).absolute().cursor(cursor);
        let sash = match vertical {
            true => sash.left_0().right_0().top(gpui::relative(ratio)).mt(px(-size / 2.)).h(px(size)),
            false => sash.top_0().bottom_0().left(gpui::relative(ratio)).ml(px(-size / 2.)).w(px(size)),
        };
        let highlight = div().absolute().bg(hover_border);
        let highlight = match vertical {
            true => highlight.left_0().right_0().top(px((size - hover) / 2.)).h(px(hover)),
            false => highlight.top_0().bottom_0().left(px((size - hover) / 2.)).w(px(hover)),
        };
        sash.when(active, |sash| sash.child(highlight))
            .on_hover(cx.listener(move |workbench, over: &bool, window, cx| {
                let splits = &mut workbench.side_by_side_splits;
                if !*over {
                    if splits.hovered == Some(entry) {
                        splits.hovered = None;
                    }
                    splits.hover = None;
                    return cx.notify();
                }
                let delay = workbench.sash_hover_delay();
                workbench.side_by_side_splits.hover = Some(cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.side_by_side_splits.hovered = Some(entry);
                        cx.notify();
                    });
                }));
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let total = workbench.side_by_side_splits.sizes.borrow().get(&entry).copied().unwrap_or_default();
                let start = axis(event.position, vertical);
                workbench.side_by_side_splits.drag = Some(SashDrag { entry, start, start_size: total * ratio, total });
                cx.notify();
            }))
            .on_click(cx.listener(move |workbench, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    workbench.side_by_side_splits.ratios.remove(&entry);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while the sash of `entry` drags.
    fn side_by_side_sash_drag(&self, entry: u32, vertical: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let drag = self.side_by_side_splits.drag.filter(|drag| drag.entry == entry)?;
        let minimum = if vertical { MINIMUM_HEIGHT } else { MINIMUM_WIDTH };
        let workbench = cx.entity();
        let element = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let size = dragged_secondary_size(drag.start_size, axis(event.position, vertical) - drag.start, drag.total, minimum);
                        workbench.side_by_side_splits.ratios.insert(entry, size / drag.total);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.side_by_side_splits.drag = None;
                        cx.notify();
                    });
                });
            },
        )
        .size_0();
        Some(element.into_any_element())
    }
}

fn axis(position: Point<Pixels>, vertical: bool) -> f32 {
    f32::from(if vertical { position.y } else { position.x })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sash_drag_keeps_both_sides_at_their_minimum() {
        assert_eq!(dragged_secondary_size(400., 100., 800., MINIMUM_WIDTH), 500.);
        assert_eq!(dragged_secondary_size(400., -300., 800., MINIMUM_WIDTH), 220.);
        assert_eq!(dragged_secondary_size(400., 300., 800., MINIMUM_WIDTH), 580.);
        assert_eq!(dragged_secondary_size(200., 50., 401., MINIMUM_WIDTH), 200.);
    }
}
