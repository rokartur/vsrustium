//! The workbench as the reference draws it in the modern floating layout
//! (src/vs/workbench/browser/media/floatingPanels.css), empty or with a folder in the Explorer.
//! Sizes are the reference's CSS pixels.

mod actions;
mod editor_commands;
mod editor_view;
mod explorer_actions;
pub(crate) mod host_requests;
mod input_box_view;
mod keyboard;
pub mod menubar;
mod minimap_view;
pub mod quick_access;
mod quick_open;
pub mod themes;
mod quick_input_view;
mod text_input;

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use config::service::Settings;
use config::workspace::Workspace;
use storage::Storage;

use gpui::{ClickEvent, Context, Div, DragMoveEvent, FocusHandle, FontFeatures, FontWeight, Rgba, Task, Window, div, prelude::*, px, rgb, svg, transparent_black};
use serde_json::Value;

use crate::commands::Commands;
use crate::editor::{EditorGroup, TextDocument};
use crate::explorer::FileTree;
use explorer_actions::DraggedItems;
use crate::file_icons::FileIconTheme;
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{ColorThemeEntry, Theme, ThemeKind};

pub struct Workbench {
    pub theme: Theme,
    /// The contributed color themes the theme picker offers.
    pub color_themes: Vec<ColorThemeEntry>,
    /// The repo root: codicon SVGs and the letterpress are read from upstream's own files under it.
    pub app_root: PathBuf,
    /// The opened folder; `None` is the empty window.
    pub folder: Option<FileTree>,
    pub languages: Languages,
    /// `None` draws files without icons, as `workbench.iconTheme: null` does.
    pub file_icons: Option<FileIconTheme>,
    pub editors: EditorGroup,
    /// The window's settings, shared with the extension host thread that reloads them on file changes.
    pub settings: Arc<Mutex<Settings>>,
    pub(crate) host: HostSync,
    pub commands: Commands,
    /// Extension tests run: dialogs answer their default without showing (abstractFileDialogService.ts skipDialogs).
    pub skip_dialogs: bool,
    pub keybindings: keyboard::Keybindings,
    /// Context keys set with the `setContext` command.
    context_keys: HashMap<String, Value>,
    /// The level extension tests set with `_extensionTests.setLogLevel`; stderr logging does not filter by it.
    log_level: String,
    /// The status bar message and the task that hides it.
    status_message: Option<(String, Task<()>)>,
    scrollbars: editor_view::Scrollbars,
    fold_controls: editor_view::FoldControls,
    minimap: minimap_view::Minimap,
    /// The line and whether on its folding control a press may toggle a fold (folding.ts `mouseDownInfo`).
    fold_mouse_down: Option<(usize, bool)>,
    /// Window x of the text area, recorded at paint, to turn a click into a column.
    text_area_left: Rc<Cell<f32>>,
    pub editor_focus: FocusHandle,
    /// The IME's marked text: a UTF-16 range of the cursor's line.
    marked_text: Option<Range<usize>>,
    /// A left button press in the text is held, so moves extend the selection.
    selecting_text: bool,
    pub quick_inputs: quick_input_view::QuickInputs,
    pub quick_access: quick_access::QuickAccess,
    pub explorer: explorer_actions::ExplorerState,
    /// `MenuRegistry`.
    pub menus: Vec<crate::menus::MenuItem>,
    /// What the application menu shows now.
    installed_menubar: Option<menubar::Menubar>,
    /// The application storage (StorageScope.PROFILE and APPLICATION in the default profile), shared with the
    /// extension host's MainThreadStorage.
    profile_storage: Arc<Mutex<Storage>>,
    /// StorageScope.WORKSPACE of the opened folder, shared with the extension host's MainThreadStorage.
    workspace_storage: Arc<Mutex<Storage>>,
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let window_border = self.theme.color("window.activeBorder");
        // The reference lays the grid out at the full window size and shifts it inside the 1px window border.
        let inset = if window_border.is_some() { 1. } else { 0. };
        let grid = div()
            .absolute()
            .left(px(inset))
            .top(px(inset))
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .flex_col()
            .child(self.title_bar(f32::from(viewport.width)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .px(px(4.))
                    .pb(px(4.))
                    // The side bar meets the activity bar flush; the bar draws the seam (floatingPanels.css).
                    .child(div().flex().child(self.activity_bar()).when(self.folder.is_some(), |row| row.child(self.side_bar(window, cx))))
                    .child(if self.editors.active().is_some() { self.text_editor_part(window, cx) } else { self.editor_part() }),
            )
            .child(self.status_bar());
        let quick_input = self.quick_input_widget(window, cx);
        self.update_menubar(window, cx);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.color("titleBar.activeBackground"))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(grid)
            .children(quick_input)
            // shortcut: 16px is the macOS 26+ (Tahoe) corner radius (style.css .macos-tahoe); older macOS uses 10px, branch on the OS version if the port supports it.
            .when_some(window_border, |root, border| root.child(div().absolute().inset_0().border_1().rounded(px(16.)).border_color(border)))
    }
}

impl Workbench {
    pub fn new(theme: Theme, app_root: PathBuf, folder: Option<FileTree>, languages: Languages, settings: Arc<Mutex<Settings>>, host: HostSync, cx: &mut Context<Self>) -> Self {
        let mut workbench = Workbench {
            theme,
            color_themes: Vec::new(),
            app_root,
            folder,
            languages,
            file_icons: None,
            editors: EditorGroup::default(),
            settings,
            host,
            commands: Commands::default(),
            skip_dialogs: false,
            keybindings: keyboard::Keybindings::default(),
            context_keys: HashMap::new(),
            log_level: "info".into(),
            status_message: None,
            scrollbars: Default::default(),
            fold_controls: Default::default(),
            minimap: Default::default(),
            fold_mouse_down: None,
            text_area_left: Rc::default(),
            editor_focus: cx.focus_handle(),
            marked_text: None,
            selecting_text: false,
            quick_inputs: quick_input_view::QuickInputs::new(cx),
            quick_access: quick_access::QuickAccess::default(),
            explorer: explorer_actions::ExplorerState::new(cx),
            menus: crate::menus::workbench_menus(),
            installed_menubar: None,
            profile_storage: Arc::new(Mutex::new(Storage::in_memory())),
            workspace_storage: Arc::new(Mutex::new(Storage::in_memory())),
        };
        crate::main_thread_commands::register_commands_documentation(&mut workbench);
        workbench.register_workbench_actions();
        workbench.register_editor_commands();
        workbench.register_context_commands();
        workbench.register_quick_input_commands();
        workbench.register_quick_access_commands();
        workbench.register_theme_commands();
        workbench.register_explorer_actions();
        // Keybindings resolve before the focused element sees the key, and with nothing focused too.
        let entity = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let _ = entity.update(cx, |workbench, cx| workbench.dispatch_key(&event.keystroke, window, cx));
        })
        .detach();
        workbench
    }

    /// Switches to the app's profile storage and reads the command history and the quick input position from it.
    pub fn restore_profile_state(&mut self, storage: Arc<Mutex<Storage>>) {
        self.profile_storage = storage;
        self.load_command_history();
        self.load_quick_input_view_state();
    }

    /// The opened folders, which quick pick items show their resource paths relative to.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.folder.as_ref().map(|tree| &tree.workspace)
    }

    /// A single click opens a preview editor and keeps the focus in the Explorer, a double click opens a pinned
    /// one and focuses it (`workbench.editor.enablePreview`).
    pub fn open_file(&mut self, path: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.find(&path) {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            None => match TextDocument::read(&path, &self.languages, &self.settings.lock().unwrap()) {
                Ok(document) => self.editors.open(document, !pinned),
                // shortcut: a read error is logged, not shown as the reference's editor error placeholder.
                Err(error) => eprintln!("opening {}: {error}", path.display()),
            },
        }
        self.marked_text = None;
        match pinned && self.editors.active().is_some() {
            true => window.focus(&self.editor_focus, cx),
            false => window.focus(&self.explorer.focus, cx),
        }
        self.editors_changed(cx);
    }

    /// Keeps the Explorer selection and the extension host in step with the group.
    fn editors_changed(&mut self, cx: &mut Context<Self>) {
        if let (Some(tree), Some(editor)) = (self.folder.as_mut(), self.editors.active()) {
            tree.reveal(&editor.document.path);
        }
        self.record_editor_history();
        self.host.send_changes(&self.editors);
        cx.notify();
    }

    /// No color resolves to transparent, as an unset CSS variable does.
    fn color(&self, key: &str) -> Rgba {
        self.theme.color(key).unwrap_or(transparent_black().into())
    }

    /// A foreground key with no color inherits, as CSS `color` does; every part here inherits `foreground`.
    fn foreground_or(&self, key: &str) -> Rgba {
        self.theme.color(key).unwrap_or(self.color("foreground"))
    }

    fn title_bar(&self, viewport_width: f32) -> Div {
        let foreground = self.color("titleBar.activeForeground");
        // Disabled codicon actions take disabledForeground (actionbar.css .disabled .action-label:not(.icon)).
        let disabled = self.color("disabledForeground");
        let command_center = div()
            .flex()
            .items_center()
            // .command-center-center is 38vw wide plus its border (titlebarpart.css).
            .w(px(viewport_width * 0.38 + 2.))
            .cursor_pointer()
            .h(px(24.))
            .pl(px(8.))
            .border_1()
            .rounded(px(6.))
            .bg(self.color("commandCenter.background"))
            .border_color(self.color("commandCenter.border"))
            .text_color(self.color("commandCenter.foreground"))
            .text_size(px(12.))
            .child(self.workspace().map_or("Search".into(), Workspace::label));
        let center = div()
            .absolute()
            .top(px(6.5))
            .w_full()
            .h(px(24.))
            .flex()
            .justify_center()
            .items_center()
            .child(self.icon("arrow-left", 22., 16., disabled))
            .child(self.icon("arrow-right", 22., 16., disabled).ml(px(4.)))
            .child(command_center.ml(px(10.)))
            .child(div().w(px(6.)));
        let side_bar_toggle = if self.folder.is_some() { "layout-sidebar-left" } else { "layout-sidebar-left-off" };
        let layout_actions = ["layout", side_bar_toggle, "layout-panel-off", "layout-sidebar-right-off"];
        let right = div()
            .absolute()
            .top(px(7.5))
            .right(px(12.))
            .flex()
            .gap(px(4.))
            .children(layout_actions.map(|name| self.icon(name, 22., 16., foreground).cursor_pointer()));
        div().relative().h(px(35.)).flex_none().child(center).child(right)
    }

    fn activity_bar(&self) -> Div {
        let views = ["search-large", "source-control", "debug-alt", "extensions"];
        let inactive = self.color("activityBar.inactiveForeground");
        let explorer = if self.folder.is_some() { self.active_view_icon("files") } else { self.icon("files", 36., 24., inactive) };
        div()
            .w(px(44.))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(3.))
            .border_1()
            .when(self.folder.is_some(), |bar| bar.rounded_l(px(8.)))
            .when(self.folder.is_none(), |bar| bar.rounded(px(8.)))
            .bg(self.color("activityBar.background"))
            .border_color(self.color("surface.border"))
            .child(div().flex().flex_col().gap(px(8.)).child(explorer.cursor_pointer()).children(views.map(|name| self.icon(name, 36., 24., inactive).cursor_pointer())))
            .child(self.icon("settings-gear", 36., 24., inactive).cursor_pointer())
    }

    /// The checked view: a 32px rounded indicator behind the icon (activityaction.css); High Contrast
    /// hides it and outlines the item in contrastActiveBorder instead (menuActivityBar.css `.hc-black, .hc-light`).
    fn active_view_icon(&self, name: &str) -> Div {
        let indicator = div().absolute().top(px(2.)).left(px(2.)).size(px(32.)).rounded(px(4.));
        let indicator = if self.theme.kind.is_high_contrast() {
            indicator.when_some(self.theme.color("contrastActiveBorder"), |indicator, outline| indicator.border_1().border_color(outline))
        } else {
            indicator.when_some(self.theme.color("modernActivityBarItem.activeBackground"), |indicator, background| indicator.bg(background))
        };
        div().relative().child(indicator).child(self.icon(name, 36., 24., self.color("modernActivityBarItem.activeForeground")))
    }

    /// The Explorer: title, the folder section with its tree, and the collapsed Outline and Timeline sections.
    fn side_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let tree = self.folder.as_ref().expect("side bar is drawn only with a folder");
        let foreground = self.foreground_or("sideBar.foreground");
        let title = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(4.))
            .when_some(self.theme.color("sideBarTitle.border"), |title, border| title.border_b_1().border_color(border))
            // The h2 keeps its 32px line under the HC border-bottom, so it is laid out from the top, not centered.
            .child(div().self_start().pl(px(8.)).line_height(px(32.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(self.foreground_or("sideBarTitle.foreground")).child("Explorer"))
            // actionbar.css: .monaco-action-bar .action-item is cursor: pointer.
            .child(self.icon("ellipsis", 22., 16., foreground).cursor_pointer());
        // explorerviewlet.css: in high contrast an explorer item has a 20px line from the row top, 1px above center.
        let label_line = if self.theme.kind.is_high_contrast() { 20. } else { 22. };
        let edit_focused = self.explorer.input_focus.is_focused(window);
        let tree_focused = self.explorer.focus.is_focused(window);
        // listWidget.ts: the focused tree draws the active selection colors and outlines its focused row.
        let (selection, selection_foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| tree_focused);
        let edited = self.explorer.edit.as_ref().map(|edit| edit.renamed.clone());
        // listWidget.ts: the drop target folder and the rows below it take list.dropBackground.
        let drop_target = self.explorer.drop_target.as_ref().filter(|_| cx.has_active_drag());
        let drop_background = self.color("list.dropBackground");
        // listWidget.ts: a selected row gets a 1px dotted listSelectionOutline (activeContrastBorder) inside it.
        let selection_outline = self.theme.color("contrastActiveBorder");
        let folder_name = tree.workspace.label();
        let rows = tree.rows().into_iter().enumerate().map(|(index, row)| {
            let twistie = match (row.is_dir, row.expanded) {
                (true, true) => Some("chevron-down"),
                (true, false) => Some("chevron-right"),
                (false, _) => None,
            };
            let path = row.path;
            let is_dir = row.is_dir;
            let is_edited = edited.as_ref().is_some_and(|renamed| match renamed {
                Some(renamed) => *renamed == path,
                None => row.is_new,
            });
            let name = match is_edited {
                true => self.explorer_input_box(label_line, edit_focused, cx).into_any_element(),
                false => row.name.into_any_element(),
            };
            let is_selected = !row.is_new && tree.selected.contains(&path);
            let is_focused = !row.is_new && tree.focused.as_ref() == Some(&path);
            let is_cut = tree.is_cut(&path);
            let is_drop_target = !row.is_new && drop_target.is_some_and(|target| path.starts_with(target));
            let drag = self.explorer_drag(&path).filter(|_| !is_edited);
            // views.css align-icons-and-twisties: with file icons and no folder icons a file's empty twistie collapses
            // to its 8px indent and the icon takes its place, so file and folder names line up.
            let icon_path = match row.is_new {
                true => PathBuf::from(&self.explorer.edit.as_ref().map(|edit| edit.input.value.clone()).unwrap_or_default()),
                false => path.clone(),
            };
            let file_icon = self.file_icons.as_ref().filter(|_| !is_dir).and_then(|icons| {
                icons.file_icon(&icon_path, self.languages.language_of(&icon_path, ""), self.theme.kind, 13., label_line, foreground)
            });
            let indent = 8. * (row.depth - 1) as f32;
            div()
                .id(index)
                // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
                .cursor_pointer()
                .h(px(22.))
                .mx(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .map(|row| match file_icon {
                    Some(icon) => row.pl(px(8. + indent)).child(icon.self_start()).child(div().self_start().flex_1().ml(px(6.)).line_height(px(label_line)).child(name)),
                    // monaco-tl-twistie: 3px row inset, 8px base indent plus 8px per level, a 16px icon, 6px before the label.
                    None => row
                        .pl(px(11. + indent))
                        .child(match twistie {
                            Some(chevron) => self.icon(chevron, 16., 16., foreground),
                            None => div().size(px(16.)).flex_none(),
                        })
                        .child(div().self_start().flex_1().ml(px(3.)).line_height(px(label_line)).child(name)),
                })
                // explorerviewlet.css: .explorer-item.cut is at half opacity; while a name is edited the other rows are at 0.3.
                .when(is_cut, |row| row.opacity(0.5))
                .when(edited.is_some() && !is_edited, |row| row.opacity(0.3))
                .when(is_selected && !is_edited, |row| {
                    // shortcut: dashed, GPUI has no dotted border.
                    // An overlay, since a border on the row would push its content 1px like a CSS border, not an outline.
                    let outline = selection_outline.map(|color| div().absolute().inset_0().rounded(px(4.)).border_1().border_dashed().border_color(color));
                    row.relative().bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)).children(outline)
                })
                .when(is_drop_target, |row| row.bg(drop_background))
                .when_some(focus_outline.filter(|_| is_focused && !is_edited), |row, color| {
                    row.relative().child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color))
                })
                .when_some(drag, |row, drag| row.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())))
                .when(!row.is_new, |row| {
                    let over = path.clone();
                    row.on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedItems>, window, cx| {
                        if event.bounds.contains(&event.event.position) {
                            let items = event.drag(cx).clone();
                            workbench.explorer_drag_over(&over, &items, window, cx);
                        }
                    }))
                    .on_drop(cx.listener(|workbench, items: &DraggedItems, window, cx| {
                        cx.stop_propagation();
                        workbench.explorer_drop(items, window, cx);
                    }))
                })
                .when(!is_edited, |row| {
                    row.on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                        let modifiers = event.modifiers();
                        window.focus(&workbench.explorer.focus.clone(), cx);
                        let tree = workbench.folder.as_mut().expect("rows exist only with a folder");
                        tree.click_select(&path, modifiers.platform, modifiers.shift);
                        cx.notify();
                        if modifiers.platform || modifiers.shift {
                            return;
                        }
                        if is_dir {
                            tree.toggle(&path);
                        } else {
                            workbench.open_file(path.clone(), event.click_count() == 2, window, cx);
                        }
                    }))
                })
        });
        div()
            .w(px(256.))
            .flex()
            .flex_col()
            // The seam with the activity bar is the bar's border; the side bar keeps a transparent 1px edge.
            .pl(px(1.))
            .border_t_1()
            .border_r_1()
            .border_b_1()
            .rounded_r(px(8.))
            .bg(self.color("sideBar.background"))
            .border_color(self.color("surface.border"))
            .text_color(foreground)
            .child(title)
            .child(div().flex_1().overflow_hidden().flex().flex_col().child(self.section_header(&folder_name, true, false)).child(div().id("explorer-tree").track_focus(&self.explorer.focus).flex().flex_col().children(rows)))
            .child(self.section_header("Outline", false, true))
            .child(self.section_header("Timeline", false, true))
            // The pane view is laid out 5px shorter than the side bar content.
            .child(div().h(px(5.)).flex_none())
    }

    /// A pane header (paneview.ts): 28px, sideBarSectionHeader colors, a top border on every section but the first.
    fn section_header(&self, title: &str, expanded: bool, separated: bool) -> Div {
        let foreground = self.foreground_or("sideBarSectionHeader.foreground");
        let border = self.theme.color("sideBarSectionHeader.border").filter(|_| separated);
        div()
            .relative()
            .h(px(28.))
            // paneview.css: .pane-header is cursor: pointer; these panes are all collapsible.
            .cursor_pointer()
            .flex_none()
            .mx(px(4.))
            .pl(px(4.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.color("sideBarSectionHeader.background"))
            .text_color(foreground)
            .when_some(border, |header, border| header.child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(border)))
            .child(self.icon(if expanded { "chevron-down" } else { "chevron-right" }, 16., 16., foreground).mx(px(2.)))
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
    }

    fn editor_part(&self) -> Div {
        // The high contrast letterpress files paint their own fill; the others are black at the opacity the file sets (editorgroupview.css).
        let (file, color) = match self.theme.kind {
            ThemeKind::Dark => ("letterpress-dark.svg", rgb(0x000000)),
            ThemeKind::Light => ("letterpress-light.svg", rgb(0x000000)),
            ThemeKind::HighContrastDark => ("letterpress-hcDark.svg", rgb(0x3c3c3c)),
            ThemeKind::HighContrastLight => ("letterpress-hcLight.svg", rgb(0xd9d9d9)),
        };
        let path = self.app_root.join("src/vs/workbench/browser/parts/editor/media").join(file);
        // .watermark-container is 280px tall and centered; the 256px letterpress sits at its top.
        let watermark = div().h(px(280.)).child(svg().external_path(path.to_string_lossy().into_owned()).size(px(256.)).text_color(color));
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .border_1()
            .rounded(px(8.))
            .bg(self.color("editor.background"))
            .border_color(self.color("surface.border"))
            .child(watermark)
    }

    fn status_bar(&self) -> Div {
        let (foreground, background) = match self.folder {
            Some(_) => (self.color("statusBar.foreground"), self.color("statusBar.background")),
            None => (self.color("statusBar.noFolderForeground"), self.color("statusBar.noFolderBackground")),
        };
        // statusbarpart.css: pointer only on items with a command; status.message has none.
        let problems = div()
            .h_full()
            .cursor_pointer()
            .ml(px(3.))
            .px(px(5.))
            .flex()
            .items_center()
            .child(self.icon("error", 16., 16., foreground))
            .child(" 0 ")
            .child(self.icon("warning", 16., 16., foreground))
            .child(" 0");
        div()
            .h(px(28.))
            .flex_none()
            .flex()
            .justify_between()
            .items_center()
            .px(px(12.))
            .bg(background)
            .text_color(foreground)
            .text_size(px(12.))
            // statusbarpart.css: font-variant-numeric: tabular-nums.
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .child(div().h(px(24.)).flex().child(self.icon("remote", 28., 16., foreground).h_full().cursor_pointer()).child(problems).children(self.status_bar_message()))
            .child(div().h(px(24.)).flex().children(self.editor_status_items(foreground)).child(self.icon("bell", 28., 16., foreground).h_full().cursor_pointer()))
    }

    /// The `status.message` entry: multi-chord mode, else the latest status message (notificationsStatus.ts).
    fn status_bar_message(&self) -> Option<Div> {
        let message = self.chord_status().or_else(|| self.status_message.as_ref().map(|(message, _)| message.clone()))?;
        Some(div().h_full().ml(px(3.)).px(px(5.)).flex().items_center().child(message))
    }

    /// A codicon centered in a square box. Drawn from the icon's SVG: Wu's text system skips fonts
    /// without an 'm' glyph, so codicon.ttf cannot load (gpui_macos text_system.rs load_family).
    fn icon(&self, name: &str, box_size: f32, icon_size: f32, color: Rgba) -> Div {
        let path = self.app_root.join("node_modules/@vscode/codicons/src/icons").join(format!("{name}.svg"));
        div()
            .size(px(box_size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().external_path(path.to_string_lossy().into_owned()).size(px(icon_size)).text_color(color))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use config::workspace::Workspace;
    use exthost::scan::scan_extension;
    use gpui::{Modifiers, TestAppContext, point, px};

    use super::Workbench;
    use crate::explorer::FileTree;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    #[gpui::test]
    fn clicking_a_folder_row_toggles_it(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let folder = app_root.join("port/fixtures/workspace");
        let docs = folder.join("docs");
        let languages = Languages::new(&[]);
        let host = HostSync::new(std::sync::mpsc::channel().0);
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(FileTree::new(Workspace::folder(folder))), languages, crate::default_settings(), host, cx));
        cx.run_until_parked();

        // The first tree row (docs) spans y 96..118 under the side bar's "workspace" header (reference DOM dump).
        let docs_row = point(px(120.), px(107.));
        let docs_expanded = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| workbench.folder.as_ref().unwrap().rows().iter().any(|row| row.path == docs && row.expanded))
        };
        cx.simulate_click(docs_row, Modifiers::none());
        assert!(docs_expanded(cx));
        cx.simulate_click(docs_row, Modifiers::none());
        assert!(!docs_expanded(cx));
    }

    #[gpui::test]
    fn keys_and_typed_text_edit_the_focused_editor_and_reach_the_host(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "ab\ncd").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        receiver.try_iter().count();
        let text = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"));

        cx.simulate_keystrokes("down end");
        cx.simulate_input("!");
        assert_eq!(text(cx), "ab\ncd!");
        cx.simulate_keystrokes("shift-home cmd-c");
        assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("cd!"));
        cx.simulate_keystrokes("backspace");
        assert_eq!(text(cx), "ab\n");
        cx.simulate_keystrokes("cmd-z cmd-z enter tab");
        assert_eq!(text(cx), "ab\ncd\n    ");
        cx.simulate_keystrokes("cmd-s");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ab\ncd\n    ");
        assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.active().unwrap().is_dirty()));
        let methods: Vec<_> = receiver.try_iter().map(|message| message.method).collect();
        assert_eq!(methods.iter().filter(|method| **method == "$acceptModelChanged").count(), 6);
        assert_eq!(methods.last(), Some(&"$acceptModelSaved"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn user_keybindings_remove_defaults_and_bind_chords(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-keybindings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "abc").unwrap();
        let user_file = dir.join("keybindings.json");
        let rules = r#"// keybindings.json is JSONC
            [{ "key": "end", "command": "-cursorEnd" }, { "key": "cmd+k cmd+e", "command": "cursorEnd", "when": "editorTextFocus && mine" }]"#;
        std::fs::write(&user_file, rules).unwrap();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), user_file.clone(), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let column = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selection().head.column);
        let status = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.chord_status());

        cx.simulate_keystrokes("end");
        assert_eq!(column(cx), 0, "the user removed the default End binding");
        cx.simulate_keystrokes("cmd-k cmd-e");
        assert_eq!(column(cx), 0, "the chord's when clause needs the `mine` context key");
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("_setContext", vec!["mine".into(), true.into()], window, cx));
        cx.simulate_keystrokes("cmd-k");
        assert_eq!(status(cx).as_deref(), Some("(⌘K) was pressed. Waiting for second key of chord..."));
        cx.simulate_keystrokes("cmd-e");
        assert_eq!(column(cx), 3);
        assert_eq!(status(cx), None, "leaving chord mode drops its message");
        cx.simulate_keystrokes("cmd-k x");
        let message = workbench.read_with(cx, |workbench, _| workbench.status_message.as_ref().map(|(message, _)| message.clone()));
        assert_eq!(message.as_deref(), Some("The key combination (⌘K, X) is not a command."));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn folding_and_cursor_keys_run_through_the_keybindings(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-folding-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "a\n  b\n    c\n  d\ne\n  f").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let hidden = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().folding.hidden().to_vec());
        let cursors = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().selections.len());

        cx.simulate_keystrokes("cmd-k cmd-0");
        assert_eq!(hidden(cx), [1..4, 5..6], "editor.foldAll");
        cx.simulate_keystrokes("cmd-k cmd-j");
        assert!(hidden(cx).is_empty(), "editor.unfoldAll");
        cx.simulate_keystrokes("alt-cmd-[");
        assert_eq!(hidden(cx), [1..4], "editor.fold");
        cx.simulate_keystrokes("alt-cmd-]");
        assert!(hidden(cx).is_empty(), "editor.unfold");
        cx.simulate_keystrokes("down cmd-k cmd-[");
        assert_eq!(hidden(cx), [2..3], "editor.foldRecursively");
        cx.simulate_keystrokes("cmd-k cmd-]");
        assert!(hidden(cx).is_empty(), "editor.unfoldRecursively");

        cx.simulate_keystrokes("alt-cmd-down");
        assert_eq!(cursors(cx), 2, "editor.action.insertCursorBelow");
        cx.simulate_keystrokes("escape");
        assert_eq!(cursors(cx), 1, "removeSecondaryCursors wins over cancelSelection with several cursors");

        // An edit that drops the folded lines changes the visible lines: the host hears the new visibleRanges.
        cx.simulate_keystrokes("up alt-cmd-[");
        receiver.try_iter().count();
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("x");
        let visible_ranges: Vec<_> = receiver.try_iter().filter(|message| message.method == "$acceptEditorPropertiesChanged" && !message.args[1]["visibleRanges"].is_null()).collect();
        assert_eq!(visible_ranges.len(), 1, "{visible_ranges:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
