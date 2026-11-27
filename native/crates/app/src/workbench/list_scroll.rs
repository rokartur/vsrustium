//! The scrolling of the workbench lists (listView.ts over scrollableElement.ts): the wheel with
//! `workbench.list.mouseWheelScrollSensitivity`, `fastScrollSensitivity` and `smoothScrolling`, the scrollbars
//! that fade in on hover and scroll (with `workbench.list.scrollByPage` track presses), the top shadow, the
//! `list.scroll*` commands and the `listScrollAtBoundary` context key. The Explorer, the Search results and the
//! Keyboard Shortcuts table each keep their GPUI scroll state; this file drives it. A notebook editor's cell list
//! keeps its own scroll top (notebook_editor.rs).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    Axis, Bounds, Context, DispatchPhase, Hitbox, HitboxBehavior, IntoElement, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, ScrollDelta, ScrollHandle, ScrollWheelEvent, Styled, Task, Window, canvas, div, fill, point, prelude::*, px, size,
};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_view::Scrollbars;
use crate::commands::Outcome;
use crate::editor::Scrollbar;
use crate::list_tree::{SMOOTH_SCROLL_DURATION, paged_scroll_position, smooth_scroll_position, wheel_scroll_delta};

#[cfg(test)]
mod tests;

/// listView.ts leaves `verticalScrollbarSize` and `horizontalScrollbarSize` at scrollableElement.ts's 10.
const SCROLLBAR_SIZE: f32 = 10.;
/// scrollableElement.ts `HIDE_TIMEOUT`.
const HIDE_TIMEOUT: Duration = Duration::from_millis(500);
/// listCommands.ts `list.scrollUp` and the others move by 10px.
const SCROLL_STEP: f32 = 10.;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ListId {
    Explorer,
    Search,
    KeybindingsTable,
    /// An editor group's tab bar (multiEditorTabsControl.ts `tabsScrollbar`); `upper` is the sticky tabs' own bar.
    EditorTabs { group: u32, upper: bool },
    /// The cell list of the notebook editor of this tab model id.
    Notebook(u32),
}

/// The GPUI state a list scrolls with: a scrolled div or uniform list, a `list` of measured rows, or a notebook
/// list's scroll top and its maximum as they are now.
#[derive(Clone)]
pub enum ListScroller {
    Div(ScrollHandle),
    List(ListState),
    /// A tab bar's scrolled tabs, after `sticky_width` of sticky tabs that stay in place: the bar scrolls as one
    /// element, as multiEditorTabsControl.ts's tabs container does with its sticky tabs inside.
    Tabs { handle: ScrollHandle, sticky_width: f32 },
    Notebook { bounds: Bounds<Pixels>, top: f32, max: f32 },
}

impl ListScroller {
    pub(super) fn bounds(&self) -> Bounds<Pixels> {
        match self {
            ListScroller::Div(handle) => handle.bounds(),
            ListScroller::Tabs { handle, sticky_width } => {
                let bounds = handle.bounds();
                Bounds::new(bounds.origin - point(px(*sticky_width), px(0.)), size(bounds.size.width + px(*sticky_width), bounds.size.height))
            }
            ListScroller::List(state) => state.viewport_bounds(),
            ListScroller::Notebook { bounds, .. } => *bounds,
        }
    }

    /// (left, top), positive.
    pub(super) fn position(&self) -> Point<f32> {
        let offset = match self {
            ListScroller::Div(handle) | ListScroller::Tabs { handle, .. } => handle.offset(),
            ListScroller::List(state) => state.scroll_px_offset_for_scrollbar(),
            ListScroller::Notebook { top, .. } => return point(0., *top),
        };
        point(-f32::from(offset.x), -f32::from(offset.y))
    }

    pub(super) fn max_position(&self) -> Point<f32> {
        let max = match self {
            ListScroller::Div(handle) | ListScroller::Tabs { handle, .. } => handle.max_offset(),
            ListScroller::List(state) => state.max_offset_for_scrollbar(),
            ListScroller::Notebook { max, .. } => return point(0., max.max(0.)),
        };
        point(f32::from(max.x).max(0.), f32::from(max.y).max(0.))
    }

}

/// One list's scrollbars and the smooth scroll running on it.
#[derive(Default)]
pub struct ListScroll {
    bars: Scrollbars,
    hide_timer: Option<Task<()>>,
    drag: Option<SliderDrag>,
    smooth: Option<SmoothScroll>,
    animation: Option<Task<()>>,
    /// When the pointer last entered or left the list.
    hover_changed: Option<Instant>,
}

#[derive(Clone, Copy)]
struct SliderDrag {
    axis: Axis,
    pointer: f32,
    state: Scrollbar,
}

struct SmoothScroll {
    from: Point<f32>,
    to: Point<f32>,
    start: Instant,
}

#[derive(Default)]
pub struct ListScrolls(HashMap<ListId, ListScroll>);

impl Workbench {
    pub(super) fn list_scroller(&self, list: ListId) -> ListScroller {
        match list {
            ListId::Explorer => ListScroller::Div(self.explorer.scroll.clone()),
            ListId::Search => ListScroller::Div(self.search.scroll.0.borrow().base_handle.clone()),
            ListId::KeybindingsTable => ListScroller::List(self.keybindings_editor.list.clone()),
            ListId::EditorTabs { group, upper } => {
                let scroll = self.tab_scroll(group, upper);
                ListScroller::Tabs { sticky_width: scroll.sticky_width.get(), handle: scroll.handle }
            }
            ListId::Notebook(tab) => self.notebook_list_scroller(tab),
        }
    }

    /// `Scrollable.validateScrollPosition`: clamped to the content. Returns whether it moved.
    fn set_list_position(&mut self, list: ListId, position: Point<f32>) -> bool {
        let scroller = self.list_scroller(list);
        let max = scroller.max_position();
        let position = point(position.x.clamp(0., max.x).round(), position.y.clamp(0., max.y).round());
        if position == scroller.position() {
            return false;
        }
        match (scroller, list) {
            (ListScroller::Div(handle) | ListScroller::Tabs { handle, .. }, _) => handle.set_offset(point(px(-position.x), px(-position.y))),
            (ListScroller::List(state), _) => state.set_offset_from_scrollbar(point(px(0.), px(-position.y))),
            (ListScroller::Notebook { .. }, ListId::Notebook(tab)) => self.set_notebook_scroll_top(tab, position.y),
            (ListScroller::Notebook { .. }, _) => unreachable!("only notebook lists scroll a notebook"),
        }
        true
    }

    /// `IListService.lastFocusedList` for the scroll commands and context key.
    fn focused_list(&self, window: &Window) -> Option<ListId> {
        if self.keybindings_editor.table_focus.is_focused(window) {
            Some(ListId::KeybindingsTable)
        } else if self.search.focus.is_focused(window) {
            Some(ListId::Search)
        } else if self.explorer.focus.is_focused(window) {
            Some(ListId::Explorer)
        } else if self.notebook_views.list_focus.is_focused(window) {
            self.active_notebook_tab().map(|(tab, _)| ListId::Notebook(tab))
        } else {
            None
        }
    }

    fn list_scroll(&mut self, list: ListId) -> &mut ListScroll {
        self.list_scrolls.0.entry(list).or_default()
    }

    pub(super) fn horizontal_scrolling(&self, list: ListId) -> bool {
        // keybindingsEditor.ts creates its table with `horizontalScrolling: false`.
        match list {
            ListId::KeybindingsTable => false,
            ListId::EditorTabs { .. } => true,
            _ => self.setting("workbench.list.horizontalScrolling") == json!(true),
        }
    }

    /// The (vertical, horizontal) scrollbar sizes, 0 for a bar the list does not have.
    fn scrollbar_sizes(&self, list: ListId) -> (f32, f32) {
        match list {
            // multiEditorTabsControl.ts `createTabsScrollbar`: `vertical: Hidden`, `getTabsScrollbarSizing` 3 or 10.
            ListId::EditorTabs { .. } => (0., if self.setting("workbench.editor.titleScrollbarSizing") == json!("large") { 10. } else { 3. }),
            _ => (SCROLLBAR_SIZE, if self.horizontal_scrolling(list) { SCROLLBAR_SIZE } else { 0. }),
        }
    }

    /// Sets the scroll position now, ending a smooth scroll, and reveals the scrollbars.
    pub(super) fn set_list_scroll(&mut self, list: ListId, position: Point<f32>, cx: &mut Context<Self>) {
        let scroll = self.list_scroll(list);
        scroll.smooth = None;
        scroll.animation = None;
        if self.set_list_position(list, position) {
            self.list_scrolled(list, cx);
        }
    }

    fn list_scrolled(&mut self, list: ListId, cx: &mut Context<Self>) {
        // Chromium's mouse move after a scroll leaves the row the content took from under the pointer; GPUI sends none.
        self.leave_hover_target(cx);
        self.reveal_list_scrollbars(list, cx);
    }

    /// scrollableElement.ts `_reveal`: the scrollbars show, and fade after the timeout unless the pointer is on them.
    fn reveal_list_scrollbars(&mut self, list: ListId, cx: &mut Context<Self>) {
        let scroll = self.list_scroll(list);
        scroll.bars.set_shown(true, Instant::now());
        scroll.hide_timer = None;
        if !scroll.bars.mouse_over && scroll.drag.is_none() {
            scroll.hide_timer = Some(cx.spawn(async move |workbench, cx| {
                cx.background_executor().timer(HIDE_TIMEOUT).await;
                workbench.update(cx, |workbench, cx| workbench.hide_list_scrollbars(list, cx)).ok();
            }));
        }
        cx.notify();
    }

    fn hide_list_scrollbars(&mut self, list: ListId, cx: &mut Context<Self>) {
        let scroll = self.list_scroll(list);
        if !scroll.bars.mouse_over && scroll.drag.is_none() {
            scroll.bars.set_shown(false, Instant::now());
            cx.notify();
        }
    }

    /// scrollableElement.ts `_onMouseWheel`. A physical wheel (AppKit's line deltas) scrolls smoothly with
    /// `workbench.list.smoothScrolling`; a trackpad always scrolls at once.
    pub(super) fn list_wheel(&mut self, list: ListId, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x) / 40., f32::from(delta.y) / 40.),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        if let ListId::EditorTabs { group, .. } = list
            && self.switch_tab_by_wheel(group, event, window, cx)
        {
            return;
        }
        let tabs = matches!(list, ListId::EditorTabs { .. });
        let number = |key: &str, default: f64| self.setting(key).as_f64().unwrap_or(default) as f32;
        // The tabs' ScrollableElement keeps its default sensitivities and scrolls without animation.
        let (sensitivity, fast) = match tabs {
            true => (1., 5.),
            false => (number("workbench.list.mouseWheelScrollSensitivity", 1.), number("workbench.list.fastScrollSensitivity", 5.)),
        };
        let (mut delta_left, mut delta_top) = wheel_scroll_delta(x, y, event.modifiers.alt, sensitivity, fast);
        if !self.horizontal_scrolling(list) {
            delta_left = 0.;
        }
        // scrollableElement.ts `scrollYToX`.
        if tabs && delta_left == 0. {
            (delta_left, delta_top) = (delta_top, 0.);
        }
        let scroller = self.list_scroller(list);
        let smooth = !tabs && matches!(event.delta, ScrollDelta::Lines(_)) && self.setting("workbench.list.smoothScrolling") == json!(true);
        // `getFutureScrollPosition`: a running smooth scroll continues from where it is going.
        let future = self.list_scroll(list).smooth.as_ref().map_or_else(|| scroller.position(), |smooth| smooth.to);
        let max = scroller.max_position();
        let desired = point((future.x + delta_left).clamp(0., max.x), (future.y + delta_top).clamp(0., max.y));
        if desired == future {
            return;
        }
        if !smooth {
            self.set_list_scroll(list, desired, cx);
            return;
        }
        // scrollable.ts `SmoothScrollingOperation.start`: 10ms longer and started 10ms ago, for a quicker response.
        let now = Instant::now();
        let scroll = self.list_scroll(list);
        scroll.smooth = Some(SmoothScroll { from: scroller.position(), to: desired, start: now - Duration::from_millis(10) });
        scroll.animation = Some(cx.spawn_in(window, async move |workbench, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(8)).await;
                let running = workbench.update(cx, |workbench, cx| workbench.tick_smooth_scroll(list, cx)).unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));
    }

    fn tick_smooth_scroll(&mut self, list: ListId, cx: &mut Context<Self>) -> bool {
        let scroller = self.list_scroller(list);
        let size = scroller.bounds().size;
        let Some(smooth) = self.list_scroll(list).smooth.as_ref() else { return false };
        let completion = smooth.start.elapsed().as_secs_f32() * 1000. / (SMOOTH_SCROLL_DURATION + 10.);
        let position = point(
            smooth_scroll_position(smooth.from.x, smooth.to.x, f32::from(size.width), completion),
            smooth_scroll_position(smooth.from.y, smooth.to.y, f32::from(size.height), completion),
        );
        let done = completion >= 1.;
        if done {
            self.list_scroll(list).smooth = None;
        }
        if self.set_list_position(list, position) {
            self.list_scrolled(list, cx);
        }
        !done
    }

    /// listCommands.ts `list.scrollUp`, `scrollDown`, `scrollLeft`, `scrollRight` and `tree.toggleStickyScroll`.
    pub(super) fn register_list_scroll_commands(&mut self) {
        let moves: [(&str, f32, f32); 4] = [("list.scrollUp", 0., -SCROLL_STEP), ("list.scrollDown", 0., SCROLL_STEP), ("list.scrollLeft", -SCROLL_STEP, 0.), ("list.scrollRight", SCROLL_STEP, 0.)];
        for (id, left, top) in moves {
            self.commands.register(id, move |workbench, _, window, cx| {
                if let Some(list) = workbench.focused_list(window) {
                    let position = workbench.list_scroller(list).position();
                    workbench.set_list_scroll(list, point(position.x + left, position.y + top), cx);
                }
                Outcome::done()
            });
        }
        self.commands.register("tree.toggleStickyScroll", |workbench, _, _, cx| {
            let enabled = workbench.setting("workbench.tree.enableStickyScroll") != json!(false);
            workbench.update_setting("workbench.tree.enableStickyScroll", json!(!enabled));
            cx.notify();
            Outcome::done()
        });
    }

    /// listService.ts `createScrollObserver`: where the focused list's scroll position is.
    pub(super) fn list_scroll_context(&self, window: &Window, context: &mut HashMap<String, Value>) {
        let Some(list) = self.focused_list(window) else { return };
        let scroller = self.list_scroller(list);
        let (position, max) = (scroller.position(), scroller.max_position());
        let at_top = position.y == 0.;
        // The reference's threshold of 1, since scrollHeight is rounded.
        let at_bottom = max.y - position.y < 1.;
        let boundary = match (at_top, at_bottom) {
            (true, true) => "both",
            (true, false) => "top",
            (false, true) => "bottom",
            (false, false) => "none",
        };
        context.insert("listScrollAtBoundary".into(), json!(boundary));
    }

    /// The list's scroll layer, laid over the list's own bounds by its relative parent: it takes the wheel before the
    /// list, draws the top shadow and the scrollbars, and moves the list from them.
    pub(super) fn list_scroll_layer(&self, list: ListId, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let scroller = self.list_scroller(list);
        let bounds = scroller.bounds();
        let (position, max) = (scroller.position(), scroller.max_position());
        let horizontal = self.horizontal_scrolling(list);
        let (vertical_size, horizontal_size) = self.scrollbar_sizes(list);
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let vertical_bar = (vertical_size > 0.).then(|| Scrollbar::new(horizontal_size, height, height + max.y, position.y));
        let horizontal_bar = horizontal.then(|| Scrollbar::new(vertical_size, width, width + max.x, position.x));
        let tabs = matches!(list, ListId::EditorTabs { .. });
        let scroll_by_page = !tabs && self.setting("workbench.list.scrollByPage") == json!(true);
        let shadow = (position.y > 0.).then(|| self.scroll_shadow());
        let now = Instant::now();
        let (mut opacity, mut shown, animating, drag) = match self.list_scrolls.0.get(&list) {
            Some(scroll) => (scroll.bars.opacity(now), scroll.bars.shown, scroll.bars.in_transition(now), scroll.drag),
            None => (0., false, false, None),
        };
        // multiEditorTabsControl.ts `getTabsScrollbarVisibility`: `visible` and `hidden` override the fade.
        match self.setting("workbench.editor.titleScrollbarVisibility").as_str().filter(|_| tabs) {
            Some("visible") => (opacity, shown) = (1., true),
            Some("hidden") => (opacity, shown) = (0., false),
            _ => {}
        }
        let faded = |key: &str| {
            let color = self.color(key);
            Rgba { a: color.a * opacity, ..color }
        };
        let colors = [faded("scrollbarSlider.background"), faded("scrollbarSlider.hoverBackground"), faded("scrollbarSlider.activeBackground")];
        let workbench = cx.entity();
        let layer = canvas(
            move |_, window, _| {
                let list_hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                let mut tracks: Vec<(Hitbox, Bounds<Pixels>, Axis, Scrollbar)> = Vec::new();
                let bars = [(Axis::Vertical, vertical_bar), (Axis::Horizontal, horizontal_bar)];
                for (axis, bar) in bars {
                    let Some(bar) = bar else { continue };
                    let track = match axis {
                        Axis::Vertical => Bounds::new(point(bounds.right() - px(vertical_size), bounds.top()), size(px(vertical_size), px(bar.track))),
                        // tabs.css: the Modern UI tabs' scrollbar sits on the strip's top edge.
                        Axis::Horizontal if tabs => Bounds::new(bounds.origin, size(px(bar.track), px(horizontal_size))),
                        Axis::Horizontal => Bounds::new(point(bounds.left(), bounds.bottom() - px(horizontal_size)), size(px(bar.track), px(horizontal_size))),
                    };
                    // An invisible scrollbar takes no pointer events (scrollbars.css `.invisible`).
                    if bar.needed && shown {
                        tracks.push((window.insert_hitbox(track, HitboxBehavior::BlockMouse), track, axis, bar));
                    }
                }
                (list_hitbox, tracks)
            },
            move |_, (list_hitbox, tracks), window, _| {
                let mut sliders = Vec::new();
                for (hitbox, track, axis, bar) in &tracks {
                    // multieditortabscontrol.css: the tabs' scrollbar is `cursor: default` in the pointer strip.
                    if tabs {
                        window.set_cursor_style(gpui::CursorStyle::Arrow, hitbox);
                    }
                    let slider = match axis {
                        Axis::Vertical => Bounds::new(track.origin + point(px(0.), px(bar.slider_position)), size(track.size.width, px(bar.slider_size))),
                        Axis::Horizontal => Bounds::new(track.origin + point(px(bar.slider_position), px(0.)), size(px(bar.slider_size), track.size.height)),
                    };
                    let color = match drag {
                        Some(drag) if drag.axis == *axis => colors[2],
                        _ if slider.contains(&window.mouse_position()) => colors[1],
                        _ => colors[0],
                    };
                    // tabs.css: the tabs' slider is `cornerRadius-circle`.
                    let radius = if tabs { slider.size.height / 2. } else { px(0.) };
                    window.paint_quad(fill(slider, color).corner_radii(radius));
                    sliders.push(slider);
                }
                if animating {
                    window.request_animation_frame();
                }
                let workbench_wheel = workbench.clone();
                let wheel_hitbox = list_hitbox.clone();
                // Capture phase: the list's own GPUI scrolling must not see the wheel.
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    // Not `is_hovered`: it is false after a key press until the pointer moves, and a wheel does not move it.
                    if phase == DispatchPhase::Capture && wheel_hitbox.should_handle_scroll(window) {
                        cx.stop_propagation();
                        workbench_wheel.update(cx, |workbench, cx| workbench.list_wheel(list, event, window, cx));
                    }
                });
                let hovered_slider = move |window: &Window| sliders.iter().any(|slider| slider.contains(&window.mouse_position()));
                let painted_hover = hovered_slider(window);
                let painted_over = list_hitbox.is_hovered(window);
                let workbench_moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if let Some(drag) = drag {
                        let along = if drag.axis == Axis::Vertical { event.position.y } else { event.position.x };
                        workbench_moved.update(cx, |workbench, cx| workbench.drag_list_slider(list, drag, f32::from(along), cx));
                        return;
                    }
                    let over = list_hitbox.is_hovered(window) || bounds.contains(&event.position);
                    if over != painted_over {
                        workbench_moved.update(cx, |workbench, cx| workbench.list_mouse_over(list, over, cx));
                    } else if hovered_slider(window) != painted_hover {
                        window.refresh();
                    }
                });
                let workbench_released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && drag.is_some() {
                        workbench_released.update(cx, |workbench, cx| {
                            workbench.list_scroll(list).drag = None;
                            workbench.hide_list_scrollbars(list, cx);
                        });
                    }
                });
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let Some((_, track, axis, bar)) = tracks.iter().find(|(hitbox, ..)| hitbox.is_hovered(window)) else { return };
                    let (track, axis, bar) = (*track, *axis, *bar);
                    let along = |position: Point<Pixels>| f32::from(if axis == Axis::Vertical { position.y } else { position.x });
                    let start = if axis == Axis::Vertical { track.top() } else { track.left() };
                    let offset = along(event.position) - f32::from(start);
                    cx.stop_propagation();
                    workbench.update(cx, |workbench, cx| workbench.press_list_scrollbar(list, axis, bar, offset, along(event.position), scroll_by_page, cx));
                });
            },
        )
        .absolute()
        .inset_0();
        div()
            .absolute()
            .inset_0()
            .child(layer)
            // scrollbars.css `.shadow.top`: 3px from the left, 3px tall.
            .children(shadow.map(|shadow| div().absolute().top_0().left(px(3.)).right_0().h(px(3.)).overflow_hidden().child(shadow.size_full())))
    }

    /// Whether the pointer is over the list, and since when.
    pub(super) fn list_hover(&self, list: ListId) -> Option<(bool, Instant)> {
        let scroll = self.list_scrolls.0.get(&list)?;
        Some((scroll.bars.mouse_over, scroll.hover_changed?))
    }

    fn list_mouse_over(&mut self, list: ListId, over: bool, cx: &mut Context<Self>) {
        let scroll = self.list_scroll(list);
        scroll.bars.mouse_over = over;
        scroll.hover_changed = Some(Instant::now());
        match over {
            true => self.reveal_list_scrollbars(list, cx),
            false => self.hide_list_scrollbars(list, cx),
        }
    }

    /// abstractScrollbar.ts `_onPointerDown`: off the slider the track jumps (a page with `scrollByPage`, else
    /// centering the slider on the pointer), then the slider follows the pointer.
    #[allow(clippy::too_many_arguments)]
    fn press_list_scrollbar(&mut self, list: ListId, axis: Axis, bar: Scrollbar, offset: f32, pointer: f32, scroll_by_page: bool, cx: &mut Context<Self>) {
        let scroller = self.list_scroller(list);
        let position = scroller.position();
        let size = scroller.bounds().size;
        let (current, visible) = match axis {
            Axis::Vertical => (position.y, f32::from(size.height)),
            Axis::Horizontal => (position.x, f32::from(size.width)),
        };
        let on_slider = (bar.slider_position..bar.slider_position + bar.slider_size).contains(&offset);
        if !on_slider {
            let desired = match scroll_by_page {
                true => paged_scroll_position(offset, bar.slider_position, current, visible),
                false => bar.position_at(offset),
            };
            self.set_list_scroll(list, with_axis(position, axis, desired), cx);
        }
        let (position, max) = (scroller.position(), scroller.max_position());
        let state = match axis {
            Axis::Vertical => Scrollbar::new(self.scrollbar_sizes(list).1, visible, visible + max.y, position.y),
            Axis::Horizontal => Scrollbar::new(self.scrollbar_sizes(list).0, visible, visible + max.x, position.x),
        };
        self.list_scroll(list).drag = Some(SliderDrag { axis, pointer, state });
        self.list_scrolled(list, cx);
    }

    fn drag_list_slider(&mut self, list: ListId, drag: SliderDrag, pointer: f32, cx: &mut Context<Self>) {
        let position = self.list_scroller(list).position();
        let desired = drag.state.position_dragged(pointer - drag.pointer);
        self.set_list_scroll(list, with_axis(position, drag.axis, desired), cx);
    }
}

fn with_axis(position: Point<f32>, axis: Axis, value: f32) -> Point<f32> {
    match axis {
        Axis::Vertical => point(position.x, value),
        Axis::Horizontal => point(value, position.y),
    }
}


