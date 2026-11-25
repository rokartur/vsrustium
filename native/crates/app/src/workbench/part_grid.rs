//! The workbench's part grid (browser/layout.ts): part sizes and visibility in `LayoutStateModel`'s storage keys,
//! the sashes between the parts, and the layout commands of layoutActions.ts.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, Context, CursorStyle, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Size, StatefulInteractiveElement, Styled, Task, Window, canvas, div,
    prelude::FluentBuilder, px,
};
use serde_json::{Value, json};

use super::Workbench;
use super::layout::{CARD_MARGIN, Grid, Part, PartSizes, Sash};
use crate::commands::Outcome;
use crate::split_view::SashState;

/// The title bar and status bar heights around the grid's middle row.
const TITLE_BAR_HEIGHT: f32 = 35.;
const STATUS_BAR_HEIGHT: f32 = 28.;
/// `workbench.sash.hoverDelay`'s default.
const SASH_HOVER_DELAY: Duration = Duration::from_millis(300);
/// `LayoutStateModel.STORAGE_PREFIX`.
const STORAGE_PREFIX: &str = "workbench.";

#[derive(Default)]
pub(super) struct PartGrid {
    /// Laid out on the first render, when the window size the default part sizes come from is known.
    grid: Option<Grid>,
    hovered: Option<Sash>,
    hover: Option<(Sash, Task<()>)>,
}

/// `LayoutStateKeys`' visibility key of a part and its default; the side bar defaults to hidden in an empty window.
fn hidden_key(part: Part) -> &'static str {
    match part {
        Part::ActivityBar => "activityBar.hidden",
        Part::SideBar => "sideBar.hidden",
        Part::Panel => "panel.hidden",
        Part::AuxiliaryBar => "auxiliaryBar.hidden",
    }
}

impl Workbench {
    pub(super) fn register_layout_actions(&mut self) {
        self.commands.register("workbench.action.toggleSidebarVisibility", |workbench, _, window, cx| {
            match workbench.is_part_visible(Part::SideBar) {
                true => workbench.hide_side_bar(window, cx),
                false => workbench.set_part_hidden(Part::SideBar, false, cx),
            }
            Outcome::done()
        });
        self.commands.register("workbench.action.closeSidebar", |workbench, _, window, cx| {
            workbench.hide_side_bar(window, cx);
            Outcome::done()
        });
        for (command, location) in [("workbench.action.activityBarLocation.default", "default"), ("workbench.action.activityBarLocation.hide", "hidden")] {
            self.commands.register(command, move |workbench, _, _, cx| {
                workbench.update_setting("workbench.activityBar.location", json!(location));
                cx.notify();
                Outcome::done()
            });
        }
    }

    /// `isVisible(part)`. The activity bar follows `workbench.activityBar.location` (`isActivityBarHidden`); the port
    /// draws only the default location, so `top` and `bottom` hide it as upstream does from the grid.
    pub(super) fn is_part_visible(&self, part: Part) -> bool {
        if part == Part::ActivityBar {
            return self.setting("workbench.activityBar.location").as_str().is_none_or(|location| location == "default");
        }
        match &self.part_grid.grid {
            Some(grid) => grid.is_visible(part),
            None => !self.stored_part_hidden(part),
        }
    }

    pub(super) fn layout_context(&self, context: &mut HashMap<String, Value>) {
        context.insert("sideBarVisible".into(), json!(self.is_part_visible(Part::SideBar)));
    }

    /// `setPartHidden`.
    pub(super) fn set_part_hidden(&mut self, part: Part, hidden: bool, cx: &mut Context<Self>) {
        if let Some(grid) = self.part_grid.grid.as_mut() {
            grid.set_visible(part, !hidden);
        }
        self.workspace_storage.lock().unwrap().set(&format!("{STORAGE_PREFIX}{}", hidden_key(part)), hidden.to_string());
        cx.notify();
    }

    /// `setSideBarHidden(true)`: the hidden view's focus moves to the editor (`focusPanelOrEditor`).
    fn hide_side_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::SideBar, true, cx);
        window.focus(&self.editor_focus.clone(), cx);
    }

    /// Forgets the laid out grid so the next render reads the folder's stored layout.
    pub(super) fn reset_part_grid(&mut self) {
        self.part_grid.grid = None;
    }

    fn stored_part_hidden(&self, part: Part) -> bool {
        let stored = self.workspace_storage.lock().unwrap().get(&format!("{STORAGE_PREFIX}{}", hidden_key(part))).map(|value| value == "true");
        stored.unwrap_or(match part {
            Part::SideBar => self.folder.is_none(),
            Part::ActivityBar => false,
            Part::Panel | Part::AuxiliaryBar => true,
        })
    }

    /// The `InitializationStateKey` sizes in profile storage, upstream's `parseInt` of the stored string.
    fn stored_part_sizes(&self, window_width: f32, window_height: f32) -> PartSizes {
        let storage = self.profile_storage.lock().unwrap();
        let stored = |key: &str, default: f32| storage.get(&format!("{STORAGE_PREFIX}{key}")).and_then(|value| value.parse::<f32>().ok()).unwrap_or(default);
        let defaults = PartSizes::defaults(window_width, window_height);
        PartSizes { side_bar: stored("sideBar.size", defaults.side_bar), panel: stored("panel.size", defaults.panel), auxiliary_bar: stored("auxiliaryBar.size", defaults.auxiliary_bar) }
    }

    /// `saveState`'s part sizes, a hidden part's the size it comes back at, and the visibility a sash snapped
    /// (`onDidVisibilityChange`). Upstream saves the sizes on shutdown only; the port saves them after each sash drag,
    /// as its storage writes synchronously and the app can exit without a shutdown phase.
    fn store_part_layout(&self) {
        let grid = self.part_grid.grid.as_ref().expect("a sash drags in the grid");
        let mut profile = self.profile_storage.lock().unwrap();
        for (key, part) in [("sideBar.size", Part::SideBar), ("panel.size", Part::Panel), ("auxiliaryBar.size", Part::AuxiliaryBar)] {
            profile.set(&format!("{STORAGE_PREFIX}{key}"), (grid.size(part).round() as i64).to_string());
        }
        let mut workspace = self.workspace_storage.lock().unwrap();
        for part in [Part::SideBar, Part::Panel, Part::AuxiliaryBar] {
            workspace.set(&format!("{STORAGE_PREFIX}{}", hidden_key(part)), (!grid.is_visible(part)).to_string());
        }
    }

    /// The grid's middle row for a window of `viewport`: the activity bar, the side bar, the editor and the sashes.
    pub(super) fn middle_section(&mut self, viewport: Size<Pixels>, editor: Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height) - TITLE_BAR_HEIGHT - STATUS_BAR_HEIGHT);
        let activity_bar_visible = self.is_part_visible(Part::ActivityBar);
        if self.part_grid.grid.is_none() {
            let sizes = self.stored_part_sizes(width, f32::from(viewport.height));
            let hidden: Vec<Part> = [Part::SideBar, Part::Panel, Part::AuxiliaryBar].into_iter().filter(|&part| self.stored_part_hidden(part)).collect();
            self.part_grid.grid = Some(Grid::new(width, height, sizes, |part| if part == Part::ActivityBar { activity_bar_visible } else { !hidden.contains(&part) }));
        }
        let grid = self.part_grid.grid.as_mut().expect("laid out above");
        grid.layout(width, height);
        grid.set_visible(Part::ActivityBar, activity_bar_visible);
        let row = grid.row_sizes();
        let editor_height = grid.column_sizes()[0];
        let sashes = grid.sashes();
        let dragging = grid.dragged_sash().is_some();
        let auxiliary_bar_visible = grid.is_visible(Part::AuxiliaryBar);
        let side_bar = self.is_part_visible(Part::SideBar).then(|| self.side_bar(window, cx));
        div()
            .relative()
            .h(px(height))
            .flex_none()
            .flex()
            .flex_row()
            .when(activity_bar_visible, |section| section.child(div().w(px(row[0])).flex_none().flex().pl(px(CARD_MARGIN)).pb(px(CARD_MARGIN)).child(self.activity_bar(cx))))
            .when_some(side_bar, |section, side_bar| {
                section.child(div().w(px(row[1])).flex_none().flex().when(!activity_bar_visible, |part| part.pl(px(CARD_MARGIN))).pb(px(CARD_MARGIN)).child(side_bar))
            })
            .child(div().w(px(row[2])).h(px(editor_height)).flex_none().flex().pl(px(CARD_MARGIN)).when(!auxiliary_bar_visible, |part| part.pr(px(CARD_MARGIN))).pb(px(CARD_MARGIN)).child(editor))
            .children(sashes.into_iter().map(|(sash, state, bounds)| self.part_sash(sash, state, bounds, cx)))
            .when(dragging, |section| section.child(self.part_sash_drag(cx)))
    }

    /// A `.monaco-sash` of the grid (sash.css): mac cursors, `sash.hoverBorder` after the hover delay and while dragged.
    fn part_sash(&self, sash: Sash, state: SashState, [x, y, width, height]: [f32; 4], cx: &mut Context<Self>) -> AnyElement {
        if state == SashState::Disabled {
            return div().into_any_element();
        }
        let vertical = matches!(sash, Sash::Row(_));
        let id = match sash {
            Sash::Row(index) => ("part-sash", index),
            Sash::Column => ("part-sash", usize::MAX),
        };
        let grid = self.part_grid.grid.as_ref().expect("sashes are drawn with the grid");
        let active = self.part_grid.hovered == Some(sash) || grid.dragged_sash() == Some(sash);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        // floatingPanels.css: a vertical sash's highlight stops `--modern-ui-floating-card-statusbar-margin` above the status bar.
        let highlight_inset = if vertical { CARD_MARGIN } else { 0. };
        let element = div()
            .id(id)
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .when(active, |sash| sash.child(div().absolute().top_0().left_0().right_0().bottom(px(highlight_inset)).bg(hover_border)));
        let cursor = match (vertical, state) {
            (true, SashState::AtMinimum) => CursorStyle::ResizeRight,
            (true, SashState::AtMaximum) => CursorStyle::ResizeLeft,
            (true, _) => CursorStyle::ResizeColumn,
            (false, SashState::AtMinimum) => CursorStyle::ResizeDown,
            (false, SashState::AtMaximum) => CursorStyle::ResizeUp,
            (false, _) => CursorStyle::ResizeRow,
        };
        element
            .cursor(cursor)
            .on_hover(cx.listener(move |workbench, hovered: &bool, window, cx| {
                // Sashes report enter and leave in any order within a frame; a leave clears only its own sash.
                if !*hovered {
                    if workbench.part_grid.hovered == Some(sash) {
                        workbench.part_grid.hovered = None;
                    }
                    if workbench.part_grid.hover.as_ref().is_some_and(|(pending, _)| *pending == sash) {
                        workbench.part_grid.hover = None;
                    }
                    cx.notify();
                    return;
                }
                workbench.part_grid.hovered = None;
                workbench.part_grid.hover = Some((sash, cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(SASH_HOVER_DELAY).await;
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.part_grid.hovered = Some(sash);
                            cx.notify();
                        });
                    })));
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workbench, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let position = if vertical { event.position.x } else { event.position.y };
                    workbench.part_grid.grid.as_mut().expect("sashes are drawn with the grid").start_sash_drag(sash, f32::from(position), event.modifiers.alt);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Follows the pointer anywhere in the window while a sash drags.
    fn part_sash_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity();
        let vertical = matches!(self.part_grid.grid.as_ref().and_then(Grid::dragged_sash), Some(Sash::Row(_)));
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    moved.update(cx, |workbench, cx| {
                        let position = if vertical { event.position.x } else { event.position.y };
                        workbench.part_grid.grid.as_mut().expect("a sash drags in the grid").drag_sash(f32::from(position), event.modifiers.alt);
                        cx.notify();
                    });
                });
                let released = workbench.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    released.update(cx, |workbench, cx| {
                        workbench.part_grid.grid.as_mut().expect("a sash drags in the grid").end_sash_drag();
                        workbench.store_part_layout();
                        cx.notify();
                    });
                });
            },
        )
        .size_0()
    }
}

#[cfg(test)]
mod tests;
