//! Editor windows (auxiliaryEditorPart.ts, auxiliaryWindowService.ts, editorParts.ts, editorActions.ts
//! `BaseMoveCopyEditorToNewWindowAction` and its kin): an editor part of its own in a window of its own, with a title
//! bar and the editor's status bar items. The window's workbench keeps every part's groups (editor_groups.rs
//! `PartLayout`) and draws the part into the window; closing the window moves the editors that would ask before
//! closing to the main window and closes the others.
//!
//! Drag and drop between windows: AppKit sends a drag's `mouseDragged` and `mouseUp` only to the window where the
//! press was, so GPUI's drop targets in other windows never see them. While a tab drags past its window's edge,
//! `cross_window_drag` hands each move and the release to the app window under the pointer (hit-tested against the
//! windows' screen bounds, front to back) as that window's own event; GPUI's drag is app-wide, so the drop targets
//! there take it as a drag of their own. A release over no window of the app opens the tab in a new window there.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, DispatchPhase, Entity, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    PlatformInput, Point, Render, Rgba, Styled, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, canvas, div, point, prelude::*, px, size,
};
use serde_json::{Value, json};

use super::Workbench;
use super::editor_groups::{MAIN_PART, PartLayout};
use super::hover::HoverOptions;
use crate::commands::Outcome;

/// window.ts `DEFAULT_AUX_WINDOW_SIZE`.
const DEFAULT_WIDTH: f32 = 1024.;
const DEFAULT_HEIGHT: f32 = 768.;

#[derive(Default)]
pub(super) struct AuxiliaryWindows {
    /// In the order the parts opened.
    windows: Vec<AuxiliaryWindowState>,
    last_part: u32,
    /// The part of the workbench window that was active last, `None` for the main window: overlays such as the quick
    /// input show there (layoutService.ts `activeContainer`).
    active: Option<u32>,
    /// Parts whose last group went: their windows close once a context is at hand.
    closing: Vec<WindowHandle<AuxiliaryWindow>>,
    /// The window a tab dragged out of its window is over, which hears the drag leave.
    drag_over: Option<AnyWindowHandle>,
    /// The workbench's own window, as it last drew.
    pub(super) main_window: Option<AnyWindowHandle>,
}

struct AuxiliaryWindowState {
    part: u32,
    /// `None` until the window opens, after the update that made the part.
    handle: Option<WindowHandle<AuxiliaryWindow>>,
    /// The window's screen bounds as last drawn, for the parts state.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Compact mode (auxiliaryEditorPart.ts `updateCompact`) and the part's active editor it last saw.
    compact: Option<Option<u32>>,
}

/// What the title bar of an editor window shows of the window's state.
#[derive(Clone, Copy)]
pub(super) struct EditorWindowTitle {
    pub(super) always_on_top: bool,
    pub(super) compact: bool,
}

impl AuxiliaryWindows {
    pub(super) fn part_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.windows.iter().map(|window| window.part)
    }

    pub(super) fn active(&self) -> Option<u32> {
        self.active
    }

    fn part_of_window(&self, handle: AnyWindowHandle) -> Option<u32> {
        self.windows.iter().find(|window| window.handle.is_some_and(|own| AnyWindowHandle::from(own) == handle)).map(|window| window.part)
    }
}

/// The root view of an editor window: its workbench draws it.
pub(crate) struct AuxiliaryWindow {
    workbench: WeakEntity<Workbench>,
    part: u32,
    _activation: Subscription,
}

impl Render for AuxiliaryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let part = self.part;
        match self.workbench.upgrade() {
            Some(workbench) => workbench.update(cx, |workbench, cx| workbench.auxiliary_window_contents(part, window, cx)),
            None => div().into_any_element(),
        }
    }
}

/// The workbench of a window: its root, or the one that drew an editor window.
pub(crate) fn window_workbench(window: &Window, cx: &App) -> Option<Entity<Workbench>> {
    if let Some(Some(workbench)) = window.root::<Workbench>() {
        return Some(workbench);
    }
    let Some(Some(auxiliary)) = window.root::<AuxiliaryWindow>() else { return None };
    auxiliary.read(cx).workbench.upgrade()
}

/// What goes into the new window.
#[derive(Clone, Copy, PartialEq)]
enum Transfer {
    MoveEditor,
    CopyEditor,
    MoveGroup,
    CopyGroup,
    Nothing,
}

impl Workbench {
    pub(super) fn register_auxiliary_window_commands(&mut self) {
        let transfers = [
            ("workbench.action.moveEditorToNewWindow", Transfer::MoveEditor),
            ("workbench.action.copyEditorToNewWindow", Transfer::CopyEditor),
            ("workbench.action.moveEditorGroupToNewWindow", Transfer::MoveGroup),
            ("workbench.action.copyEditorGroupToNewWindow", Transfer::CopyGroup),
            ("workbench.action.newEmptyEditorWindow", Transfer::Nothing),
        ];
        for (id, transfer) in transfers {
            self.commands.register(id, move |workbench, args, window, cx| {
                workbench.leave_panel_group();
                // `resolveCommandsContext`: the tab menu's editor is the one that goes.
                if let Some(index) = workbench.context_editor(&args).filter(|_| matches!(transfer, Transfer::MoveEditor | Transfer::CopyEditor)) {
                    workbench.editors.activate(index);
                }
                let bounds = new_window_bounds(window.bounds(), None);
                workbench.transfer_to_new_window(transfer, bounds, cx);
                Outcome::done()
            });
        }
        // editorActions.ts `RestoreEditorsToMainWindowAction`: the window's groups merge into the main part's active group.
        self.commands.register("workbench.action.restoreEditorsToMainWindow", |workbench, _, window, cx| {
            let Some(part) = workbench.auxiliary_part_of_window(window) else { return Outcome::done() };
            let target = workbench.part_active_group(MAIN_PART).expect("the main part has a group");
            for group in workbench.part_groups(part) {
                workbench.merge_group(group, target, window, cx);
            }
            Outcome::done()
        });
        // auxiliaryEditorPart.ts: Toggle Window Compact Mode and the title bar's Turn On/Off Compact Mode.
        let compacts = [
            ("workbench.action.toggleCompactAuxiliaryWindow", None),
            ("workbench.action.enableCompactAuxiliaryWindow", Some(true)),
            ("workbench.action.disableCompactAuxiliaryWindow", Some(false)),
        ];
        for (id, compact) in compacts {
            self.commands.register(id, move |workbench, _, window, cx| {
                let Some(part) = workbench.auxiliary_part_of_window(window) else { return Outcome::done() };
                let compact = compact.unwrap_or(!workbench.is_compact_part(part));
                workbench.set_compact(part, compact);
                cx.notify();
                Outcome::done()
            });
        }
    }

    pub(super) fn is_compact_part(&self, part: u32) -> bool {
        self.auxiliary_windows.windows.iter().any(|window| window.part == part && window.compact.is_some())
    }

    /// The group being drawn is in a compact editor window: no tabs and no breadcrumbs (`showTabs: none`,
    /// `showBreadcrumbs: false`).
    pub(super) fn drawing_compact_part(&self) -> bool {
        self.part_of_group(self.editors.id).is_some_and(|part| self.is_compact_part(part))
    }

    fn set_compact(&mut self, part: u32, compact: bool) {
        let active_editor = self.part_active_group(part).and_then(|group| self.editors.group_active_tab(group)).map(|editor| editor.model_id);
        let Some(state) = self.auxiliary_windows.windows.iter_mut().find(|window| window.part == part) else { return };
        state.compact = compact.then_some(active_editor);
    }

    /// auxiliaryEditorPart.ts: a compact window leaves compact mode when its part gets a second group, or its group
    /// shows another editor while it has more than one.
    pub(super) fn leave_compact_mode(&mut self) {
        let compact: Vec<(u32, Option<u32>)> = self.auxiliary_windows.windows.iter().filter_map(|window| Some((window.part, window.compact?))).collect();
        for (part, seen) in compact {
            let groups = self.part_groups(part);
            let (editors, active) = self.editors.group_tabs(groups[0]);
            let active = active.map(|index| editors[index].model_id);
            let leave = groups.len() > 1 || (active != seen && editors.len() > 1);
            let state = self.auxiliary_windows.windows.iter_mut().find(|window| window.part == part).expect("listed above");
            state.compact = (!leave).then_some(active);
        }
    }

    /// `transferToNewWindow`: a new part with one group in a new window, the editor or the group moved or copied
    /// into it; the window takes the focus.
    fn transfer_to_new_window(&mut self, transfer: Transfer, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let source = self.editors.id;
        if matches!(transfer, Transfer::MoveEditor | Transfer::CopyEditor) && self.editors.active().is_none() {
            return;
        }
        let copies: Vec<_> = match transfer {
            Transfer::CopyEditor => self.editors.copy_active().into_iter().collect(),
            Transfer::CopyGroup => self.copy_group_editors(),
            _ => Vec::new(),
        };
        let moved: Vec<_> = match transfer {
            Transfer::MoveEditor => vec![self.editors.take_editor(self.editors.active_index().expect("checked above"))],
            Transfer::MoveGroup => (0..self.editors.editors.len()).map(|_| self.editors.take_editor(0)).collect(),
            _ => Vec::new(),
        };
        let active = self.editors.active_index();
        let group = self.create_auxiliary_part(bounds, cx);
        self.activate_group(group);
        for editor in copies.into_iter().chain(moved) {
            self.insert_moved_editor(editor);
        }
        if let (Transfer::CopyGroup | Transfer::MoveGroup, Some(active)) = (transfer, active) {
            self.editors.activate(active.min(self.editors.editors.len().saturating_sub(1)));
        }
        // `mergeGroup` removes the moved group unless it is its part's last.
        let several = self.part_of_group(source).is_some_and(|part| self.part_groups(part).len() > 1);
        if transfer == Transfer::MoveGroup && several {
            self.remove_group(source);
        }
        self.close_group_if_empty(source);
        self.editors_changed(cx);
    }

    /// Copies of the active group's editors in tab order, its active editor's copy first.
    fn copy_group_editors(&mut self) -> Vec<crate::editor::OpenEditor> {
        let Some(active) = self.editors.active_index() else { return Vec::new() };
        let mut copies = Vec::new();
        for index in 0..self.editors.editors.len() {
            self.editors.activate(index);
            copies.extend(self.editors.copy_active());
        }
        self.editors.activate(active);
        copies
    }

    /// editorParts.ts `createAuxiliaryEditorPart`: the part and its one group now, its window after this update
    /// (the window's first frame reads the workbench).
    fn create_auxiliary_part(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) -> u32 {
        self.auxiliary_windows.last_part += 1;
        let part = self.auxiliary_windows.last_part;
        let group = self.editors.add_group(false);
        let layout = PartLayout::auxiliary(group, f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.editor_groups.parked.push((part, layout));
        self.auxiliary_windows.windows.push(AuxiliaryWindowState { part, handle: None, bounds: Rc::new(Cell::new(bounds)), compact: None });
        self.auxiliary_windows.active = Some(part);
        self.open_auxiliary_window(part, bounds, cx);
        group
    }

    fn open_auxiliary_window(&mut self, part: u32, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let workbench = cx.entity().downgrade();
        let focus = self.editor_focus.clone();
        cx.defer(move |cx| {
            let titlebar = TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(point(px(10.), px(10.))) };
            // window.ts `WindowMinimumSize`.
            let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(titlebar), app_owns_titlebar_drag: true, window_min_size: Some(size(px(400.), px(270.))), ..Default::default() };
            let opened = cx.open_window(options, |window, cx| {
                let closing = workbench.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    let _ = closing.update(cx, |workbench, cx| {
                        // The window is closing: nothing closes it again.
                        if let Some(state) = workbench.auxiliary_windows.windows.iter_mut().find(|window| window.part == part) {
                            state.handle = None;
                        }
                        workbench.close_auxiliary_window(part, cx);
                    });
                    true
                });
                let activated = workbench.clone();
                cx.new(|cx| AuxiliaryWindow {
                    workbench: workbench.clone(),
                    part,
                    _activation: cx.observe_window_activation(window, move |_, window, cx| {
                        if window.is_window_active() {
                            let _ = activated.update(cx, |workbench, cx| workbench.window_activated(Some(part), cx));
                        }
                    }),
                })
            });
            let handle = match opened {
                Ok(handle) => handle,
                Err(error) => return eprintln!("[auxiliaryWindow] {error:#}"),
            };
            let opened = workbench.update(cx, |workbench, _| match workbench.auxiliary_windows.windows.iter_mut().find(|window| window.part == part) {
                Some(state) => state.handle = Some(handle),
                // The part closed before its window opened.
                None => workbench.auxiliary_windows.closing.push(handle),
            });
            if opened.is_ok() {
                let _ = handle.update(cx, |_, window, cx| window.focus(&focus, cx));
            }
        });
    }

    /// auxiliaryEditorPart.ts `close` (`doClose(true)`), when the window closes: its editors that would ask before
    /// closing (dirty ones) move to the main part's active group, the others close, and the part goes.
    pub(super) fn close_auxiliary_window(&mut self, part: u32, cx: &mut Context<Self>) {
        if !self.auxiliary_windows.windows.iter().any(|window| window.part == part) {
            return;
        }
        let target = self.part_active_group(MAIN_PART).expect("the main part has a group");
        for group in self.part_groups(part) {
            let dirty: Vec<_> = self.with_group(group, |workbench| {
                let dirty: Vec<usize> = (0..workbench.editors.editors.len()).filter(|index| workbench.editor_shows_dirty(&workbench.editors.editors[*index])).collect();
                dirty.into_iter().rev().map(|index| workbench.editors.take_editor(index)).collect()
            });
            if !dirty.is_empty() {
                self.with_group(target, |workbench| {
                    for editor in dirty.into_iter().rev() {
                        workbench.insert_moved_editor(editor);
                    }
                });
            }
            self.remove_group(group);
        }
        self.editors_changed(cx);
        self.save_editor_part_state();
        cx.notify();
    }

    /// The part's last group went (editor_groups.rs `remove_group`): its window closes, as `doRemoveLastGroup`
    /// closes it.
    pub(super) fn close_auxiliary_part(&mut self, part: u32) {
        // The active group is in no part (the panel's): the main part's layout takes the fields.
        if self.editor_groups.active_part() == part {
            self.editor_groups.switch_part(MAIN_PART);
        }
        self.editor_groups.parked.retain(|(id, _)| *id != part);
        let Some(index) = self.auxiliary_windows.windows.iter().position(|window| window.part == part) else { return };
        let state = self.auxiliary_windows.windows.remove(index);
        self.auxiliary_windows.closing.extend(state.handle);
        if self.auxiliary_windows.active == Some(part) {
            self.auxiliary_windows.active = None;
        }
    }

    /// Closes the windows of the parts that went.
    pub(super) fn close_auxiliary_windows_of_closed_parts(&mut self, cx: &mut Context<Self>) {
        for handle in std::mem::take(&mut self.auxiliary_windows.closing) {
            cx.defer(move |cx| drop(handle.update(cx, |_, window, _| window.remove_window())));
        }
    }

    /// The main window closes or reloads: its editor windows close with it and keep their state.
    pub(super) fn remove_auxiliary_windows(&mut self, cx: &mut Context<Self>) {
        self.save_editor_part_state();
        for state in std::mem::take(&mut self.auxiliary_windows.windows) {
            self.auxiliary_windows.closing.extend(state.handle);
        }
        self.close_auxiliary_windows_of_closed_parts(cx);
    }

    /// A window of this workbench became active: its part becomes the active one (editorParts.ts `activePart` is the
    /// part of the focused window), with that part's most recent group.
    pub(super) fn window_activated(&mut self, part: Option<u32>, cx: &mut Context<Self>) {
        if part.is_some_and(|part| !self.auxiliary_windows.windows.iter().any(|window| window.part == part)) {
            return;
        }
        self.auxiliary_windows.active = part;
        let part = part.unwrap_or(MAIN_PART);
        if self.part_of_group(self.editor_group_id()) != Some(part)
            && let Some(group) = self.part_active_group(part)
        {
            self.leave_panel_group();
            self.activate_group(group);
            self.editors_changed(cx);
        }
        cx.notify();
    }

    /// The window that shows `group`: its part's, the active part's for a group in no part; `None` before the
    /// window opened or drew.
    pub(super) fn group_window(&self, group: u32) -> Option<AnyWindowHandle> {
        match self.part_of_group(group).unwrap_or(self.editor_groups.active_part()) {
            MAIN_PART => self.auxiliary_windows.main_window,
            part => self.auxiliary_windows.windows.iter().find(|window| window.part == part)?.handle.map(AnyWindowHandle::from),
        }
    }

    /// The part of `window` when it is one of this workbench's editor windows.
    pub(super) fn auxiliary_part_of_window(&self, window: &Window) -> Option<u32> {
        self.auxiliary_windows.part_of_window(window.window_handle())
    }

    /// A part's most recently active group.
    pub(super) fn part_active_group(&self, part: u32) -> Option<u32> {
        self.editors.group_ids_mru().into_iter().find(|group| self.part_of_group(*group) == Some(part))
    }

    /// contextkeys.ts `IsAuxiliaryWindowFocusedContext`, and the auxiliary part's `IsAuxiliaryWindowContext`.
    pub(super) fn auxiliary_window_context(&self, window: &Window, context: &mut std::collections::HashMap<String, Value>) {
        let auxiliary = self.auxiliary_part_of_window(window).is_some();
        context.insert("isAuxiliaryWindowFocusedContext".into(), json!(auxiliary));
        context.insert("isAuxiliaryWindow".into(), json!(auxiliary));
    }

    /// An editor window: the title bar, the part's groups and the status bar with the part's editor status
    /// (auxiliaryEditorPart.ts `create`), and the overlays while it is the active window.
    fn auxiliary_window_contents(&mut self, part: u32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.auxiliary_windows.windows.iter().find(|state| state.part == part) else { return div().into_any_element() };
        state.bounds.set(window.bounds());
        let Some(group) = self.part_active_group(part) else { return div().into_any_element() };
        self.editors.sync_editors();
        let viewport = window.viewport_size();
        let active = window.is_window_active();
        let title = self.with_group(group, |workbench| workbench.window_title());
        window.set_window_title(&title);
        let always_on_top = crate::dialog::app_kit_window(window).is_some_and(|ns_window| ns_window.level() == objc2_app_kit::NSFloatingWindowLevel);
        let compact = self.is_compact_part(part);
        let editor_window = EditorWindowTitle { always_on_top, compact };
        let title_bar = self.with_group(group, |workbench| workbench.title_bar(f32::from(viewport.width), active, Some(editor_window), cx));
        let editor = self.in_part(part, |workbench| workbench.editor_groups_part(window, cx));
        let status_bar = self.with_group(group, |workbench| workbench.auxiliary_status_bar(window, cx));
        let overlays = match self.auxiliary_windows.active == Some(part) {
            true => self.window_overlays(window, cx),
            false => Vec::new(),
        };
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(self.color("editor.background"))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(title_bar)
            .child(editor)
            .when(self.is_status_bar_visible() && !compact, |root| root.child(status_bar))
            .children(overlays)
            .child(self.cross_window_drag(cx))
            .into_any_element()
    }

    /// An editor window's `MenuId.LayoutControlMenu` items: Turn On/Off Compact Mode (auxiliaryEditorPart.ts, order 0)
    /// and Turn On/Off Always on Top (windowActions.ts, order 1).
    pub(super) fn editor_window_title_actions(&self, state: EditorWindowTitle, foreground: Rgba, cx: &mut Context<Self>) -> [AnyElement; 2] {
        let compact = match state.compact {
            true => ("screen-normal", "Turn Off Compact Mode", "workbench.action.disableCompactAuxiliaryWindow"),
            false => ("screen-full", "Turn On Compact Mode", "workbench.action.enableCompactAuxiliaryWindow"),
        };
        let always_on_top = match state.always_on_top {
            true => ("pinned", "Turn Off Always on Top", "workbench.action.disableWindowAlwaysOnTop"),
            false => ("pin", "Turn On Always on Top", "workbench.action.enableWindowAlwaysOnTop"),
        };
        [compact, always_on_top].map(|(icon, title, command)| {
            self.icon(icon, 22., 16., foreground)
                .id(command)
                .cursor_pointer()
                .on_click(cx.listener(move |workbench, _, window, cx| drop(workbench.execute_command(command, Vec::new(), window, cx))))
                .child(self.action_hover(title, command, HoverOptions::instant()))
                .into_any_element()
        })
    }

    /// auxiliaryEditorPart.ts `statusbarPart`: the part's editor status items only.
    fn auxiliary_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let (foreground, background) = match self.folder {
            Some(_) => (self.color("statusBar.foreground"), self.color("statusBar.background")),
            None => (self.color("statusBar.noFolderForeground"), self.color("statusBar.noFolderBackground")),
        };
        div()
            .h(px(22.))
            .flex_none()
            .flex()
            .justify_end()
            .items_center()
            .px(px(12.))
            .bg(background)
            .text_color(foreground)
            .text_size(px(12.))
            .child(div().h(px(22.)).flex().child(self.editor_status_items(foreground, window, cx)))
    }

    /// The overlays of the active window (quick input, menus, dialogs, hovers): the main window's while it is the
    /// active one, else the editor window's.
    pub(super) fn window_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let quick_input = self.quick_input_widget(window, cx);
        let context_menu = self.context_menu_widget(cx);
        let dialog = self.custom_dialog_widget(window, cx);
        let action_widget = self.action_widget(window, cx);
        let hover = self.hover_widget(window, cx);
        let mut overlays: Vec<AnyElement> = Vec::new();
        overlays.extend(quick_input.map(|widget| div().absolute().top_0().left_0().child(widget).into_any_element()));
        overlays.extend(context_menu);
        overlays.extend(dialog);
        overlays.extend(action_widget);
        overlays.extend(hover);
        overlays
    }

    /// `editorparts.state`: each editor window's part and bounds, and the parts' MRU order (editorParts.ts
    /// `createState`); none without editor windows.
    pub(super) fn auxiliary_parts_state(&mut self) -> Option<Value> {
        if self.auxiliary_windows.windows.is_empty() {
            return None;
        }
        let parts: Vec<(u32, Bounds<Pixels>)> = self.auxiliary_windows.windows.iter().map(|window| (window.part, window.bounds.get())).collect();
        let mut auxiliary = Vec::new();
        for (part, bounds) in &parts {
            let state = self.in_part(*part, |workbench| workbench.part_state(*part));
            let value = |pixels: Pixels| f32::from(pixels).round() as i64;
            auxiliary.push(json!({
                "state": state,
                "bounds": { "x": value(bounds.origin.x), "y": value(bounds.origin.y), "width": value(bounds.size.width), "height": value(bounds.size.height) },
                "zoomLevel": 0,
                "compact": self.is_compact_part(*part),
            }));
        }
        // `mostRecentActiveParts` as indexes into the persistable parts, the main part 0.
        let mut mru: Vec<usize> = Vec::new();
        for group in self.editors.group_ids_mru() {
            let index = match self.part_of_group(group) {
                Some(MAIN_PART) => 0,
                Some(part) => 1 + parts.iter().position(|(id, _)| *id == part).expect("an open part"),
                None => continue,
            };
            if !mru.contains(&index) {
                mru.push(index);
            }
        }
        Some(json!({ "auxiliary": auxiliary, "mru": mru }))
    }

    /// editorParts.ts `restoreParts`: the stored editor windows open again with their groups and editors.
    fn restore_auxiliary_parts(&mut self, state: &Value, cx: &mut Context<Self>) {
        for stored in state["auxiliary"].as_array().into_iter().flatten() {
            let number = |key: &str| stored["bounds"][key].as_f64().map(|value| value as f32);
            let (Some(x), Some(y)) = (number("x"), number("y")) else { continue };
            let bounds = Bounds::new(point(px(x), px(y)), size(px(number("width").unwrap_or(DEFAULT_WIDTH)), px(number("height").unwrap_or(DEFAULT_HEIGHT))));
            self.auxiliary_windows.last_part += 1;
            let part = self.auxiliary_windows.last_part;
            let group = self.editors.add_group(false);
            self.editor_groups.parked.push((part, PartLayout::auxiliary(group, f32::from(bounds.size.width), f32::from(bounds.size.height))));
            self.auxiliary_windows.windows.push(AuxiliaryWindowState { part, handle: None, bounds: Rc::new(Cell::new(bounds)), compact: None });
            self.activate_group(group);
            self.restore_part_state(part, &stored["state"]);
            self.set_compact(part, stored["compact"] == true);
            self.open_auxiliary_window(part, bounds, cx);
        }
        // The most recent part's active group is the active group, the main part's when it was.
        let most_recent = state["mru"].as_array().and_then(|mru| mru.first()).and_then(Value::as_u64).unwrap_or(0) as usize;
        let parts: Vec<u32> = self.auxiliary_windows.part_ids().collect();
        let part = most_recent.checked_sub(1).and_then(|index| parts.get(index)).copied();
        let active = self.part_active_group(part.unwrap_or(MAIN_PART)).expect("a part has a group");
        self.activate_group(active);
        self.auxiliary_windows.active = part;
    }

    /// At startup, once the main part restored its editors: the editor windows of the last session open again.
    pub fn restore_auxiliary_windows(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.stored_editor_parts_state() {
            self.restore_auxiliary_parts(&state, cx);
            self.editors_changed(cx);
        }
    }

    /// While a tab drags out of this window: the window under the pointer gets the moves and the release (see the
    /// module doc).
    pub(super) fn cross_window_drag(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let workbench = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = workbench.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.pressed_button != Some(MouseButton::Left) || !dragging_tab(&moved, cx) {
                        return;
                    }
                    let target = outside(window, event.position).then(|| window_under(window, event.position, cx)).flatten();
                    let previous = moved.update(cx, |workbench, _| std::mem::replace(&mut workbench.auxiliary_windows.drag_over, target.map(|(handle, _)| handle))).ok().flatten();
                    if let Some(previous) = previous.filter(|previous| target.is_none_or(|(handle, _)| handle != *previous)) {
                        // Left that window: its drop targets see the pointer go.
                        forward(previous, PlatformInput::MouseMove(MouseMoveEvent { position: point(px(-1.), px(-1.)), pressed_button: Some(MouseButton::Left), modifiers: event.modifiers }), cx);
                    }
                    if let Some((handle, position)) = target {
                        forward(handle, PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: Some(MouseButton::Left), modifiers: event.modifiers }), cx);
                    }
                });
                let released = workbench.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left || !dragging_tab(&released, cx) {
                        return;
                    }
                    let dragged = released.update(cx, |workbench, _| {
                        workbench.auxiliary_windows.drag_over = None;
                        workbench.editor_groups.dragged_tab.take()
                    });
                    if !outside(window, event.position) {
                        return;
                    }
                    match window_under(window, event.position, cx) {
                        Some((handle, position)) => {
                            cx.stop_propagation();
                            forward(handle, PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers: event.modifiers, click_count: 1 }), cx);
                        }
                        None => {
                            let screen = window.bounds().origin + event.position;
                            if let Ok(Some(dragged)) = dragged {
                                cx.stop_propagation();
                                let _ = released.update(cx, |workbench, cx| workbench.drop_tab_outside_windows(&dragged, screen, event.modifiers, window, cx));
                            }
                        }
                    }
                    cx.stop_active_drag(window);
                });
            },
        )
        .size_0()
    }

    /// editorTabsControl.ts `onGroupDragEnd` and multiEditorTabsControl.ts `onDragEnd`: a tab released over no
    /// window moves into a new window there (`workbench.editor.dragToOpenWindow`, Option inverts it).
    fn drop_tab_outside_windows(&mut self, dragged: &super::editor_groups::DraggedTab, screen: Point<Pixels>, modifiers: Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let drag_to_open = self.setting("workbench.editor.dragToOpenWindow") != json!(false);
        if drag_to_open == modifiers.alt || !dragged.is_from(cx.entity().entity_id()) {
            return;
        }
        let Some(group) = self.part_of_group(dragged.group).map(|_| dragged.group) else { return };
        self.activate_group(group);
        if dragged.index >= self.editors.editors.len() {
            return;
        }
        self.editors.activate(dragged.index);
        let bounds = new_window_bounds(window.bounds(), Some(screen - dragged.offset));
        self.transfer_to_new_window(Transfer::MoveEditor, bounds, cx);
    }
}

fn dragging_tab(workbench: &WeakEntity<Workbench>, cx: &App) -> bool {
    workbench.upgrade().is_some_and(|workbench| workbench.read(cx).editor_groups.dragged_tab.is_some())
}

fn outside(window: &Window, position: Point<Pixels>) -> bool {
    !Bounds::new(Point::default(), window.viewport_size()).contains(&position)
}

/// The app's frontmost other window under `position` of `window`, with the position in it.
fn window_under(window: &Window, position: Point<Pixels>, cx: &mut App) -> Option<(AnyWindowHandle, Point<Pixels>)> {
    let screen = window.bounds().origin + position;
    let own = window.window_handle();
    let stack = cx.window_stack().unwrap_or_else(|| cx.windows());
    stack.into_iter().filter(|handle| *handle != own).find_map(|handle| {
        let bounds = handle.update(cx, |_, window, _| window.bounds()).ok()?;
        bounds.contains(&screen).then(|| (handle, screen - bounds.origin))
    })
}

fn forward(handle: AnyWindowHandle, event: PlatformInput, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| window.dispatch_event(event, cx));
}

/// auxiliaryWindowService.ts `openWindow`: the default size centered on the active window (30px down and right when
/// that lands on its origin), or at `origin` (dnd.ts `getNewWindowBoundsAtDragEnd`).
fn new_window_bounds(active: Bounds<Pixels>, origin: Option<Point<Pixels>>) -> Bounds<Pixels> {
    let window_size = size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT));
    let origin = origin.unwrap_or_else(|| {
        let centered = point((active.origin.x + active.size.width / 2. - window_size.width / 2.).max(px(0.)), (active.origin.y + active.size.height / 2. - window_size.height / 2.).max(px(0.)));
        if centered == active.origin { centered + point(px(30.), px(30.)) } else { centered }
    });
    // A fractional origin (a drop point less the pointer's offset in the tab) grows the window by a point.
    Bounds::new(point(origin.x.round(), origin.y.round()), window_size)
}

#[cfg(test)]
mod tests;
