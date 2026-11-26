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
use super::layout::{Alignment, Arrangement, Grid, Part, PartSizes, Position, Sash, card_margins};
use crate::commands::Outcome;
use crate::split_view::SashState;

/// The title bar and status bar heights around the grid's middle row.
const TITLE_BAR_HEIGHT: f32 = 35.;
const STATUS_BAR_HEIGHT: f32 = 28.;
/// `workbench.sash.hoverDelay` and `workbench.sash.size` defaults.
const SASH_HOVER_DELAY: f64 = 300.;
const SASH_SIZE: f32 = 4.;
/// `LayoutStateModel.STORAGE_PREFIX`.
const STORAGE_PREFIX: &str = "workbench.";
/// `LayoutStateKeys` of the panel's place: position in workspace storage, alignment in profile storage.
const PANEL_POSITION_KEY: &str = "workbench.panel.position";
const PANEL_ALIGNMENT_KEY: &str = "workbench.panel.alignment";
/// `PANEL_LAST_NON_MAXIMIZED_HEIGHT` and `_WIDTH` (profile, default 300) and `PANEL_WAS_LAST_MAXIMIZED` (workspace).
const PANEL_LAST_HEIGHT_KEY: &str = "workbench.panel.lastNonMaximizedHeight";
const PANEL_LAST_WIDTH_KEY: &str = "workbench.panel.lastNonMaximizedWidth";
const PANEL_WAS_LAST_MAXIMIZED_KEY: &str = "workbench.panel.wasLastMaximized";

#[derive(Default)]
pub(super) struct PartGrid {
    /// Laid out on the first render, when the window size the default part sizes come from is known.
    pub(super) grid: Option<Grid>,
    hovered: Option<Sash>,
    hover: Option<(Sash, Task<()>)>,
    /// `SASH_GRIPPERS_HIDDEN_ATTRIBUTE`: the grippers receded after `workbench.sash.autoHideGrippersDelay` without
    /// a mouse move; the timer and the last pointer position.
    grippers_hidden: bool,
    grippers_timer: Option<Task<()>>,
    last_pointer: Option<gpui::Point<Pixels>>,
}

/// `LayoutStateKeys`' visibility key of a part and its default; the side bar defaults to hidden in an empty window.
fn hidden_key(part: Part) -> &'static str {
    match part {
        Part::ActivityBar => "activityBar.hidden",
        Part::SideBar => "sideBar.hidden",
        Part::Editor => "editor.hidden",
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
        self.register_panel_actions();
        for (command, location) in [
            ("workbench.action.activityBarLocation.default", "default"),
            ("workbench.action.activityBarLocation.top", "top"),
            ("workbench.action.activityBarLocation.bottom", "bottom"),
            ("workbench.action.activityBarLocation.hide", "hidden"),
        ] {
            self.commands.register(command, move |workbench, _, _, cx| {
                workbench.update_setting("workbench.activityBar.location", json!(location));
                cx.notify();
                Outcome::done()
            });
        }
        // layoutActions.ts `MoveSidebarPositionAction` and `ToggleSidebarPositionAction` write `workbench.sideBar.location`.
        for (command, position) in [("workbench.action.moveSideBarRight", Some("right")), ("workbench.action.moveSideBarLeft", Some("left")), ("workbench.action.toggleSidebarPosition", None)] {
            self.commands.register(command, move |workbench, _, _, cx| {
                let toggled = if workbench.arrangement().side_bar == Position::Left { "right" } else { "left" };
                workbench.update_setting("workbench.sideBar.location", json!(position.unwrap_or(toggled)));
                cx.notify();
                Outcome::done()
            });
        }
        // panelActions.ts `PositionPanelActionConfigs` and `AlignPanelActionConfigs`.
        for (command, position) in [
            ("workbench.action.positionPanelTop", Position::Top),
            ("workbench.action.positionPanelLeft", Position::Left),
            ("workbench.action.positionPanelRight", Position::Right),
            ("workbench.action.positionPanelBottom", Position::Bottom),
        ] {
            self.commands.register(command, move |workbench, _, _, cx| {
                workbench.set_panel_position(position, cx);
                Outcome::done()
            });
        }
        for (command, alignment) in [
            ("workbench.action.alignPanelLeft", Alignment::Left),
            ("workbench.action.alignPanelRight", Alignment::Right),
            ("workbench.action.alignPanelCenter", Alignment::Center),
            ("workbench.action.alignPanelJustify", Alignment::Justify),
        ] {
            self.commands.register(command, move |workbench, _, _, cx| {
                workbench.set_panel_alignment(alignment, cx);
                Outcome::done()
            });
        }
    }

    /// `isVisible(part)`. The activity bar follows `workbench.activityBar.location` (`isActivityBarHidden`); the port
    /// draws only the default location, so `top` and `bottom` hide it as upstream does from the grid.
    pub(super) fn is_part_visible(&self, part: Part) -> bool {
        if part == Part::ActivityBar {
            return !self.zen_mode_hides_activity_bar() && self.setting("workbench.activityBar.location").as_str().is_none_or(|location| location == "default");
        }
        match &self.part_grid.grid {
            Some(grid) => grid.is_visible(part),
            None => !self.stored_part_hidden(part),
        }
    }

    pub(super) fn layout_context(&self, context: &mut HashMap<String, Value>) {
        let arrangement = self.arrangement();
        context.insert("sideBarVisible".into(), json!(self.is_part_visible(Part::SideBar)));
        context.insert("panelVisible".into(), json!(self.is_part_visible(Part::Panel)));
        context.insert("panelMaximized".into(), json!(self.is_panel_maximized()));
        context.insert("panelPosition".into(), json!(arrangement.panel.name()));
        context.insert("panelAlignment".into(), json!(arrangement.alignment.name()));
        context.insert("auxiliaryBarVisible".into(), json!(self.is_part_visible(Part::AuxiliaryBar)));
        context.insert("mainEditorAreaVisible".into(), json!(self.is_part_visible(Part::Editor)));
        context.insert("inZenMode".into(), json!(self.zen_mode.active));
        context.insert("isCenteredLayout".into(), json!(self.is_main_editor_centered()));
    }

    /// The parts' places: the grid's, or the stored ones before it is laid out.
    pub(super) fn arrangement(&self) -> Arrangement {
        match &self.part_grid.grid {
            Some(grid) => grid.arrangement(),
            None => self.stored_arrangement(),
        }
    }

    /// `LayoutStateModel.load`: the side bar's side from `workbench.sideBar.location`, the panel's position from
    /// workspace storage with `workbench.panel.defaultLocation` as its default, its alignment from profile storage.
    fn stored_arrangement(&self) -> Arrangement {
        let side_bar = self.setting("workbench.sideBar.location").as_str().and_then(Position::from_name).filter(|position| !position.is_horizontal()).unwrap_or(Position::Left);
        let stored_panel = self.workspace_storage.lock().unwrap().get(PANEL_POSITION_KEY).and_then(|value| value.parse::<i64>().ok()).and_then(Position::from_number);
        let panel = stored_panel.or_else(|| self.setting("workbench.panel.defaultLocation").as_str().and_then(Position::from_name)).unwrap_or(Position::Bottom);
        let alignment = self.profile_storage.lock().unwrap().get(PANEL_ALIGNMENT_KEY).as_deref().and_then(Alignment::from_name).unwrap_or(Alignment::Center);
        Arrangement { side_bar, panel, alignment }
    }

    /// `setPartHidden`. A shown panel opens maximized per `workbench.panel.opensMaximized` (`panelOpensMaximized`); a
    /// hidden one gives the editor back first; the editor and the panel never hide together (`setEditorHidden`).
    pub(super) fn set_part_hidden(&mut self, part: Part, hidden: bool, cx: &mut Context<Self>) {
        let was_hidden = !self.is_part_visible(part);
        if part == Part::Panel && hidden && self.is_panel_maximized() {
            self.toggle_maximized_panel(cx);
        }
        if let Some(grid) = self.part_grid.grid.as_mut() {
            grid.set_visible(part, !hidden);
        }
        self.workspace_storage.lock().unwrap().set(&format!("{STORAGE_PREFIX}{}", hidden_key(part)), hidden.to_string());
        if part == Part::Panel && !hidden && was_hidden && self.panel_opens_maximized() && !self.is_panel_maximized() {
            self.toggle_maximized_panel(cx);
        }
        if part == Part::Editor && hidden && !self.is_part_visible(Part::Panel) {
            self.set_part_hidden(Part::Panel, false, cx);
        }
        cx.notify();
    }

    /// `isPanelMaximized`: the editor hidden while the panel is centered or on a side.
    pub(super) fn is_panel_maximized(&self) -> bool {
        let arrangement = self.arrangement();
        (arrangement.alignment == Alignment::Center || !arrangement.panel.is_horizontal()) && !self.is_part_visible(Part::Editor)
    }

    fn panel_opens_maximized(&self) -> bool {
        let arrangement = self.arrangement();
        if arrangement.alignment != Alignment::Center && arrangement.panel.is_horizontal() {
            return false;
        }
        match self.setting("workbench.panel.opensMaximized").as_str() {
            Some("always") => true,
            Some("never") => false,
            _ => self.workspace_storage.lock().unwrap().get(PANEL_WAS_LAST_MAXIMIZED_KEY).is_some_and(|value| value == "true"),
        }
    }

    /// `toggleMaximizedPanel`: hides the editor, keeping the panel's size to come back at.
    pub(super) fn toggle_maximized_panel(&mut self, cx: &mut Context<Self>) {
        let Some(grid) = self.part_grid.grid.as_ref() else { return };
        let horizontal = grid.arrangement().panel.is_horizontal();
        let panel_size = grid.is_visible(Part::Panel).then(|| grid.size(Part::Panel));
        let maximize = !self.is_panel_maximized();
        if maximize {
            if let Some(size) = panel_size {
                let key = if horizontal { PANEL_LAST_HEIGHT_KEY } else { PANEL_LAST_WIDTH_KEY };
                self.profile_storage.lock().unwrap().set(key, (size.round() as i64).to_string());
            }
            self.part_grid.grid.as_mut().expect("checked above").set_visible(Part::Editor, false);
        } else {
            let size = self.stored_panel_size(horizontal);
            let grid = self.part_grid.grid.as_mut().expect("checked above");
            grid.set_visible(Part::Editor, true);
            grid.resize(Part::Panel, size);
        }
        let mut workspace = self.workspace_storage.lock().unwrap();
        workspace.set(PANEL_WAS_LAST_MAXIMIZED_KEY, maximize.to_string());
        workspace.set(&format!("{STORAGE_PREFIX}{}", hidden_key(Part::Editor)), maximize.to_string());
        drop(workspace);
        cx.notify();
    }

    /// `PANEL_LAST_NON_MAXIMIZED_HEIGHT` or `_WIDTH`.
    fn stored_panel_size(&self, horizontal: bool) -> f32 {
        let key = if horizontal { PANEL_LAST_HEIGHT_KEY } else { PANEL_LAST_WIDTH_KEY };
        self.profile_storage.lock().unwrap().get(key).and_then(|value| value.parse::<f32>().ok()).unwrap_or(300.)
    }

    /// `setPanelPosition`: the panel shows, keeps its size for the other direction and moves next to the editor.
    pub(super) fn set_panel_position(&mut self, position: Position, cx: &mut Context<Self>) {
        if !self.is_part_visible(Part::Panel) {
            self.set_part_hidden(Part::Panel, false, cx);
        }
        let Some(grid) = self.part_grid.grid.as_ref() else {
            self.workspace_storage.lock().unwrap().set(PANEL_POSITION_KEY, (position as i64).to_string());
            return cx.notify();
        };
        let mut arrangement = grid.arrangement();
        let [_, _, width, height] = grid.part_bounds(Part::Panel).expect("shown above");
        let mut editor_hidden = !grid.is_visible(Part::Editor);
        if position != arrangement.panel && !editor_hidden {
            if position.is_horizontal() {
                self.profile_storage.lock().unwrap().set(PANEL_LAST_WIDTH_KEY, (width.round() as i64).to_string());
            } else if arrangement.panel.is_horizontal() {
                self.profile_storage.lock().unwrap().set(PANEL_LAST_HEIGHT_KEY, (height.round() as i64).to_string());
            }
        }
        if position.is_horizontal() && arrangement.alignment != Alignment::Center && editor_hidden {
            self.toggle_maximized_panel(cx);
            editor_hidden = false;
        }
        self.workspace_storage.lock().unwrap().set(PANEL_POSITION_KEY, (position as i64).to_string());
        let panel_size = match (editor_hidden, position.is_horizontal()) {
            (true, true) => height,
            (true, false) => width,
            (false, horizontal) => self.stored_panel_size(horizontal),
        };
        arrangement.panel = position;
        self.rearrange_parts(arrangement, panel_size);
        cx.notify();
    }

    /// `setPanelAlignment`: only a top or bottom panel aligns, and only a centered one stays maximized.
    pub(super) fn set_panel_alignment(&mut self, alignment: Alignment, cx: &mut Context<Self>) {
        if !self.arrangement().panel.is_horizontal() {
            self.set_panel_position(Position::Bottom, cx);
        }
        if alignment != Alignment::Center && self.is_panel_maximized() {
            self.toggle_maximized_panel(cx);
        }
        self.profile_storage.lock().unwrap().set(PANEL_ALIGNMENT_KEY, alignment.name().to_string());
        if let Some(grid) = self.part_grid.grid.as_ref() {
            let arrangement = Arrangement { alignment, ..grid.arrangement() };
            let panel_size = grid.size(Part::Panel);
            self.rearrange_parts(arrangement, panel_size);
        }
        cx.notify();
    }

    /// `adjustPartPositions`: the grid again with the parts where `arrangement` puts them, at their sizes.
    fn rearrange_parts(&mut self, arrangement: Arrangement, panel_size: f32) {
        let grid = self.part_grid.grid.as_ref().expect("a laid out grid");
        let [width, height] = grid.dimensions();
        let sizes = PartSizes { side_bar: grid.size(Part::SideBar), panel: panel_size, auxiliary_bar: grid.size(Part::AuxiliaryBar) };
        let visible: Vec<Part> = [Part::ActivityBar, Part::SideBar, Part::Editor, Part::Panel, Part::AuxiliaryBar].into_iter().filter(|&part| grid.is_visible(part)).collect();
        self.part_grid.grid = Some(Grid::new(width, height, sizes, arrangement, |part| visible.contains(&part)));
        self.part_grid.hovered = None;
        self.part_grid.hover = None;
    }

    /// `setSideBarHidden(true)`: the hidden view's focus moves to the editor (`focusPanelOrEditor`).
    fn hide_side_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_part_hidden(Part::SideBar, true, cx);
        window.focus(&self.editor_focus.clone(), cx);
    }

    /// contrib/sash/browser/sash.ts: `workbench.sash.size` clamped to 4..20 is every sash's size (`setGlobalSashSize`).
    pub(super) fn sash_size(&self) -> f32 {
        self.setting("workbench.sash.size").as_f64().map_or(SASH_SIZE, |size| (size as f32).clamp(4., 20.))
    }

    /// The sash highlight's width, `--vscode-sash-hover-size`: the setting clamped to 1..8.
    pub(super) fn sash_hover_size(&self) -> f32 {
        self.setting("workbench.sash.size").as_f64().map_or(SASH_SIZE, |size| (size as f32).clamp(1., 8.))
    }

    /// `workbench.sash.hoverDelay` (0..2000 in its schema) before a hovered sash shows its border (`setGlobalHoverDelay`).
    pub(super) fn sash_hover_delay(&self) -> Duration {
        Duration::from_millis(self.setting("workbench.sash.hoverDelay").as_f64().map_or(SASH_HOVER_DELAY, |delay| delay.clamp(0., 2000.)) as u64)
    }

    /// Forgets the laid out grid so the next render reads the folder's stored layout.
    pub(super) fn reset_part_grid(&mut self) {
        self.part_grid.grid = None;
    }

    fn stored_part_hidden(&self, part: Part) -> bool {
        let stored = self.workspace_storage.lock().unwrap().get(&format!("{STORAGE_PREFIX}{}", hidden_key(part))).map(|value| value == "true");
        stored.unwrap_or(match part {
            Part::SideBar => self.folder.is_none(),
            Part::ActivityBar | Part::Editor => false,
            Part::Panel | Part::AuxiliaryBar => true,
        })
    }

    /// The `InitializationStateKey` sizes in profile storage, upstream's `parseInt` of the stored string.
    fn stored_part_sizes(&self, window_width: f32, window_height: f32, panel: Position) -> PartSizes {
        let storage = self.profile_storage.lock().unwrap();
        let stored = |key: &str, default: f32| storage.get(&format!("{STORAGE_PREFIX}{key}")).and_then(|value| value.parse::<f32>().ok()).unwrap_or(default);
        let defaults = PartSizes::defaults(window_width, window_height, panel);
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

    /// The grid's middle row for a window of `viewport`: every part where the grid puts it, inside its card margins,
    /// and the sashes. A changed `workbench.sideBar.location` moves the parts (`onDidChangeConfiguration`).
    pub(super) fn middle_section(&mut self, viewport: Size<Pixels>, editor: Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let status_bar_height = if self.is_status_bar_visible() { STATUS_BAR_HEIGHT } else { 0. };
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height) - TITLE_BAR_HEIGHT - self.banner_height() - status_bar_height);
        let activity_bar_visible = self.is_part_visible(Part::ActivityBar);
        let stored = self.stored_arrangement();
        if self.part_grid.grid.is_none() {
            let sizes = self.stored_part_sizes(width, f32::from(viewport.height), stored.panel);
            let hidden: Vec<Part> = [Part::SideBar, Part::Editor, Part::Panel, Part::AuxiliaryBar].into_iter().filter(|&part| self.stored_part_hidden(part)).collect();
            self.part_grid.grid = Some(Grid::new(width, height, sizes, stored, |part| if part == Part::ActivityBar { activity_bar_visible } else { !hidden.contains(&part) }));
            self.restore_zen_mode(window, cx);
        }
        let grid = self.part_grid.grid.as_ref().expect("laid out above");
        if grid.arrangement().side_bar != stored.side_bar {
            let arrangement = Arrangement { side_bar: stored.side_bar, ..grid.arrangement() };
            let panel_size = grid.size(Part::Panel);
            self.rearrange_parts(arrangement, panel_size);
        }
        let activity_bar_visible = self.is_part_visible(Part::ActivityBar);
        let grid = self.part_grid.grid.as_mut().expect("laid out above");
        grid.layout(width, height);
        grid.set_visible(Part::ActivityBar, activity_bar_visible);
        let arrangement = grid.arrangement();
        let parts = grid.bounds();
        let sashes = grid.sashes();
        let dragging = grid.dragged_sash().is_some();
        if self.is_part_visible(Part::SideBar) {
            self.layout_explorer_panes(parts.iter().find(|(part, _)| *part == Part::SideBar).map_or(height, |(_, [_, _, _, h])| *h) - super::layout::CARD_MARGIN, cx);
        }
        self.sync_tree_views(cx);
        let mut editor = Some(editor);
        let mut section = div().relative().h(px(height)).flex_none();
        for &(part, rect) in &parts {
            let [left, top, right, bottom] = card_margins(part, rect, &parts, arrangement, width, height, false);
            let content = match part {
                Part::ActivityBar => self.activity_bar(window, cx),
                Part::SideBar => self.side_bar(window, cx),
                Part::Editor => editor.take().expect("one editor part"),
                Part::Panel => self.panel_part(window, cx),
                Part::AuxiliaryBar => self.auxiliary_bar_part(cx),
            };
            let [x, y, w, h] = rect;
            section = section.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).flex().pl(px(left)).pt(px(top)).pr(px(right)).pb(px(bottom)).child(content));
        }
        let section = section
            .children(sashes.into_iter().map(|(sash, state, bounds)| self.part_sash(sash, state, bounds, cx)))
            .when(dragging, |section| section.child(self.part_sash_drag(cx)))
            .child(self.gripper_activity(cx));
        // The labels drawn above asked for the decorations they lack.
        self.send_decoration_requests(cx);
        section
    }

    /// contrib/sash `SashGrippersController`: with `workbench.sash.autoHideGrippers`, the grippers show on mouse
    /// movement and recede after the delay; without it they always show.
    fn gripper_activity(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let auto_hide = self.setting("workbench.sash.autoHideGrippers") != json!(false);
        if !auto_hide {
            self.part_grid.grippers_hidden = false;
            self.part_grid.grippers_timer = None;
        } else if self.part_grid.grippers_timer.is_none() && !self.part_grid.grippers_hidden {
            self.show_grippers(cx);
        }
        let workbench = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                if !auto_hide {
                    return;
                }
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != gpui::DispatchPhase::Capture {
                        return;
                    }
                    workbench.update(cx, |workbench, cx| {
                        if workbench.part_grid.last_pointer != Some(event.position) {
                            workbench.part_grid.last_pointer = Some(event.position);
                            workbench.show_grippers(cx);
                        }
                    });
                });
            },
        )
        .size_0()
    }

    fn show_grippers(&mut self, cx: &mut Context<Self>) {
        let delay = self.setting("workbench.sash.autoHideGrippersDelay").as_f64().unwrap_or(2000.).max(0.);
        if self.part_grid.grippers_hidden {
            self.part_grid.grippers_hidden = false;
            cx.notify();
        }
        self.part_grid.grippers_timer = Some(cx.spawn(async move |workbench, cx| {
            cx.background_executor().timer(Duration::from_millis(delay as u64)).await;
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.part_grid.grippers_hidden = true;
                cx.notify();
            });
        }));
    }

    /// partGrid.css `.part-grid-gap-sash::after`: three 2px dots 4px apart across the sash's middle, hidden while
    /// the sash highlights or the grippers receded.
    fn sash_gripper(&self, vertical: bool, width: f32, height: f32) -> Div {
        let color = self.color("modernSash.gripForeground");
        let (cx, cy) = (width / 2., height / 2.);
        let dot = |dx: f32, dy: f32| div().absolute().left(px(cx + dx - 1.)).top(px(cy + dy - 1.)).size(px(2.)).rounded_full().bg(color);
        let offsets = [-4., 0., 4.];
        div().absolute().inset_0().children(offsets.map(|offset| if vertical { dot(0., offset) } else { dot(offset, 0.) }))
    }

    /// A `.monaco-sash` of the grid (sash.css): mac cursors, `sash.hoverBorder` after the hover delay and while dragged.
    fn part_sash(&self, sash: Sash, state: SashState, [x, y, width, height]: [f32; 4], cx: &mut Context<Self>) -> AnyElement {
        if state == SashState::Disabled {
            return div().into_any_element();
        }
        let vertical = sash.vertical;
        let id = ("part-sash", sash.branch * 16 + sash.index);
        let grid = self.part_grid.grid.as_ref().expect("sashes are drawn with the grid");
        let active = self.part_grid.hovered == Some(sash) || grid.dragged_sash() == Some(sash);
        let hover_border = self.theme.color("sash.hoverBorder").unwrap_or(self.color("focusBorder"));
        // floatingPanels.css: a vertical sash's highlight stops `--modern-ui-floating-card-statusbar-margin` above the status bar.
        let highlight_inset = if vertical { super::layout::CARD_MARGIN } else { 0. };
        let element = div()
            .id(id)
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .when(!active && !self.part_grid.grippers_hidden, |element| element.child(self.sash_gripper(vertical, width, height)))
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
                let delay = workbench.sash_hover_delay();
                workbench.part_grid.hover = Some((sash, cx.spawn_in(window, async move |workbench, cx| {
                        cx.background_executor().timer(delay).await;
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
        let vertical = self.part_grid.grid.as_ref().and_then(Grid::dragged_sash).is_some_and(|sash| sash.vertical);
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
