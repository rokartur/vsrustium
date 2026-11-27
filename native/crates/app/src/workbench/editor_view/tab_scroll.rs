//! The editor tab bars' scrolling (multiEditorTabsControl.ts `tabsScrollbar` and `doLayoutTabsNonWrapping`,
//! connectedTabClipping.ts): the tabs scroll under their own scrollbar, the wheel scrolls them sideways, the active
//! tab is revealed when it or the bar's widths change, and sticky compact or shrink tabs stay in place.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui::{Context, Div, ScrollDelta, ScrollHandle, ScrollWheelEvent, Window, canvas, div, point, prelude::*, px};

use super::super::Workbench;
use super::super::list_scroll::ListId;
use super::TabRow;

/// multiEditorTabsControl.ts `TAB_WIDTH.fit`: with less room than this left beside them, sticky tabs scroll too.
const TAB_WIDTH_FIT: f32 = 120.;
/// `--modern-ui-connected-tab-shoulder-radius` and `--modern-ui-connected-tab-cap-radius`: 6px plus the stroke.
pub(super) const CONNECTED_TAB_RADIUS: f32 = 7.;
/// multiEditorTabsControl.ts `MOUSE_WHEEL_EVENT_THRESHOLD` (ms) and `MOUSE_WHEEL_DISTANCE_THRESHOLD`.
const MOUSE_WHEEL_EVENT_THRESHOLD: f32 = 150.;
const MOUSE_WHEEL_DISTANCE_THRESHOLD: f32 = 1.5;

/// One tab bar's scroll state, kept per group and bar.
#[derive(Clone, Default)]
pub(in crate::workbench) struct TabScroll {
    pub(in crate::workbench) handle: ScrollHandle,
    /// The width of the bar's tabs, sticky and scrolled, as last laid out.
    area_width: Rc<Cell<f32>>,
    /// The width of the sticky tabs that stay in place before the scrolled ones.
    pub(in crate::workbench) sticky_width: Rc<Cell<f32>>,
    /// The model ids of the scrolled tabs as last laid out: `handle`'s child bounds are theirs.
    laid_out: Rc<RefCell<Vec<u32>>>,
    /// The active tab and the bar's widths the last reveal ran for.
    revealed: Rc<Cell<Option<(u32, f32, f32)>>>,
}

/// connectedTabClipping.ts `updateConnectedTabClipping` for the active connected tab, in its own coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct TabClip {
    /// How much of the fill is cut at its start and end; a cut end keeps its cap (and the end its shoulder) at
    /// the bar's edge, as the overflow edge does.
    pub(super) left: f32,
    pub(super) right: f32,
    /// The left shoulder would leave the bar (`connected-tab-left-edge`).
    pub(super) left_edge: bool,
    /// Too little of the fill is left to draw (`connected-tab-hidden`).
    pub(super) hidden: bool,
}

/// `updateConnectedTabClipping` with the fill at `fill_left..fill_right` and the bar showing `scroll_left` on for `width`.
pub(super) fn connected_tab_clip(fill_left: f32, fill_right: f32, scroll_left: f32, width: f32) -> TabClip {
    let (visible_left, visible_right) = (scroll_left, scroll_left + width);
    let hidden = fill_left.max(visible_left) + CONNECTED_TAB_RADIUS >= fill_right.min(visible_right);
    TabClip {
        left: (visible_left - fill_left).max(0.),
        right: (fill_right + CONNECTED_TAB_RADIUS - visible_right).max(0.),
        left_edge: fill_left - CONNECTED_TAB_RADIUS < visible_left,
        hidden,
    }
}

/// `doLayoutTabsNonWrapping`'s reveal of the tab at `left..right` in a bar showing `scroll_left` on for `width`:
/// minimally to the right while it fits, else to its start.
fn revealed_scroll_left(scroll_left: f32, width: f32, left: f32, right: f32) -> f32 {
    let fits = right - left <= width;
    if fits && scroll_left + width < right {
        right - width
    } else if scroll_left > left || !fits {
        left
    } else {
        scroll_left
    }
}

impl Workbench {
    pub(in crate::workbench) fn tab_scroll(&self, group: u32, upper: bool) -> TabScroll {
        self.editor_groups.tab_scrolls.borrow_mut().entry((group, upper)).or_default().clone()
    }

    /// The tabs of one bar of the active group, `upper` for the sticky tabs' own bar: the sticky ones that stay in
    /// place, then the scrolled ones with their scrollbar.
    pub(super) fn tab_bar_tabs(&self, tabs: Range<usize>, upper: bool, height: f32, cx: &mut Context<Self>) -> Div {
        let group = self.editors.id;
        let scroll = self.tab_scroll(group, upper);
        let active = self.editors.active_index().expect("tab bars are drawn only with an editor open");
        let sticky_width: f32 = tabs.clone().map_while(|index| self.sticky_tab_width(index)).sum();
        let fixed = match !upper && sticky_width > 0. && scroll.area_width.get() - sticky_width >= TAB_WIDTH_FIT {
            true => tabs.start + tabs.clone().take_while(|index| self.sticky_tab_width(*index).is_some()).count(),
            false => tabs.start,
        };
        scroll.sticky_width.set(if fixed > tabs.start { sticky_width } else { 0. });
        let scrolled = fixed..tabs.end;
        let ids: Vec<u32> = scrolled.clone().map(|index| self.editors.editors[index].model_id).collect();
        let clip = match !upper && scrolled.contains(&active) && *scroll.laid_out.borrow() == ids {
            true => scroll.handle.bounds_for_item(active - fixed).map(|tab| {
                let bounds = scroll.handle.bounds();
                let left = f32::from(tab.left() - bounds.left());
                connected_tab_clip(left, left + f32::from(tab.size.width), -f32::from(scroll.handle.offset().x), f32::from(bounds.size.width))
            }),
            false => None,
        };
        let row = |index: usize| match upper {
            true => TabRow::Upper,
            false => TabRow::Main { first: index == tabs.start, clip: clip.filter(|_| index == active).unwrap_or_default(), after_sticky: fixed > tabs.start },
        };
        let active_id = self.editors.editors[active].model_id;
        let reveal = scrolled.contains(&active).then_some(active - fixed);
        let has_left_neighbor = active > tabs.start;
        let last = active + 1 == tabs.end;
        let (measured, laid_out) = (scroll.clone(), scroll.clone());
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .child(canvas(move |bounds, _, _| measured.area_width.set(f32::from(bounds.size.width)), |_, _, _, _| {}).absolute().size_full())
            .child(div().flex_none().h_full().flex().children((tabs.start..fixed).map(|index| self.tab(index, index == active, row(index), cx))))
            // The tabs' overflow also clips vertically in GPUI, so the scrolled box reaches into the part's top
            // border row, where the active tab's fill starts.
            .child(
                div()
                    .id((if upper { "upper-tabs" } else { "tabs" }, group as usize))
                    .flex_1()
                    .min_w_0()
                    .mt(px(-1.))
                    .pt(px(1.))
                    .h(px(height + 1.))
                    // connectedEditorTabs.css `.connected-tab-last`: the last tab keeps its shoulder and stroke after it.
                    .when(!upper, |tabs| tabs.pr(px(CONNECTED_TAB_RADIUS + 1.)))
                    .flex()
                    .overflow_x_scroll()
                    .track_scroll(&scroll.handle)
                    .children(scrolled.map(|index| self.tab(index, index == active, row(index), cx))),
            )
            .child(self.list_scroll_layer(ListId::EditorTabs { group, upper }, cx))
            // After the tabs' prepaint: their bounds are this frame's.
            .child(
                canvas(
                    move |_, window, _| {
                        *laid_out.laid_out.borrow_mut() = ids;
                        let Some(child) = reveal else { return };
                        let handle = &laid_out.handle;
                        let (bounds, Some(tab)) = (handle.bounds(), handle.bounds_for_item(child)) else { return };
                        let width = f32::from(bounds.size.width);
                        let key = (active_id, width, width + f32::from(handle.max_offset().x));
                        if laid_out.revealed.replace(Some(key)) == Some(key) {
                            return;
                        }
                        // The connected fill's shoulders count, and the last tab's 1px border.
                        let left_shoulder = if has_left_neighbor && !upper { CONNECTED_TAB_RADIUS } else { 0. };
                        let left = (f32::from(tab.left() - bounds.left()) - left_shoulder).floor();
                        let right = match upper {
                            true => f32::from(tab.right() - bounds.left()),
                            false => (f32::from(tab.right() - bounds.left()) + CONNECTED_TAB_RADIUS).ceil() + if last { 1. } else { 0. },
                        };
                        let scroll_left = -f32::from(handle.offset().x);
                        let target = revealed_scroll_left(scroll_left, width, left, right).clamp(0., f32::from(handle.max_offset().x).max(0.));
                        if target != scroll_left {
                            handle.set_offset(point(px(-target), px(0.)));
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
    }

    /// multiEditorTabsControl.ts `getStickyTabWidth`: a sticky tab's width with `workbench.editor.pinnedTabSizing`
    /// compact (28px with the modern UI tabs) or shrink (80px); `None` for other tabs and `normal`.
    pub(super) fn sticky_tab_width(&self, index: usize) -> Option<f32> {
        if !self.editors.editors[index].sticky {
            return None;
        }
        match self.setting("workbench.editor.pinnedTabSizing").as_str() {
            Some("compact") => Some(28.),
            Some("shrink") => Some(80.),
            _ => None,
        }
    }

    /// multiEditorTabsControl.ts's wheel listener: with `workbench.editor.scrollToSwitchTabs` (or Shift without it)
    /// the wheel opens the next or previous tab instead of scrolling. Returns whether it did.
    pub(in crate::workbench) fn switch_tab_by_wheel(&mut self, group: u32, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let switches = self.setting("workbench.editor.scrollToSwitchTabs") == serde_json::json!(true);
        if switches == event.modifiers.shift {
            return false;
        }
        let (count, active) = self.with_group(group, |workbench| (workbench.editors.editors.len(), workbench.editors.active_index()));
        let Some(active) = active.filter(|_| count >= 2) else { return false };
        // The DOM's wheel deltas: positive to the end, about 40px a line.
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => -f32::from(delta.x + delta.y),
            ScrollDelta::Lines(delta) => -(delta.x + delta.y) * 40.,
        };
        let now = Instant::now();
        let recent = self.editor_groups.last_tab_wheel.get().is_some_and(|last| (now - last).as_secs_f32() * 1000. < MOUSE_WHEEL_EVENT_THRESHOLD - 2. * delta.abs());
        if recent {
            return true;
        }
        self.editor_groups.last_tab_wheel.set(Some(now));
        let next = match delta {
            delta if delta < -MOUSE_WHEEL_DISTANCE_THRESHOLD => active.checked_sub(1),
            delta if delta > MOUSE_WHEEL_DISTANCE_THRESHOLD => Some(active + 1).filter(|next| *next < count),
            _ => None,
        };
        let Some(next) = next else { return true };
        self.focus_group(group, window, cx);
        self.editors.activate(next);
        self.editors_changed(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use exthost::scan::scan_extension;
    use gpui::{TestAppContext, VisualTestContext};
    use serde_json::json;

    use super::*;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    /// 30 files open as tabs, the last one active, with `settings`.
    fn thirty_tabs<'a>(name: &str, settings: serde_json::Value, cx: &'a mut TestAppContext) -> (gpui::Entity<Workbench>, &'a mut VisualTestContext) {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-tab-scroll-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("User")).unwrap();
        let settings_service = config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None);
        let settings_service = std::sync::Arc::new(std::sync::Mutex::new(settings_service));
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings_service, HostSync::new(std::sync::mpsc::channel().0), cx);
            for (key, value) in settings.as_object().unwrap() {
                workbench.update_setting(key, value.clone());
            }
            for index in 0..30 {
                let path = dir.join(format!("file-{index:02}.txt"));
                std::fs::write(&path, "text").unwrap();
                workbench.open_file(path, true, window, cx);
            }
            workbench
        });
        cx.run_until_parked();
        (workbench, cx)
    }

    /// The active tab's left and right in the bar's viewport, and the bar's width.
    fn active_tab_in_view(workbench: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> (f32, f32, f32) {
        workbench.read_with(cx, |workbench, _| {
            let scroll = workbench.tab_scroll(workbench.editors.id, false);
            let fixed = workbench.editors.editors.len() - scroll.laid_out.borrow().len();
            let tab = scroll.handle.bounds_for_item(workbench.editors.active_index().unwrap() - fixed).unwrap();
            let bounds = scroll.handle.bounds();
            let left = f32::from(tab.left() - bounds.left() + scroll.handle.offset().x);
            (left, left + f32::from(tab.size.width), f32::from(bounds.size.width))
        })
    }

    fn activate(workbench: &gpui::Entity<Workbench>, index: usize, cx: &mut VisualTestContext) {
        workbench.update(cx, |workbench, cx| {
            workbench.editors.activate(index);
            workbench.editors_changed(cx);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_active_tab_is_scrolled_into_view(cx: &mut TestAppContext) {
        let (workbench, cx) = thirty_tabs("reveal", json!({}), cx);
        let (left, right, width) = active_tab_in_view(&workbench, cx);
        // The last tab ends at the bar's end, less its shoulder and its 1px stroke.
        assert!(left > 0. && (width - right - (CONNECTED_TAB_RADIUS + 1.)).abs() < 0.5, "{left} {right} {width}");
        activate(&workbench, 0, cx);
        let (left, ..) = active_tab_in_view(&workbench, cx);
        assert_eq!(left, 0.);
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.tab_scroll(workbench.editors.id, false).handle.offset().x), px(0.));
    }

    #[gpui::test]
    fn sticky_shrink_tabs_stay_in_place_while_the_others_scroll(cx: &mut TestAppContext) {
        let (workbench, cx) = thirty_tabs("sticky", json!({ "workbench.editor.pinnedTabSizing": "shrink" }), cx);
        for index in 0..2 {
            workbench.update(cx, |workbench, _| workbench.editors.stick(index));
        }
        activate(&workbench, 29, cx);
        let (scrolled, offset) = workbench.read_with(cx, |workbench, _| {
            let scroll = workbench.tab_scroll(workbench.editors.id, false);
            (scroll.laid_out.borrow().len(), scroll.handle.offset().x)
        });
        // The two sticky tabs sit outside the scrolled box, which is scrolled.
        assert_eq!(scrolled, 28);
        assert!(offset < px(0.));
        // With `normal` sizing they scroll with the others.
        workbench.update(cx, |workbench, cx| {
            workbench.update_setting("workbench.editor.pinnedTabSizing", json!("normal"));
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.tab_scroll(workbench.editors.id, false).laid_out.borrow().len()), 30);
    }

    #[test]
    fn a_tab_is_revealed_by_the_smallest_scroll() {
        // Off to the right: its end comes to the bar's end.
        assert_eq!(revealed_scroll_left(0., 300., 350., 450.), 150.);
        // Off to the left: its start comes to the bar's start.
        assert_eq!(revealed_scroll_left(200., 300., 100., 180.), 100.);
        // In view: no scroll.
        assert_eq!(revealed_scroll_left(100., 300., 150., 250.), 100.);
        // Wider than the bar: its start.
        assert_eq!(revealed_scroll_left(0., 100., 50., 400.), 50.);
    }

    #[test]
    fn a_clipped_connected_tab_keeps_its_cap_and_shoulder_in_the_bar() {
        // Fully in view, shoulders included.
        assert_eq!(connected_tab_clip(50., 150., 0., 300.), TabClip::default());
        // Its left shoulder leaves the bar, its fill does not.
        assert_eq!(connected_tab_clip(3., 100., 0., 300.), TabClip { left_edge: true, ..TabClip::default() });
        // Cut on the left by 20px.
        assert_eq!(connected_tab_clip(80., 200., 100., 300.), TabClip { left: 20., left_edge: true, ..TabClip::default() });
        // Its end within 7px of the bar's end: the fill ends 7px before it, the shoulder in the last 7px.
        assert_eq!(connected_tab_clip(200., 298., 0., 300.), TabClip { right: 5., ..TabClip::default() });
        // Out of view but for 5px.
        assert_eq!(connected_tab_clip(295., 400., 0., 300.), TabClip { right: 107., hidden: true, ..TabClip::default() });
    }
}
