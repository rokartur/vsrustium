//! What abstractTree.ts draws over the Explorer and Search trees: indent guides (`workbench.tree.renderIndentGuides`)
//! and sticky scroll (`workbench.tree.enableStickyScroll`, `stickyScrollMaxItemCount`).

use std::time::Duration;

use gpui::{AnyElement, Context, Div, DispatchPhase, MouseButton, MouseDownEvent, Rgba, SharedString, Window, canvas, div, point, prelude::*, px};
use serde_json::json;

use super::Workbench;
use super::list_scroll::ListId;
use crate::list_tree::{StickyRow, active_guide_parents, indent_guides, position_below_widget, sticky_rows};

const ROW_HEIGHT: f32 = 22.;
/// tree.css `.monaco-enable-motion .monaco-tl-indent > .indent-guide { transition: opacity 0.1s linear }`.
const GUIDE_FADE: Duration = Duration::from_millis(100);
/// tree.css `.monaco-tl-indent { left: 16px }`, from the row's `.monaco-tl-row`.
const GUIDES_LEFT: f32 = 16.;

/// The indent guides of one render of a tree: the depths of its rows and the rows whose guides are active.
pub struct TreeGuides {
    depths: Vec<usize>,
    active_parents: Vec<usize>,
    mode: GuidesMode,
    indent: f32,
    inactive: Rgba,
    active: Rgba,
    /// The element group whose hover shows the inactive guides, for a tree without list scroll state (no fade).
    hover_group: Option<SharedString>,
}

#[derive(Clone, Copy, PartialEq)]
enum GuidesMode {
    None,
    OnHover,
    Always,
}

impl TreeGuides {
    /// The guides of row `index`, an absolute layer for the list row it is put in; `inset` is where the row's
    /// `.monaco-tl-row` starts inside that element.
    pub fn row(&self, index: usize, inset: f32) -> Option<Div> {
        if self.mode == GuidesMode::None {
            return None;
        }
        let guides = indent_guides(&self.depths, index, &self.active_parents);
        if guides.is_empty() {
            return None;
        }
        let mut layer = div().absolute().top_0().bottom_0().left_0();
        for (level, active) in guides.into_iter().enumerate() {
            let color = if active { self.active } else { self.inactive };
            let guide = div().absolute().top_0().bottom_0().left(px(inset + GUIDES_LEFT + level as f32 * self.indent)).w(px(1.)).bg(color);
            let guide = match (&self.hover_group, active || self.mode == GuidesMode::Always) {
                (Some(group), false) => guide.invisible().group_hover(group.clone(), |guide| guide.visible()),
                _ => guide,
            };
            layer = layer.child(guide);
        }
        Some(layer)
    }
}

impl Workbench {
    /// abstractTree.ts `style`: inactive guides show while the pointer is over the list (or always), active ones
    /// always, in `tree.inactiveIndentGuidesStroke` and `tree.indentGuidesStroke`.
    pub(super) fn tree_guides(&self, list: ListId, depths: Vec<usize>, active: &[usize], window: &Window) -> TreeGuides {
        let mode = self.guides_mode();
        let opacity = match mode {
            GuidesMode::Always => 1.,
            _ => self.list_hover_fade(list, GUIDE_FADE, window),
        };
        let inactive = self.color("tree.inactiveIndentGuidesStroke");
        TreeGuides {
            active_parents: active_guide_parents(&depths, active),
            depths,
            mode,
            indent: self.tree_indent(),
            inactive: Rgba { a: inactive.a * opacity, ..inactive },
            active: self.color("tree.indentGuidesStroke"),
            hover_group: None,
        }
    }

    /// `tree_guides` for a tree drawn in a plain element group: inactive guides show while the pointer is over
    /// `group`, without the fade.
    pub(super) fn tree_guides_in_group(&self, group: SharedString, depths: Vec<usize>, active: &[usize]) -> TreeGuides {
        TreeGuides {
            active_parents: active_guide_parents(&depths, active),
            depths,
            mode: self.guides_mode(),
            indent: self.tree_indent(),
            inactive: self.color("tree.inactiveIndentGuidesStroke"),
            active: self.color("tree.indentGuidesStroke"),
            hover_group: Some(group),
        }
    }

    fn guides_mode(&self) -> GuidesMode {
        match self.setting("workbench.tree.renderIndentGuides").as_str() {
            Some("none") => GuidesMode::None,
            Some("always") => GuidesMode::Always,
            _ => GuidesMode::OnHover,
        }
    }

    /// The rows sticky scroll shows over a tree with these row depths, none with it off.
    pub(super) fn tree_sticky_rows(&self, list: ListId, depths: &[usize]) -> Vec<StickyRow> {
        if self.setting("workbench.tree.enableStickyScroll") == json!(false) {
            return Vec::new();
        }
        let scroller = self.list_scroller(list);
        let height = f32::from(scroller.bounds().size.height);
        sticky_rows(depths, ROW_HEIGHT, scroller.position().y, height, self.sticky_scroll_max_items())
    }

    fn sticky_scroll_max_items(&self) -> usize {
        self.setting("workbench.tree.stickyScrollMaxItemCount").as_u64().unwrap_or(7) as usize
    }

    /// `StickyScrollWidget`: the sticky rows over the top of the list, each drawn by the tree's own renderer, on the
    /// side bar background with the `scrollbar.shadow` below. A left press scrolls the row's source right under the
    /// widget and focuses and selects it (`handleStickyScrollMouseEvent`); other buttons reach the row.
    pub(super) fn tree_sticky_container(&self, list: ListId, sticky: &[StickyRow], rows: Vec<AnyElement>, cx: &mut Context<Self>) -> Option<Div> {
        let last = sticky.last()?;
        let background = self.color("sideBar.background");
        let hover = self.color("list.hoverBackground");
        let mut container = div().absolute().top_0().left_0().right_0().h(px(last.position + ROW_HEIGHT)).bg(background);
        for (sticky_row, row) in sticky.iter().zip(rows) {
            let index = sticky_row.index;
            container = container.child(
                div()
                    .id(("sticky-row", index))
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(sticky_row.position))
                    .h(px(ROW_HEIGHT))
                    .overflow_hidden()
                    .bg(background)
                    .hover(move |row| row.bg(hover))
                    .cursor_pointer()
                    .child(row)
                    .child(self.sticky_press_layer(list, index, cx)),
            );
        }
        let shadow = self.theme.color("scrollbar.shadow").map(|_| div().absolute().left_0().right_0().bottom(px(-3.)).h(px(3.)).overflow_hidden().child(self.scroll_shadow().size_full()));
        Some(container.children(shadow))
    }

    /// Takes a left press on a sticky row in the capture phase, before the row's own handlers.
    fn sticky_press_layer(&self, list: ListId, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let workbench = cx.entity();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let workbench = workbench.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left && hitbox.is_hovered(window) {
                        cx.stop_propagation();
                        workbench.update(cx, |workbench, cx| workbench.sticky_row_pressed(list, index, window, cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn sticky_row_pressed(&mut self, list: ListId, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let depths = self.tree_depths(list);
        let top = index as f32 * ROW_HEIGHT - position_below_widget(&depths, index, ROW_HEIGHT, self.sticky_scroll_max_items());
        let left = self.list_scroller(list).position().x;
        self.set_list_scroll(list, point(left, top), cx);
        match list {
            ListId::Explorer => {
                let Some(tree) = self.folder.as_mut() else { return };
                let Some(row) = tree.rows().into_iter().nth(index) else { return };
                tree.set_selection(&row.path);
                window.focus(&self.explorer.focus.clone(), cx);
            }
            ListId::Search => {
                let rows = self.search.rows(&self.search_folders(), Some(&self.collapse_results()));
                let Some(row) = rows.into_iter().nth(index) else { return };
                self.search.focus_and_select(Some(row.key));
                window.focus(&self.search.focus.clone(), cx);
            }
            ListId::KeybindingsTable | ListId::EditorTabs { .. } => unreachable!("not a tree"),
        }
        cx.notify();
    }

    pub(super) fn tree_depths(&self, list: ListId) -> Vec<usize> {
        match list {
            ListId::Explorer => self.folder.as_ref().map(|tree| tree.rows().iter().map(|row| row.depth).collect()).unwrap_or_default(),
            ListId::Search => self.search.rows(&self.search_folders(), Some(&self.collapse_results())).iter().map(|row| row.depth).collect(),
            ListId::KeybindingsTable | ListId::EditorTabs { .. } => Vec::new(),
        }
    }

    /// How far the 100ms opacity fade that follows the pointer over the list has gone: 1 over it, 0 away.
    fn list_hover_fade(&self, list: ListId, duration: Duration, window: &Window) -> f32 {
        let Some((over, since)) = self.list_hover(list) else { return 0. };
        // tree.css: the fade is a `.monaco-enable-motion` transition.
        let progress = if self.is_motion_reduced() { 1. } else { (since.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.) };
        if progress < 1. {
            window.request_animation_frame();
        }
        if over { progress } else { 1. - progress }
    }
}
