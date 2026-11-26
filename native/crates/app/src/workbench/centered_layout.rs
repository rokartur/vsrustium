//! The main editor's centered layout: browser/layout.ts `centerMainEditorLayout` (`MAIN_EDITOR_CENTERED`) and
//! base/browser/ui/centered/centeredViewLayout.ts drawn in the editor part's card: two margins beside the groups with
//! the group separator between them, resized by their sashes and stored in the profile's editor part memento
//! (`editorpart.centeredview`).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, StatefulInteractiveElement, Styled, Task, canvas, div,
    prelude::FluentBuilder, px,
};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;

/// `LayoutStateKeys.MAIN_EDITOR_CENTERED` in workspace storage.
const CENTERED_KEY: &str = "workbench.editor.centered";
/// EditorPart's profile memento and its `EDITOR_PART_CENTERED_VIEW_STORAGE_KEY`.
const MEMENTO_KEY: &str = "memento/workbench.parts.editor";
const STATE_KEY: &str = "editorpart.centeredview";
/// centeredViewLayout.ts `defaultState` and the margins' `minimumSize`.
const DEFAULT_TARGET_WIDTH: f32 = 900.;
const DEFAULT_MARGIN_RATIO: f32 = 0.1909;
const MARGIN_MINIMUM: f32 = 60.;
/// editor.ts `DEFAULT_EDITOR_MIN_DIMENSIONS`.
const EDITOR_MINIMUM_WIDTH: f32 = 220.;

/// centeredViewLayout.ts `CenteredViewState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CenteredViewState {
    pub(super) target_width: f32,
    pub(super) left_margin_ratio: f32,
    pub(super) right_margin_ratio: f32,
}

impl Default for CenteredViewState {
    fn default() -> Self {
        CenteredViewState { target_width: DEFAULT_TARGET_WIDTH, left_margin_ratio: DEFAULT_MARGIN_RATIO, right_margin_ratio: DEFAULT_MARGIN_RATIO }
    }
}

impl CenteredViewState {
    /// `resizeSplitViews`: the margins for a part `width` wide, a fixed center width or the margins' ratios, each at
    /// least its minimum while the editor keeps its own.
    pub(super) fn margins(&self, width: f32, fixed_width: bool) -> (f32, f32) {
        let (left, right) = if fixed_width {
            let center = width.min(self.target_width);
            let margin = (width - center) / 2.;
            (margin.floor(), margin.ceil())
        } else {
            (self.left_margin_ratio * width, self.right_margin_ratio * width)
        };
        let room = (width - EDITOR_MINIMUM_WIDTH).max(2. * MARGIN_MINIMUM);
        let left = left.max(MARGIN_MINIMUM).min(room - MARGIN_MINIMUM);
        let right = right.max(MARGIN_MINIMUM).min(room - left);
        (left, right)
    }
}

#[derive(Default)]
pub(super) struct CenteredLayout {
    /// The card's width the last frame measured.
    pub(super) card_width: Rc<Cell<f32>>,
    state: Option<CenteredViewState>,
    /// The margin a sash drags (`true` for the left one) with the pointer's x and the margins when it started.
    drag: Option<(bool, f32, (f32, f32))>,
    hovered: Option<bool>,
    hover: Option<(bool, Task<()>)>,
}

impl Workbench {
    pub(super) fn register_centered_layout_actions(&mut self) {
        // layoutActions.ts `ToggleCenteredLayout`.
        self.commands.register("workbench.action.toggleCenteredLayout", |workbench, _, window, cx| {
            let centered = workbench.is_main_editor_centered();
            workbench.center_main_editor_layout(!centered, cx);
            window.focus(&workbench.editor_focus.clone(), cx);
            Outcome::done()
        });
    }

    /// `isMainEditorLayoutCentered`.
    pub(super) fn is_main_editor_centered(&self) -> bool {
        self.workspace_storage.lock().unwrap().get(CENTERED_KEY).is_some_and(|value| value == "true")
    }

    /// `centerMainEditorLayout`.
    pub(super) fn center_main_editor_layout(&mut self, active: bool, cx: &mut Context<Self>) {
        self.workspace_storage.lock().unwrap().set(CENTERED_KEY, active.to_string());
        cx.notify();
    }

    /// `centerMainEditorLayout`'s check: with `workbench.editor.centeredLayoutAutoResize` the layout centers only one
    /// column of groups (or a maximized group) and no side by side diff.
    pub(super) fn is_editor_layout_centered(&self) -> bool {
        if !self.is_main_editor_centered() {
            return false;
        }
        if self.setting("workbench.editor.centeredLayoutAutoResize") == json!(false) {
            return true;
        }
        let side_by_side = self.setting("diffEditor.renderSideBySide") != json!(false);
        let complex = self.editors.active_tab().is_some_and(|tab| (side_by_side && self.editors.diffs.contains_key(&tab.model_id)) || self.editors.multi_diffs.contains_key(&tab.model_id));
        !complex && !self.editor_groups_have_several_columns()
    }

    fn centered_view_state(&self) -> CenteredViewState {
        if let Some(state) = self.centered_layout.state {
            return state;
        }
        let memento: Option<Value> = self.profile_storage.lock().unwrap().get(MEMENTO_KEY).and_then(|value| serde_json::from_str(value).ok());
        let stored = memento.as_ref().map(|memento| &memento[STATE_KEY]);
        let number = |key: &str| stored.and_then(|state| state[key].as_f64()).map(|value| value as f32);
        match (number("targetWidth"), number("leftMarginRatio"), number("rightMarginRatio")) {
            (Some(target_width), Some(left_margin_ratio), Some(right_margin_ratio)) => CenteredViewState { target_width, left_margin_ratio, right_margin_ratio },
            _ => CenteredViewState::default(),
        }
    }

    /// EditorPart `saveState`: a default state leaves the memento (`isDefault`).
    fn store_centered_view_state(&mut self, state: CenteredViewState) {
        self.centered_layout.state = Some(state);
        let mut storage = self.profile_storage.lock().unwrap();
        let mut memento: Value = storage.get(MEMENTO_KEY).and_then(|value| serde_json::from_str(value).ok()).unwrap_or_else(|| json!({}));
        let fixed = self.setting("workbench.editor.centeredLayoutFixedWidth") == json!(true);
        let default = CenteredViewState::default();
        let is_default = if fixed { state.target_width == default.target_width } else { state.left_margin_ratio == default.left_margin_ratio && state.right_margin_ratio == default.right_margin_ratio };
        match is_default {
            true => _ = memento.as_object_mut().map(|memento| memento.remove(STATE_KEY)),
            false => memento[STATE_KEY] = json!({ "targetWidth": state.target_width, "leftMarginRatio": state.left_margin_ratio, "rightMarginRatio": state.right_margin_ratio }),
        }
        storage.set(MEMENTO_KEY, memento.to_string());
    }

    /// The margins of the centered layout in the card this frame, if it centers.
    pub(super) fn centered_margins(&self) -> Option<(f32, f32)> {
        if !self.is_editor_layout_centered() {
            return None;
        }
        let width = self.centered_layout.card_width.get();
        let fixed = self.setting("workbench.editor.centeredLayoutFixedWidth") == json!(true);
        Some(self.centered_view_state().margins(width, fixed))
    }

    /// The card's measure, the separators (`editorGroup.border`, splitview.ts `separatorBorder`) and the two sashes.
    pub(super) fn centered_layout_chrome(&self, margins: Option<(f32, f32)>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let card_width = self.centered_layout.card_width.clone();
        let measure = canvas(move |bounds, _, _| card_width.set(f32::from(bounds.size.width)), |_, _, _, _| ()).absolute().inset_0().into_any_element();
        let Some((left, right)) = margins else { return vec![measure] };
        let width = self.centered_layout.card_width.get();
        let separator = self.theme.color("editorGroup.border").or(self.theme.color("contrastBorder")).unwrap_or(gpui::transparent_black().into());
        let line = |x: f32| div().absolute().top_0().bottom_0().left(px(x)).w(px(1.)).bg(separator).into_any_element();
        vec![measure, line(left), line(width - right - 1.), self.centered_sash(true, left, cx), self.centered_sash(false, width - right, cx)]
    }

    fn centered_sash(&self, left_margin: bool, x: f32, cx: &mut Context<Self>) -> AnyElement {
        let size = self.sash_size();
        let active = self.centered_layout.hovered == Some(left_margin) || self.centered_layout.drag.is_some_and(|(dragged, ..)| dragged == left_margin);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        let hover = self.sash_hover_size();
        div()
            .id(if left_margin { "centered-sash-left" } else { "centered-sash-right" })
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(x - size / 2.))
            .w(px(size))
            .cursor(CursorStyle::ResizeColumn)
            .when(active, |sash| sash.child(div().absolute().top_0().bottom_0().left(px((size - hover) / 2.)).w(px(hover)).bg(hover_border)))
            .on_hover(cx.listener(move |workbench, over: &bool, window, cx| {
                let layout = &mut workbench.centered_layout;
                if !*over {
                    if layout.hovered == Some(left_margin) {
                        layout.hovered = None;
                    }
                    layout.hover = None;
                    cx.notify();
                    return;
                }
                let delay = workbench.sash_hover_delay();
                workbench.centered_layout.hover = Some((left_margin, cx.spawn_in(window, async move |workbench, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = workbench.update(cx, |workbench, cx| {
                        workbench.centered_layout.hovered = Some(left_margin);
                        cx.notify();
                    });
                })));
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some(margins) = workbench.centered_margins() {
                    workbench.centered_layout.drag = Some((left_margin, f32::from(event.position.x), margins));
                }
                cx.notify();
            }))
            // splitview.ts `onDidSashReset`: a double click restores the default state.
            .on_click(cx.listener(|workbench, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    workbench.store_centered_view_state(CenteredViewState::default());
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a centered layout sash drags: the margin and the editor share
    /// the change (the other margin stays), each kept at its minimum.
    pub(super) fn centered_sash_drag(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (left_margin, start, (left, right)) = self.centered_layout.drag?;
        let workbench = cx.entity();
        let width = self.centered_layout.card_width.get();
        let element = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let delta = f32::from(event.position.x) - start;
                        let room = width - EDITOR_MINIMUM_WIDTH;
                        let margins = match left_margin {
                            true => ((left + delta).clamp(MARGIN_MINIMUM, (room - right).max(MARGIN_MINIMUM)), right),
                            false => (left, (right - delta).clamp(MARGIN_MINIMUM, (room - left).max(MARGIN_MINIMUM))),
                        };
                        // centeredViewLayout.ts `updateState` on every sash change.
                        let state = CenteredViewState { target_width: width - margins.0 - margins.1, left_margin_ratio: margins.0 / width, right_margin_ratio: margins.1 / width };
                        workbench.centered_layout.state = Some(state);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.centered_layout.drag = None;
                        if let Some(state) = workbench.centered_layout.state {
                            workbench.store_centered_view_state(state);
                        }
                        cx.notify();
                    });
                });
            },
        )
        .size_0();
        Some(element.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::CenteredViewState;

    #[test]
    fn the_default_margins_are_a_fifth_of_the_width_each() {
        let (left, right) = CenteredViewState::default().margins(1000., false);
        assert!((left - 190.9).abs() < 0.01 && (right - 190.9).abs() < 0.01);
    }

    #[test]
    fn a_fixed_width_centers_the_target_width() {
        assert_eq!(CenteredViewState::default().margins(1201., true), (150., 151.));
        assert_eq!(CenteredViewState::default().margins(1001., true), (60., 60.));
    }

    #[test]
    fn narrow_parts_keep_the_margins_minimum_and_the_editor_its_own() {
        assert_eq!(CenteredViewState::default().margins(300., false), (60., 60.));
    }
}
