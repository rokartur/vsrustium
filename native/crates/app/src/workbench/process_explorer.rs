//! processExplorer.contribution.ts, processExplorerControl.ts and media/processExplorer.css: Developer: Open Process
//! Explorer shows the tree of the app's processes in its own always-on-top window, refreshed every second.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, Render, Rgba, Role,
    ScrollHandle, Size, Subscription, Task, TextRun, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowKind, WindowOptions, div, point, prelude::*, px, rgba, size,
};
use objc2_foundation::NSProcessInfo;
use serde_json::{Value, json};
use storage::Storage;

use super::Workbench;
use super::context_menu::{LayoutAnchorPosition, layout2d};
use crate::commands::Outcome;
use crate::ps::{self, ProcessItem};

/// `OpenProcessExplorer.STATE_KEY` in StorageScope.APPLICATION.
const STATE_KEY: &str = "workbench.processExplorerWindowState";
/// `OpenProcessExplorer.DEFAULT_STATE`.
const DEFAULT_WIDTH: f32 = 800.;
const DEFAULT_HEIGHT: f32 = 500.;
/// `ProcessListDelegate.getHeight`.
const ROW_HEIGHT: f32 = 22.;
/// The control's `Delayer(1000)` between one update and the next.
const REFRESH_DELAY: Duration = Duration::from_millis(1000);
/// `ByteSize.MB`.
const BYTES_PER_MB: f64 = 1024. * 1024.;
/// The port's extension host entry; processMainService.ts names the host by its utility process instead.
const EXTENSION_HOST_ENTRY: &str = "bootstrap-exthost.js";
/// processMainService.ts `pidToNames`: a utility process is `${name} [${responseWindowId}]`; the port runs one window per process.
const EXTENSION_HOST_NAME: &str = "extension-host [1]";

/// The menu geometry of context_menu.rs (menu.ts `getMenuWidgetCSS`).
const MENU_FONT_SIZE: f32 = 13.;
const MENU_ITEM_HEIGHT: f32 = 24.;
const MENU_SEPARATOR_HEIGHT: f32 = 11.;
const MENU_LABEL_PADDING: f32 = 26.;
const MENU_ITEM_MARGIN: f32 = 4.;
const MENU_BAR_PADDING: f32 = 4.;
const MENU_MIN_WIDTH: f32 = 160.;
const MENU_RADIUS: f32 = 8.;
const MENU_ITEM_RADIUS: f32 = 6.;
/// contextMenuHandler.ts `runOnceToEnableMouseUp`.
const MENU_MOUSE_UP_DELAY: Duration = Duration::from_millis(100);

/// `onTreeContextMenu`'s actions; `None` is the separator. Debug (`attachTo`) is left out: it runs
/// `debug.startFromConfig`, which the port's debug service does not register.
const MENU_ITEMS: [Option<(MenuCommand, &str)>; 5] =
    [Some((MenuCommand::Kill, "Kill Process")), Some((MenuCommand::ForceKill, "Force Kill Process")), None, Some((MenuCommand::Copy, "Copy")), Some((MenuCommand::CopyAll, "Copy All"))];

impl Workbench {
    pub(super) fn register_process_explorer_commands(&mut self) {
        self.commands.register("workbench.action.openProcessExplorer", |workbench, _, _, cx| {
            let (entity, storage) = (cx.entity().downgrade(), workbench.profile_storage.clone());
            // The new window's first render reads the workbench, which this command is still updating.
            cx.defer(move |cx| open_process_explorer(entity, storage, cx));
            Outcome::done()
        });
    }
}

/// `OpenProcessExplorer.run`: `revealIfOpened`, else a new floating window at the stored bounds.
fn open_process_explorer(workbench: WeakEntity<Workbench>, storage: Arc<Mutex<Storage>>, cx: &mut App) {
    if let Some(open) = cx.windows().into_iter().find_map(|window| window.downcast::<ProcessExplorer>()) {
        let _ = open.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let state = storage.lock().unwrap().get(STATE_KEY).map(str::to_owned);
    let (origin, width, height) = stored_window_bounds(state.as_deref());
    let window_size = size(px(width), px(height));
    let bounds = match origin {
        Some((x, y)) => Bounds { origin: point(px(x), px(y)), size: window_size },
        None => Bounds::centered(None, window_size, cx),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Process Explorer".into()), appears_transparent: false, traffic_light_position: None }),
        // `alwaysOnTop`: an NSPanel at the floating window level.
        kind: WindowKind::Floating,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let closing_storage = storage.clone();
        window.on_window_should_close(cx, move |window, _| {
            save_window_state(&closing_storage, window);
            true
        });
        cx.new(|cx| ProcessExplorer::new(workbench, storage, window, cx))
    });
    if let Err(error) = opened {
        eprintln!("[processExplorer] {error:#}");
    }
}

/// `loadState`: the stored bounds, 800x500 when nothing is stored or it does not parse.
fn stored_window_bounds(state: Option<&str>) -> (Option<(f32, f32)>, f32, f32) {
    let state: Value = state.and_then(|state| serde_json::from_str(state).ok()).unwrap_or_default();
    let number = |key: &str| state["bounds"][key].as_f64().map(|value| value as f32);
    (number("x").zip(number("y")), number("width").unwrap_or(DEFAULT_WIDTH), number("height").unwrap_or(DEFAULT_HEIGHT))
}

/// `saveState`, when the window closes.
fn save_window_state(storage: &Mutex<Storage>, window: &Window) {
    let bounds = window.window_bounds().get_bounds();
    let value = |pixels: Pixels| f32::from(pixels).round() as i64;
    let state = json!({ "bounds": { "x": value(bounds.origin.x), "y": value(bounds.origin.y), "width": value(bounds.size.width), "height": value(bounds.size.height) } });
    storage.lock().unwrap().set(STATE_KEY, state.to_string());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RowId {
    /// The `IProcessInformation` element, drawn as the column header.
    Header,
    Process(u32),
}

/// A visible node of the tree.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: RowId,
    /// The parents from the header down; the last is this row's parent.
    ancestors: Vec<RowId>,
    depth: usize,
    has_children: bool,
    expanded: bool,
    /// `ProcessAccessibilityProvider.getAriaLabel`: the process's own name, none for the header.
    aria_label: Option<String>,
    /// Name, CPU, memory and PID, as the row's cells show them.
    cells: [String; 4],
    cmd: String,
}

/// The tree's visible rows: the header, then, while it is expanded, the processes under the root.
fn visible_rows(root: Option<&ProcessItem>, application_name: &str, collapsed: &HashSet<RowId>) -> Vec<Row> {
    let expanded = !collapsed.contains(&RowId::Header);
    let cells = ["Process Name", "CPU (%)", "Memory (MB)", "PID"].map(String::from);
    let mut rows = vec![Row { id: RowId::Header, ancestors: Vec::new(), depth: 1, has_children: true, expanded, aria_label: None, cells, cmd: String::new() }];
    if let (true, Some(root)) = (expanded, root) {
        // `ProcessExplorerModel.update` names the local root after the product.
        push_process(&mut rows, root, application_name, application_name.into(), vec![RowId::Header], collapsed);
    }
    rows
}

fn push_process(rows: &mut Vec<Row>, item: &ProcessItem, name: &str, shown_name: String, ancestors: Vec<RowId>, collapsed: &HashSet<RowId>) {
    let id = RowId::Process(item.pid);
    let expanded = !collapsed.contains(&id);
    let cells = [shown_name, to_fixed_0(item.load), to_fixed_0(item.mem / BYTES_PER_MB), item.pid.to_string()];
    let is_root = ancestors.len() == 1;
    let mut child_ancestors = ancestors.clone();
    child_ancestors.push(id);
    rows.push(Row { id, depth: ancestors.len() + 1, ancestors, has_children: !item.children.is_empty(), expanded, aria_label: Some(name.into()), cells, cmd: item.cmd.clone() });
    if !expanded {
        return;
    }
    for child in &item.children {
        let shown_name = match is_root && child.cmd.contains(EXTENSION_HOST_ENTRY) {
            true => EXTENSION_HOST_NAME.into(),
            false => child.name.clone(),
        };
        push_process(rows, child, &child.name, shown_name, child_ancestors.clone(), collapsed);
    }
}

fn contains_pid(item: &ProcessItem, pid: u32) -> bool {
    item.pid == pid || item.children.iter().any(|child| contains_pid(child, pid))
}

/// `Number.prototype.toFixed(0)`: halves round away from zero, where `{:.0}` rounds them to even.
fn to_fixed_0(value: f64) -> String {
    format!("{:.0}", value.round())
}

/// A row's `innerText`: its cells are flex items, so each is a line.
fn row_text(row: &Row) -> String {
    row.cells.join("\n")
}

/// Copy: the rows of `pids` that are shown, in the given order.
fn copy_text(rows: &[Row], pids: &[u32]) -> String {
    let texts: Vec<String> = pids.iter().filter_map(|pid| rows.iter().find(|row| row.id == RowId::Process(*pid))).map(row_text).collect();
    texts.join("\n")
}

/// Copy All: the `innerText` of the whole tree, header first.
fn copy_all_text(rows: &[Row]) -> String {
    let texts: Vec<String> = rows.iter().map(row_text).collect();
    texts.join("\n")
}

/// abstractTree.ts `_onDidChangeActiveNodes`: an expanded focused or selected node lights its own guides, any other its parent's.
fn active_guide_nodes(rows: &[Row], active: &[RowId]) -> HashSet<RowId> {
    let mut nodes = HashSet::new();
    for row in rows.iter().filter(|row| active.contains(&row.id)) {
        match row.has_children && row.expanded {
            true => nodes.insert(row.id),
            false => row.ancestors.last().is_some_and(|parent| nodes.insert(*parent)),
        };
    }
    nodes
}

/// nativeHostMainService.ts `killProcess`: `process.kill(pid, signal)`.
fn kill(pid: u32, signal: i32) {
    if unsafe { libc::kill(pid as i32, signal) } != 0 {
        eprintln!("[processExplorer] kill {pid}: {}", std::io::Error::last_os_error());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MenuCommand {
    Kill,
    ForceKill,
    Copy,
    CopyAll,
}

struct ProcessMenu {
    pid: u32,
    origin: Point<Pixels>,
    focused: Option<usize>,
    keyboard_navigation: bool,
    shown_at: Instant,
    focus: FocusHandle,
    _blur: Subscription,
    _window_deactivated: Subscription,
}

pub struct ProcessExplorer {
    /// The theme and icons come from the window that opened the explorer.
    workbench: WeakEntity<Workbench>,
    storage: Arc<Mutex<Storage>>,
    application_name: String,
    root: Option<ProcessItem>,
    collapsed: HashSet<RowId>,
    selection: Vec<RowId>,
    focused: Option<RowId>,
    /// Where a Shift selection starts.
    anchor: Option<RowId>,
    /// `RenderIndentGuides.OnHover`.
    tree_hovered: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    menu: Option<ProcessMenu>,
    _refresh: Task<()>,
}

impl ProcessExplorer {
    fn new(workbench: WeakEntity<Workbench>, storage: Arc<Mutex<Storage>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let pid = std::process::id();
        let total_memory = NSProcessInfo::processInfo().physicalMemory() as f64;
        // `update`: `ps` runs off the main thread, the next run 1 s after the last one ended.
        let refresh = cx.spawn(async move |explorer, cx| {
            loop {
                let listed = cx.background_executor().spawn(async move { ps::list_processes(pid, total_memory) }).await;
                if explorer.update(cx, |explorer, cx| explorer.set_processes(listed, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(REFRESH_DELAY).await;
            }
        });
        Self {
            workbench,
            storage,
            application_name: crate::product::get("applicationName").as_str().unwrap_or_default().to_owned(),
            root: None,
            collapsed: HashSet::new(),
            selection: Vec::new(),
            focused: None,
            anchor: None,
            tree_hovered: false,
            focus,
            scroll: ScrollHandle::new(),
            menu: None,
            _refresh: refresh,
        }
    }

    /// `updateChildren`: nodes keep their state by pid; gone processes leave the selection.
    fn set_processes(&mut self, listed: Result<ProcessItem, String>, cx: &mut Context<Self>) {
        self.root = listed.map_err(|error| eprintln!("Listing processes failed: {error}")).ok();
        let present = |id: &RowId| match (id, self.root.as_ref()) {
            (RowId::Header, _) => true,
            (RowId::Process(pid), Some(root)) => contains_pid(root, *pid),
            (RowId::Process(_), None) => false,
        };
        self.selection.retain(|id| present(id));
        self.collapsed.retain(|id| present(id));
        self.focused = self.focused.filter(|id| present(id));
        self.anchor = self.anchor.filter(|id| present(id));
        cx.notify();
    }

    fn rows(&self) -> Vec<Row> {
        visible_rows(self.root.as_ref(), &self.application_name, &self.collapsed)
    }

    fn selected_pids(&self) -> Vec<u32> {
        self.selection.iter().filter_map(|id| if let RowId::Process(pid) = id { Some(*pid) } else { None }).collect()
    }

    fn toggle(&mut self, id: RowId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }

    /// The list's mouse controller: Cmd toggles a row in the selection, Shift selects from the anchor.
    fn click_row(&mut self, id: RowId, modifiers: Modifiers) {
        self.focused = Some(id);
        if modifiers.shift {
            self.select_range(&self.rows(), id);
            return;
        }
        self.anchor = Some(id);
        if !modifiers.platform {
            self.selection = vec![id];
            return;
        }
        match self.selection.iter().position(|selected| *selected == id) {
            Some(index) => _ = self.selection.remove(index),
            None => self.selection.push(id),
        }
    }

    fn select_range(&mut self, rows: &[Row], to: RowId) {
        let index_of = |id: RowId| rows.iter().position(|row| row.id == id);
        let Some(end) = index_of(to) else { return };
        let start = self.anchor.and_then(index_of).unwrap_or(end);
        self.selection = rows[start.min(end)..=start.max(end)].iter().map(|row| row.id).collect();
    }

    /// listCommands.ts `navigate`: focus the row, reveal it and select it (with Shift, from the anchor).
    fn focus_row(&mut self, rows: &[Row], index: usize, extend: bool) {
        let id = rows[index].id;
        self.focused = Some(id);
        self.scroll.scroll_to_item(index);
        if extend {
            self.select_range(rows, id);
            return;
        }
        self.anchor = Some(id);
        self.selection = vec![id];
    }

    /// The list and tree keybindings (`list.*`) and `onTreeKeyDown`'s Alt+E.
    fn tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        let only = |modifiers: Modifiers| keystroke.modifiers == modifiers;
        let shift = Modifiers { shift: true, ..Default::default() };
        let command = Modifiers { platform: true, ..Default::default() };
        let rows = self.rows();
        let last = rows.len() - 1;
        let focused = self.focused.and_then(|id| rows.iter().position(|row| row.id == id));
        let page = (f32::from(self.scroll.bounds().size.height) / ROW_HEIGHT).floor().max(2.) as usize - 1;
        match keystroke.key.as_str() {
            "e" if only(Modifiers { alt: true, ..Default::default() }) => {
                for pid in self.selected_pids() {
                    kill(pid, libc::SIGTERM);
                }
            }
            // The editor's Close: the auxiliary window closes with its only editor.
            "w" if only(command) => {
                save_window_state(&self.storage, window);
                window.remove_window();
            }
            "a" if only(command) => self.selection = rows.iter().map(|row| row.id).collect(),
            "down" if plain || only(shift) => self.focus_row(&rows, focused.map_or(0, |index| (index + 1).min(last)), !plain),
            "up" if plain || only(shift) => self.focus_row(&rows, focused.map_or(last, |index| index.saturating_sub(1)), !plain),
            "pagedown" if plain => self.focus_row(&rows, focused.map_or(0, |index| (index + page).min(last)), false),
            "pageup" if plain => self.focus_row(&rows, focused.map_or(0, |index| index.saturating_sub(page)), false),
            "home" if plain => self.focus_row(&rows, 0, false),
            "end" if plain => self.focus_row(&rows, last, false),
            "left" if plain => {
                let Some(index) = focused else { return };
                let row = &rows[index];
                if row.has_children && row.expanded {
                    self.collapsed.insert(row.id);
                } else if let Some(parent) = row.ancestors.last().and_then(|parent| rows.iter().position(|row| row.id == *parent)) {
                    self.focus_row(&rows, parent, false);
                }
            }
            "right" if plain => {
                let Some(index) = focused else { return };
                let row = &rows[index];
                match (row.has_children, row.expanded) {
                    (true, false) => _ = self.collapsed.remove(&row.id),
                    (true, true) => self.focus_row(&rows, index + 1, false),
                    (false, _) => {}
                }
            }
            "space" if plain => {
                let Some(index) = focused.filter(|index| rows[*index].has_children) else { return };
                self.toggle(rows[index].id);
            }
            "enter" if plain => {
                let Some(index) = focused else { return };
                self.focus_row(&rows, index, false);
            }
            "escape" if plain => self.selection.clear(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// `onTreeContextMenu` for a process row, at the pointer (`getAnchorRect`: a 2px block).
    fn show_menu(&mut self, pid: u32, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let viewport = Size { width: f32::from(viewport.width), height: f32::from(viewport.height) };
        let anchor = Bounds { origin: point(f32::from(position.x), f32::from(position.y)), size: Size { width: 2., height: 2. } };
        let origin = layout2d(viewport, Size { width: menu_width(window), height: menu_height() }, anchor, LayoutAnchorPosition::Before);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let blur = cx.on_blur(&focus, window, |explorer, window, cx| explorer.hide_menu(window, cx));
        let window_deactivated = cx.observe_window_activation(window, |explorer, window, cx| {
            if !window.is_window_active() {
                explorer.hide_menu(window, cx);
            }
        });
        self.menu = Some(ProcessMenu { pid, origin, focused: None, keyboard_navigation: false, shown_at: Instant::now(), focus, _blur: blur, _window_deactivated: window_deactivated });
        cx.notify();
    }

    fn hide_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else { return };
        if menu.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn run_menu_command(&mut self, command: MenuCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pid) = self.menu.as_ref().map(|menu| menu.pid) else { return };
        self.hide_menu(window, cx);
        match command {
            MenuCommand::Kill => kill(pid, libc::SIGTERM),
            MenuCommand::ForceKill => kill(pid, libc::SIGKILL),
            MenuCommand::Copy => {
                let mut pids = self.selected_pids();
                // A right-clicked row outside the selection is copied alone.
                if !pids.contains(&pid) {
                    pids = vec![pid];
                }
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text(&self.rows(), &pids)));
            }
            MenuCommand::CopyAll => cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_all_text(&self.rows()))),
        }
    }

    /// The `Menu` keys: Up and Down loop over the items, Enter and Space run one, Escape hides the menu.
    fn menu_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(menu) = self.menu.as_mut() else { return };
        if event.keystroke.modifiers.modified() {
            return;
        }
        menu.keyboard_navigation = true;
        let step = |start: usize, forward: bool| {
            let mut index = start;
            loop {
                index = if forward { (index + 1) % MENU_ITEMS.len() } else { (index + MENU_ITEMS.len() - 1) % MENU_ITEMS.len() };
                if MENU_ITEMS[index].is_some() {
                    return index;
                }
            }
        };
        let last = MENU_ITEMS.len() - 1;
        match event.keystroke.key.as_str() {
            "down" => menu.focused = Some(step(menu.focused.unwrap_or(last), true)),
            "up" => menu.focused = Some(step(menu.focused.unwrap_or(0), false)),
            "home" | "pageup" => menu.focused = Some(step(last, true)),
            "end" | "pagedown" => menu.focused = Some(step(0, false)),
            "escape" => return self.hide_menu(window, cx),
            "enter" | "space" => {
                let Some((command, _)) = menu.focused.and_then(|index| MENU_ITEMS[index]) else { return };
                return self.run_menu_command(command, window, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    fn render_row(&self, index: usize, row: &Row, workbench: &Workbench, tree_focused: bool, active_guides: &HashSet<RowId>, cx: &Context<Self>) -> AnyElement {
        let id = row.id;
        let is_header = id == RowId::Header;
        let selected = self.selection.contains(&id);
        let focused = self.focused == Some(id);
        let theme = &workbench.theme;
        let (selection, selection_foreground) = match tree_focused {
            true => (workbench.color("list.activeSelectionBackground"), theme.color("list.activeSelectionForeground")),
            false => (workbench.color("list.inactiveSelectionBackground"), theme.color("list.inactiveSelectionForeground")),
        };
        let foreground = selection_foreground.filter(|_| selected).unwrap_or(workbench.color("foreground"));
        let column_border = theme.color("tree.tableColumnsBorder").unwrap_or(gpui::transparent_black().into());
        // `TreeRenderer.DefaultIndent` plus the 8px indent per level, as the twistie's left padding.
        let indent = 8. * row.depth as f32;
        let guides = row.ancestors.iter().enumerate().filter_map(|(level, ancestor)| {
            let active = active_guides.contains(ancestor);
            let color = match active {
                true => theme.color("tree.indentGuidesStroke"),
                false => theme.color("tree.inactiveIndentGuidesStroke").filter(|_| self.tree_hovered),
            }?;
            // `.monaco-tl-indent` starts 16px in, one 8px guide per parent.
            Some(div().absolute().top_0().bottom_0().left(px(16. + 8. * level as f32)).w(px(8.)).border_l_1().border_color(color))
        });
        let twistie = match row.has_children {
            true => workbench
                .icon(if row.expanded { "chevron-down" } else { "chevron-right" }, 16., 16., foreground)
                .id(("process-twistie", index))
                .on_click(cx.listener(move |explorer, _: &ClickEvent, _, cx| {
                    // `expandOnlyOnTwistieClick`: the twistie toggles without selecting.
                    cx.stop_propagation();
                    explorer.toggle(id);
                    cx.notify();
                }))
                .into_any_element(),
            false => div().into_any_element(),
        };
        let cell = |text: &str| div().h_full().flex().items_center().overflow_hidden().whitespace_nowrap().text_ellipsis().border_r_1().border_color(column_border).child(text.to_owned());
        let hover_colors = (workbench.color("editorHoverWidget.background"), workbench.color("editorHoverWidget.border"), workbench.color("editorHoverWidget.foreground"));
        let cmd = row.cmd.clone();
        let name = cell(&row.cells[0]).id(("process-name", index)).flex_1().min_w_0().pr(px(10.)).when(!is_header, |name| {
            name.tooltip(move |_, cx| cx.new(|_| CommandHover { text: cmd.clone(), colors: hover_colors }).into())
        });
        let cells = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .when(is_header, |cells| cells.font_weight(FontWeight::SEMIBOLD).border_b_1().border_color(column_border))
            .child(name)
            // The CSS widths are content boxes: 60, 90 and 50px inside their padding and border.
            .child(cell(&row.cells[1]).flex_none().w(px(81.)).px(px(10.)))
            .child(cell(&row.cells[2]).flex_none().w(px(111.)).px(px(10.)))
            .child(cell(&row.cells[3]).flex_none().w(px(61.)).pl(px(10.)));
        let outline = theme.color("list.focusOutline").or(theme.color("focusBorder")).filter(|_| focused && (tree_focused || self.menu.is_some()));
        let selection_outline = theme.color("contrastActiveBorder").filter(|_| selected);
        div()
            .id(("process-row", index))
            .role(Role::TreeItem)
            .when_some(row.aria_label.clone(), |element, label| element.aria_label(label))
            .aria_level(row.depth)
            .aria_selected(selected)
            .when(row.has_children, |element| element.aria_expanded(row.expanded))
            .when(focused, |element| element.aria_active_descendant())
            // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
            .cursor_pointer()
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .mx(px(4.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .pl(px(indent))
            .text_color(foreground)
            // `.monaco-list-row:first-of-type` has the same bottom border as the header's cells.
            .when(is_header, |element| element.border_b_1().border_color(column_border))
            .when(selected, |element| element.bg(selection))
            .when(!selected && !focused, |element| {
                let hover = workbench.color("list.hoverBackground");
                element.hover(move |style| style.bg(hover))
            })
            .children(guides)
            // processExplorer.css `force-no-twistie`: the header's twistie has no width.
            .when(!is_header, |element| element.child(div().flex_none().w(px(16.)).mr(px(6.)).h_full().flex().items_center().child(div().relative().left(px(3.)).child(twistie))))
            .child(cells)
            .when_some(selection_outline, |element, color| element.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_dashed().border_color(color)))
            .when_some(outline, |element, color| element.child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color)))
            .on_click(cx.listener(move |explorer, event: &ClickEvent, window, cx| {
                window.focus(&explorer.focus, cx);
                explorer.click_row(id, event.modifiers());
                cx.notify();
            }))
            .when_some(if let RowId::Process(pid) = id { Some(pid) } else { None }, |element, pid| {
                element.on_mouse_down(MouseButton::Right, cx.listener(move |explorer, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    explorer.focused = Some(id);
                    explorer.show_menu(pid, event.position, window, cx);
                }))
            })
            .into_any_element()
    }

    /// The context view: a block over the window that hides the menu on any press outside it, and the menu.
    fn render_menu(&self, workbench: &Workbench, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let theme = &workbench.theme;
        let high_contrast = theme.kind.is_high_contrast();
        let border = theme.color("menu.border").or(theme.color("editorWidget.border"));
        let items: Vec<AnyElement> = MENU_ITEMS.iter().enumerate().map(|(index, item)| self.render_menu_item(menu, index, *item, workbench, cx)).collect();
        let panel = div()
            .id("process-explorer-menu")
            .track_focus(&menu.focus)
            .on_key_down(cx.listener(|explorer, event: &KeyDownEvent, window, cx| explorer.menu_key_down(event, window, cx)))
            .absolute()
            .left(menu.origin.x)
            .top(menu.origin.y)
            .min_w(px(MENU_MIN_WIDTH + 2.))
            .py(px(MENU_BAR_PADDING))
            .rounded(px(MENU_RADIUS))
            .when_some(border, |panel, border| panel.border_1().border_color(border))
            .text_size(px(MENU_FONT_SIZE))
            .text_color(workbench.foreground_or("menu.foreground"))
            .occlude()
            .when(high_contrast, |panel| panel.bg(workbench.color("menu.background")))
            .when(!high_contrast, |panel| {
                panel.child(workbench.frosted_glass("menu.background", MENU_RADIUS)).shadow(vec![BoxShadow { color: rgba(0x00000024).into(), offset: point(px(0.), px(0.)), blur_radius: px(6.), spread_radius: px(0.), inset: false }])
            })
            .children(items);
        let block = div().id("process-explorer-menu-block").absolute().inset_0().occlude().on_any_mouse_down(cx.listener(|explorer, _, window, cx| explorer.hide_menu(window, cx)));
        Some(div().absolute().inset_0().child(block).child(panel).into_any_element())
    }

    fn render_menu_item(&self, menu: &ProcessMenu, index: usize, item: Option<(MenuCommand, &'static str)>, workbench: &Workbench, cx: &Context<Self>) -> AnyElement {
        let Some((command, title)) = item else {
            return div().my(px(5.)).border_b_1().border_color(workbench.color("menu.separatorBackground")).into_any_element();
        };
        let theme = &workbench.theme;
        let high_contrast = theme.kind.is_high_contrast();
        let selected = menu.focused == Some(index);
        let outline = theme.color("menu.selectionBorder").filter(|_| selected && (menu.keyboard_navigation || high_contrast));
        let run = cx.listener(move |explorer: &mut ProcessExplorer, _: &MouseUpEvent, window, cx| {
            cx.stop_propagation();
            if explorer.menu.as_ref().is_some_and(|menu| menu.shown_at.elapsed() >= MENU_MOUSE_UP_DELAY) {
                explorer.run_menu_command(command, window, cx);
            }
        });
        let run = std::rc::Rc::new(run);
        let run_right = run.clone();
        div()
            .id(("process-explorer-menu-item", index))
            .relative()
            .h(px(MENU_ITEM_HEIGHT))
            .mx(px(MENU_ITEM_MARGIN))
            .flex()
            .items_center()
            .rounded(px(MENU_ITEM_RADIUS))
            .cursor_pointer()
            .when(selected && !high_contrast, |item| item.bg(workbench.color("list.hoverBackground")).when_some(theme.color("list.hoverForeground"), |item, color| item.text_color(color)))
            .when_some(outline, |item, color| item.child(div().absolute().inset_0().rounded(px(MENU_ITEM_RADIUS)).border_1().when(high_contrast && !menu.keyboard_navigation, |outline| outline.border_dashed()).border_color(color)))
            .child(div().flex_1().px(px(MENU_LABEL_PADDING)).line_height(px(MENU_FONT_SIZE)).child(title))
            // The empty keybinding label keeps its 2em padding.
            .child(div().px(px(MENU_LABEL_PADDING)))
            .on_mouse_move(cx.listener(move |explorer, _, _, cx| {
                let Some(menu) = explorer.menu.as_mut() else { return };
                if menu.focused != Some(index) || menu.keyboard_navigation {
                    menu.focused = Some(index);
                    menu.keyboard_navigation = false;
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, move |event, window, cx| run(event, window, cx))
            .on_mouse_up(MouseButton::Right, move |event, window, cx| run_right(event, window, cx))
            .into_any_element()
    }
}

impl Render for ProcessExplorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(workbench) = self.workbench.upgrade() else { return div() };
        let workbench = workbench.read(cx);
        let rows = self.rows();
        let tree_focused = self.focus.is_focused(window);
        let mut active = self.selection.clone();
        active.extend(self.focused);
        let active_guides = active_guide_nodes(&rows, &active);
        let elements: Vec<AnyElement> = rows.iter().enumerate().map(|(index, row)| self.render_row(index, row, workbench, tree_focused, &active_guides, cx)).collect();
        div()
            .size_full()
            .relative()
            .bg(workbench.color("editor.background"))
            .text_color(workbench.color("foreground"))
            .text_size(px(13.))
            .child(
                div()
                    .id("process-explorer")
                    .role(Role::Tree)
                    .aria_label("Process Explorer")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|explorer, event: &KeyDownEvent, window, cx| explorer.tree_key_down(event, window, cx)))
                    .on_hover(cx.listener(|explorer, hovered: &bool, _, cx| {
                        explorer.tree_hovered = *hovered;
                        cx.notify();
                    }))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .children(elements),
            )
            .children(self.render_menu(workbench, cx))
    }
}

/// `ProcessItemHover`: the process's command line over its name.
struct CommandHover {
    text: String,
    /// `editorHoverWidget` background, border and foreground.
    colors: (Rgba, Rgba, Rgba),
}

impl Render for CommandHover {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (background, border, foreground) = self.colors;
        div().max_w(px(700.)).px(px(8.)).py(px(2.)).bg(background).border_1().border_color(border).rounded(px(8.)).text_size(px(12.)).line_height(px(19.)).text_color(foreground).child(self.text.clone())
    }
}

/// context_menu.rs `menu_width` for these items: the widest label, its padding, the empty keybinding's and the border.
fn menu_width(window: &Window) -> f32 {
    let font = window.text_style().font();
    let widest = MENU_ITEMS.iter().flatten().map(|(_, title)| {
        let run = TextRun { len: title.len(), font: font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        f32::from(window.text_system().shape_line((*title).into(), px(MENU_FONT_SIZE), &[run], None).width)
    });
    let content = widest.fold(0., f32::max) + 4. * MENU_LABEL_PADDING + 2. * MENU_ITEM_MARGIN;
    content.max(MENU_MIN_WIDTH) + 2.
}

fn menu_height() -> f32 {
    let rows: f32 = MENU_ITEMS.iter().map(|item| if item.is_some() { MENU_ITEM_HEIGHT } else { MENU_SEPARATOR_HEIGHT }).sum();
    2. + 2. * MENU_BAR_PADDING + rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, cmd: &str, load: f64, mem: f64, children: Vec<ProcessItem>) -> ProcessItem {
        ProcessItem { name: ps::find_name(cmd), cmd: cmd.into(), pid, ppid: 0, load, mem, children }
    }

    fn tree() -> ProcessItem {
        let host = process(300, "/x/bun /app/out-exthost/bootstrap-exthost.js --skipWorkspaceStorageLock", 3., 10. * BYTES_PER_MB, vec![process(400, "node /x/server.js", 0., 0., vec![])]);
        process(100, "/Applications/VSRustium.app/Contents/MacOS/vsrustium", 12.5, 150.4 * BYTES_PER_MB, vec![process(200, "/bin/zsh -l", 0., 0., vec![]), host])
    }

    fn ids(rows: &[Row]) -> Vec<RowId> {
        rows.iter().map(|row| row.id).collect()
    }

    #[test]
    fn rows_follow_the_tree_and_hide_collapsed_children() {
        let root = tree();
        let rows = visible_rows(Some(&root), "vsrustium", &HashSet::new());
        assert_eq!(ids(&rows), [RowId::Header, RowId::Process(100), RowId::Process(200), RowId::Process(300), RowId::Process(400)]);
        let names: Vec<&str> = rows.iter().map(|row| row.cells[0].as_str()).collect();
        assert_eq!(names, ["Process Name", "vsrustium", "/bin/zsh -l", "extension-host [1]", "node /x/server.js"]);
        assert_eq!(rows[3].aria_label.as_deref(), Some("electron-nodejs (bootstrap-exthost.js)"));
        assert_eq!(rows.iter().map(|row| row.depth).collect::<Vec<_>>(), [1, 2, 3, 3, 4]);
        assert_eq!(rows[4].ancestors, [RowId::Header, RowId::Process(100), RowId::Process(300)]);
        assert!(rows[3].has_children && !rows[2].has_children);

        let collapsed = HashSet::from([RowId::Process(300)]);
        let rows = visible_rows(Some(&root), "vsrustium", &collapsed);
        assert_eq!(ids(&rows), [RowId::Header, RowId::Process(100), RowId::Process(200), RowId::Process(300)]);
        assert!(!rows[3].expanded);
        assert_eq!(ids(&visible_rows(Some(&root), "vsrustium", &HashSet::from([RowId::Header]))), [RowId::Header]);
        assert_eq!(ids(&visible_rows(None, "vsrustium", &HashSet::new())), [RowId::Header]);
    }

    #[test]
    fn copy_writes_each_cell_on_its_own_line() {
        let rows = visible_rows(Some(&tree()), "vsrustium", &HashSet::new());
        assert_eq!(copy_text(&rows, &[300, 999, 200]), "extension-host [1]\n3\n10\n300\n/bin/zsh -l\n0\n0\n200");
        let all = copy_all_text(&rows);
        assert!(all.starts_with("Process Name\nCPU (%)\nMemory (MB)\nPID\nvsrustium\n13\n150\n100\n"), "{all}");
        assert!(all.ends_with("node /x/server.js\n0\n0\n400"), "{all}");
    }

    #[test]
    fn numbers_round_as_to_fixed() {
        assert_eq!(to_fixed_0(2.5), "3");
        assert_eq!(to_fixed_0(0.4), "0");
        assert_eq!(to_fixed_0(12.5), "13");
        assert_eq!(to_fixed_0(1.5 * BYTES_PER_MB / BYTES_PER_MB), "2");
    }

    #[test]
    fn active_guides_light_the_parent_or_the_expanded_node() {
        let rows = visible_rows(Some(&tree()), "vsrustium", &HashSet::new());
        assert_eq!(active_guide_nodes(&rows, &[RowId::Process(400)]), HashSet::from([RowId::Process(300)]));
        assert_eq!(active_guide_nodes(&rows, &[RowId::Process(300)]), HashSet::from([RowId::Process(300)]));
        assert_eq!(active_guide_nodes(&rows, &[RowId::Process(200)]), HashSet::from([RowId::Process(100)]));
    }

    #[test]
    fn window_state_falls_back_to_800_by_500() {
        assert_eq!(stored_window_bounds(None), (None, 800., 500.));
        assert_eq!(stored_window_bounds(Some("not json")), (None, 800., 500.));
        assert_eq!(stored_window_bounds(Some(r#"{"bounds":{"x":10,"y":20,"width":900,"height":600}}"#)), (Some((10., 20.)), 900., 600.));
    }
}
