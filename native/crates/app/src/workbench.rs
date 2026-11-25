//! The workbench as the reference draws it in the modern floating layout
//! (src/vs/workbench/browser/media/floatingPanels.css), empty or with a folder in the Explorer.
//! Sizes are the reference's CSS pixels.

mod actions;
mod bulk_text_edits;
mod context_menu;
mod define_keybinding;
mod diagnostics;
mod editor_commands;
mod editor_menus;
mod editor_message;
pub mod editor_status;
mod editor_view;
mod explorer_actions;
mod extension_management;
mod explorer_find;
mod file_participants;
mod find_widget;
mod hover;
pub(crate) mod host_requests;
mod input_box_view;
mod keybindings_editor_view;
mod keyboard;
mod layout;
mod marker_navigation;
pub mod menubar;
mod middle_scroll;
mod minimap_view;
pub mod logs_actions;
mod notifications_view;
mod part_grid;
pub mod output_actions;
mod output_view;
pub mod quick_access;
mod quick_open;
mod readonly;
mod search_actions;
mod search_editor_view;
mod search_editor_widget;
mod search_view;
mod status_entries;
use status_entries::{EDITOR_STATUS_PRIORITY, PROBLEMS_PRIORITY};
pub mod themes;
mod quick_input_view;
mod text_input;
mod title_bar_actions;
mod unicode_highlight;
mod unusual_line_terminators;
mod webview_panels;

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use config::service::Settings;
use config::workspace::Workspace;
use storage::Storage;

use gpui::{ClickEvent, Context, Div, DragMoveEvent, ExternalPaths, FocusHandle, FontFeatures, FontWeight, MouseButton, MouseDownEvent, Rgba, Task, Window, div, prelude::*, px, rgb, svg, transparent_black};
use serde_json::Value;

use crate::commands::Commands;
use crate::contextkey;
use crate::menus::MenuId;
use crate::editor::{EditorGroup, TextDocument};
use crate::explorer::FileTree;
use explorer_actions::DraggedItems;
use hover::{HoverContent, HoverOptions, HoverPosition};
use search_view::SideBarView;
use crate::file_icons::{FileIconTheme, FolderIcon};
use crate::host_sync::HostSync;
use crate::languages::Languages;
use crate::theme::{ColorThemeEntry, Theme, ThemeKind};

pub struct Workbench {
    pub theme: Theme,
    /// The contributed color themes the theme picker offers.
    pub color_themes: Vec<ColorThemeEntry>,
    /// The extensions the host runs, whose contributions the workbench reads.
    pub extensions: Vec<exthost::scan::ScannedExtension>,
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
    pub keybindings_editor: keybindings_editor_view::KeybindingsEditor,
    keybinding_marks: define_keybinding::KeybindingMarks,
    diagnostics: diagnostics::Diagnostics,
    /// Context keys set with the `setContext` command.
    context_keys: HashMap<String, Value>,
    pub output: crate::output::Output,
    output_view: output_view::OutputView,
    /// The status bar message and the task that hides it.
    status_message: Option<(String, Task<()>)>,
    scrollbars: editor_view::Scrollbars,
    fold_controls: editor_view::FoldControls,
    sticky_scroll: editor_view::sticky_scroll_view::StickyScroll,
    minimap: minimap_view::Minimap,
    unicode_highlight: unicode_highlight::UnicodeHighlight,
    unusual_line_terminators: unusual_line_terminators::UnusualLineTerminators,
    find: find_widget::FindWidget,
    /// The line and whether on its folding control a press may toggle a fold (folding.ts `mouseDownInfo`).
    fold_mouse_down: Option<(usize, bool)>,
    /// Window x of the text area, recorded at paint, to turn a click into a column.
    text_area_left: Rc<Cell<f32>>,
    /// The text area's window bounds at paint, where content widgets that may overflow it lay out.
    text_area_bounds: Rc<Cell<gpui::Bounds<f32>>>,
    editor_message: editor_message::EditorMessages,
    marker_navigation: marker_navigation::MarkerNavigation,
    readonly_overrides: readonly::SessionOverrides,
    pub editor_focus: FocusHandle,
    /// bannerController.ts: the banner element is focusable (`tabIndex = 0`) inside the editor's DOM node.
    unicode_banner_focus: FocusHandle,
    status_bar_focus: FocusHandle,
    /// `editor.tabFocusMode` when Toggle Tab Key Moves Focus last ran, and the mode it set.
    tab_focus_toggle: Option<(bool, bool)>,
    /// inputMode.ts `InputMode` is `overtype` instead of `insert`: one mode for every editor, starting in insert.
    overtype: bool,
    editor_focus_listeners: Vec<gpui::Subscription>,
    /// `--screenshot` hides the input carets, as ref-screenshot does in the reference, so the blink phase drops out.
    pub(crate) input_carets_hidden: bool,
    /// The IME's marked text: a UTF-16 range of the cursor's line.
    marked_text: Option<Range<usize>>,
    /// A left button press in the text is held, so moves extend the selection.
    selecting_text: bool,
    text_drag: Option<editor_view::drag_and_drop::TextDrag>,
    /// editorOptions.ts `stopRenderingLineAfter` (10000) of the group's code editor; longLinesHelper.ts clears it.
    stop_rendering_line_after: Option<usize>,
    editor_zoom: crate::editor::zoom::EditorZoom,
    middle_scroll: Option<middle_scroll::MiddleScroll>,
    pub quick_inputs: quick_input_view::QuickInputs,
    pub quick_access: quick_access::QuickAccess,
    pub explorer: explorer_actions::ExplorerState,
    pub side_bar_view: search_view::SideBarView,
    pub search: search_view::SearchView,
    pub search_editors: search_editor_view::SearchEditors,
    pub search_editor_widget: search_editor_widget::SearchEditorWidget,
    pub webview_panels: webview_panels::WebviewPanels,
    /// `MenuRegistry`.
    pub menus: Vec<crate::menus::MenuItem>,
    pub(crate) context_menu: Option<context_menu::ContextMenu>,
    context_menus_shown: usize,
    pub(crate) notifications: notifications_view::NotificationsUi,
    /// Extension status bar entries, in statusbarModel.ts order.
    pub(crate) status_entries: Vec<crate::main_thread_status_bar::StatusEntry>,
    pub hovers: hover::Hovers,
    /// What the application menu shows now.
    installed_menubar: Option<menubar::Menubar>,
    /// The application storage (StorageScope.PROFILE and APPLICATION in the default profile), shared with the
    /// extension host's MainThreadStorage.
    profile_storage: Arc<Mutex<Storage>>,
    /// StorageScope.WORKSPACE of the opened folder, shared with the extension host's MainThreadStorage.
    workspace_storage: Arc<Mutex<Storage>>,
    part_grid: part_grid::PartGrid,
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cursors move and editors switch on many paths; every one of them renders.
        self.close_editor_message_on_cursor_change(cx);
        self.follow_marker_navigation_cursor();
        self.refresh_selection_highlights(window);
        self.update_webview_panels(window, cx);
        let viewport = window.viewport_size();
        let window_border = self.theme.color("window.activeBorder");
        // The reference lays the grid out at the full window size and shifts it inside the 1px window border.
        let inset = if window_border.is_some() { 1. } else { 0. };
        let editor = if self.editors.active().is_some() { self.text_editor_part(window, cx) } else { self.editor_part() };
        let grid = div()
            .absolute()
            .left(px(inset))
            .top(px(inset))
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .flex_col()
            .child(self.title_bar(f32::from(viewport.width), cx))
            .child(self.middle_section(viewport, editor, window, cx))
            .child(self.status_bar(window, cx));
        let modal_editor = self.keybindings_editor_widget(window, cx);
        let quick_input = self.quick_input_widget(window, cx);
        let notifications = self.notifications_widget(viewport, window, cx);
        let context_menu = self.context_menu_widget(cx);
        let hover = self.hover_widget(window, cx);
        self.update_menubar(window, cx);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.color("titleBar.activeBackground"))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(grid)
            .children(modal_editor)
            // The quick input lives in the workbench container, inside the window border as the grid.
            .children(quick_input.map(|widget| div().absolute().left(px(inset)).top(px(inset)).child(widget)))
            .children(notifications)
            .children(context_menu)
            .children(hover)
            // style.css: .macos-tahoe windows round 16px, older macOS 10px.
            .when_some(window_border, |root, border| root.child(div().absolute().inset_0().border_1().rounded(px(if is_tahoe_or_newer() { 16. } else { 10. })).border_color(border)))
    }
}

impl Workbench {
    pub fn new(theme: Theme, app_root: PathBuf, workspace: Option<Workspace>, languages: Languages, settings: Arc<Mutex<Settings>>, host: HostSync, cx: &mut Context<Self>) -> Self {
        let mut workbench = Workbench {
            theme,
            color_themes: Vec::new(),
            extensions: Vec::new(),
            app_root,
            folder: workspace.map(|workspace| FileTree::new(workspace, settings.clone())),
            languages,
            file_icons: None,
            editors: EditorGroup::default(),
            settings,
            host,
            commands: Commands::default(),
            skip_dialogs: false,
            keybindings: keyboard::Keybindings::default(),
            keybindings_editor: keybindings_editor_view::KeybindingsEditor::new(cx),
            keybinding_marks: Default::default(),
            diagnostics: Default::default(),
            context_keys: HashMap::new(),
            output: Default::default(),
            output_view: output_view::OutputView::new(cx),
            status_message: None,
            scrollbars: Default::default(),
            fold_controls: Default::default(),
            sticky_scroll: editor_view::sticky_scroll_view::StickyScroll::new(cx),
            minimap: Default::default(),
            unicode_highlight: Default::default(),
            unusual_line_terminators: Default::default(),
            find: find_widget::FindWidget::new(cx),
            fold_mouse_down: None,
            text_area_left: Rc::default(),
            text_area_bounds: Rc::default(),
            editor_message: Default::default(),
            marker_navigation: Default::default(),
            readonly_overrides: Default::default(),
            editor_focus: cx.focus_handle(),
            unicode_banner_focus: cx.focus_handle(),
            status_bar_focus: cx.focus_handle(),
            tab_focus_toggle: None,
            overtype: false,
            editor_focus_listeners: Vec::new(),
            input_carets_hidden: false,
            marked_text: None,
            selecting_text: false,
            text_drag: None,
            stop_rendering_line_after: Some(10_000),
            editor_zoom: Default::default(),
            middle_scroll: None,
            quick_inputs: quick_input_view::QuickInputs::new(cx),
            quick_access: quick_access::QuickAccess::default(),
            explorer: explorer_actions::ExplorerState::new(cx),
            side_bar_view: search_view::SideBarView::Explorer,
            search: search_view::SearchView::new(cx),
            search_editors: HashMap::new(),
            search_editor_widget: search_editor_widget::SearchEditorWidget::new(cx),
            webview_panels: Default::default(),
            menus: crate::menus::workbench_menus(),
            context_menu: None,
            context_menus_shown: 0,
            notifications: notifications_view::NotificationsUi::new(cx),
            status_entries: Vec::new(),
            hovers: hover::Hovers::default(),
            installed_menubar: None,
            profile_storage: Arc::new(Mutex::new(Storage::in_memory())),
            workspace_storage: Arc::new(Mutex::new(Storage::in_memory())),
            part_grid: part_grid::PartGrid::default(),
        };
        crate::main_thread_commands::register_commands_documentation(&mut workbench);
        workbench.register_workbench_actions();
        workbench.register_editor_commands();
        workbench.register_find_commands();
        workbench.register_sticky_scroll_commands();
        workbench.register_context_commands();
        workbench.register_run_commands();
        workbench.register_quick_input_commands();
        workbench.register_quick_access_commands();
        workbench.register_theme_commands();
        workbench.register_editor_status_commands();
        workbench.register_title_bar_actions();
        workbench.register_explorer_actions();
        workbench.register_explorer_find_commands();
        workbench.register_search_actions();
        workbench.register_layout_actions();
        workbench.register_search_editor_actions();
        workbench.register_search_editor_widget_actions();
        workbench.register_file_participant_commands();
        workbench.register_keybindings_editor_commands();
        workbench.register_readonly_commands();
        workbench.register_editor_message_commands();
        workbench.register_marker_navigation_commands();
        workbench.register_define_keybinding_command();
        workbench.register_notifications_commands();
        workbench.register_output_actions();
        workbench.register_logs_actions();
        workbench.register_extension_management_commands();
        // Keybindings resolve before the focused element sees the key, and with nothing focused too.
        let entity = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let _ = entity.update(cx, |workbench, cx| workbench.dispatch_key(&event.keystroke, window, cx));
        })
        .detach();
        cx.on_app_quit(|workbench, _| {
            workbench.store_explorer_view_state();
            async {}
        })
        .detach();
        cx.on_release(|workbench, _| workbench.store_explorer_view_state()).detach();
        workbench
    }

    /// Switches to the app's profile storage and reads the command history and the quick input position from it.
    pub fn restore_profile_state(&mut self, storage: Arc<Mutex<Storage>>) {
        self.profile_storage = storage;
        self.load_command_history();
        self.load_quick_input_view_state();
        self.load_notification_filters();
    }

    /// The opened folders, which quick pick items show their resource paths relative to.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.folder.as_ref().map(|tree| &tree.workspace)
    }

    /// A single click opens a preview editor and keeps the focus in the Explorer, a double click opens a pinned
    /// one and focuses it (`workbench.editor.enablePreview`).
    pub fn open_file(&mut self, path: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        // editorGroupFinder.ts `handleModalEditorPart`: an editor opened without preserveFocus closes the modal.
        if pinned {
            self.close_keybindings_editor(window, cx);
        }
        if path.to_string_lossy().ends_with(crate::search_editor::SEARCH_EDITOR_EXT) {
            return self.open_search_editor_file(path, pinned, window, cx);
        }
        match self.editors.find(&path) {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            // A model without an editor, as a search replace leaves it, keeps its text.
            None => match self.editors.models.iter().position(|model| model.document.is("file", &path)) {
                Some(index) => self.editors.show_model(index, !pinned),
                None => match TextDocument::read(&path, &self.languages, &self.settings.lock().unwrap()) {
                    Ok(document) => self.editors.open(document, !pinned),
                    // shortcut: a read error is logged, not shown as the reference's editor error placeholder.
                    Err(error) => eprintln!("opening {}: {error}", path.display()),
                },
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
        self.drop_closed_search_editors();
        self.load_search_editor_widget();
        self.find_editor_changed();
        self.schedule_keybinding_marks(cx);
        self.update_marker_decorations(None);
        self.sync_webview_panels();
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

    fn title_bar(&self, viewport_width: f32, cx: &mut Context<Self>) -> Div {
        let foreground = self.color("titleBar.activeForeground");
        let command_center_visible = self.setting("window.commandCenter") != Value::Bool(false);
        let layout_control_enabled = self.setting("workbench.layoutControl.enabled") != Value::Bool(false);
        // Disabled codicon actions take disabledForeground (actionbar.css .disabled .action-label:not(.icon)).
        let disabled = self.color("disabledForeground");
        let command_center = div()
            .id("command-center-center")
            .flex()
            .items_center()
            // .command-center-center is 38vw wide plus its border (titlebarpart.css).
            .w(px(viewport_width * 0.38 + 2.))
            .relative()
            .cursor_pointer()
            .h(px(24.))
            .pl(px(8.))
            .border_1()
            .rounded(px(6.))
            .bg(self.color("commandCenter.background"))
            .border_color(self.color("commandCenter.border"))
            .text_color(self.color("commandCenter.foreground"))
            .hover(|center| center.bg(self.color("commandCenter.activeBackground")).border_color(self.color("commandCenter.activeBorder")).text_color(self.color("commandCenter.activeForeground")))
            .text_size(px(12.))
            .child(self.workspace().map_or("Search".into(), Workspace::label))
            .child(self.hover_target("command-center", HoverContent::Text(self.command_center_tooltip()), HoverOptions::instant()))
            .on_mouse_down(MouseButton::Right, cx.listener(show_command_center_context_menu))
            // A Cmd+click belongs to the title's menu, which upstream takes in the capture phase.
            .on_click(cx.listener(|workbench, event: &ClickEvent, window, cx| {
                if !event.modifiers().platform {
                    workbench.execute_command("workbench.action.quickOpenWithModes", Vec::new(), window, cx);
                }
            }));
        let title = div().id("window-title").flex().items_center().map(|title| match command_center_visible {
            true => title
                .when(contextkey::truthy(Some(&self.setting("workbench.navigationControl.enabled"))), |title| {
                    title
                        .child(self.icon("arrow-left", 22., 16., disabled).on_mouse_down(MouseButton::Right, cx.listener(show_command_center_context_menu)).child(self.action_hover("Go Back", "workbench.action.navigateBack", HoverOptions::instant())))
                        .child(self.icon("arrow-right", 22., 16., disabled).ml(px(4.)).mr(px(10.)).on_mouse_down(MouseButton::Right, cx.listener(show_command_center_context_menu)).child(self.action_hover("Go Forward", "workbench.action.navigateForward", HoverOptions::instant())))
                })
                .child(command_center)
                .child(div().w(px(6.))),
            false => title.text_size(px(12.)).text_color(foreground).child(self.window_title()),
        });
        // On macOS a right-click or a Cmd+click on the title shows the title's own menu (titlebarPart.ts).
        let title = title
            .on_mouse_down(MouseButton::Right, cx.listener(|workbench, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                workbench.show_title_bar_context_menu(MenuId::TitleBarTitleContext, event.position, window, cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                // The command center is `-webkit-app-region: no-drag`; the plain window title drags.
                if command_center_visible || event.modifiers.platform {
                    cx.stop_propagation();
                }
                if event.modifiers.platform {
                    workbench.show_title_bar_context_menu(MenuId::TitleBarTitleContext, event.position, window, cx);
                }
            }));
        let center = div().absolute().top(px(6.5)).w_full().h(px(24.)).flex().justify_center().items_center().child(title);
        let side_bar_toggle = if self.side_bar_visible() { "layout-sidebar-left" } else { "layout-sidebar-left-off" };
        let layout_actions = [
            ("layout", "Customize Layout...", "workbench.action.customizeLayout"),
            (side_bar_toggle, "Toggle Primary Side Bar", "workbench.action.toggleSidebarVisibility"),
            ("layout-panel-off", "Toggle Panel", "workbench.action.togglePanel"),
            ("layout-sidebar-right-off", "Toggle Secondary Side Bar", "workbench.action.toggleAuxiliaryBar"),
        ];
        let right = div()
            .absolute()
            .top(px(7.5))
            .right(px(12.))
            .flex()
            .gap(px(4.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.notifications_title_bar_item(foreground, cx))
            .when(layout_control_enabled, |right| right.children(layout_actions.map(|(name, title, command)| self.icon(name, 22., 16., foreground).cursor_pointer().child(self.action_hover(title, command, HoverOptions::instant())))));
        // NativeTitlebarPart.minimumHeight: DEFAULT_CUSTOM_TITLEBAR_HEIGHT with the command center, else macTitlebarSize.
        let height = match (command_center_visible, is_tahoe_or_newer()) {
            (true, _) => 35.,
            (false, true) => 32.,
            (false, false) => 28.,
        };
        div()
            .relative()
            .h(px(height))
            .flex_none()
            .child(center)
            .child(right)
            // titlebarpart.css `.titlebar-drag-region`: the bar drags the window and a double click runs the system's
            // title bar action; the no-drag parts stop the press before it gets here.
            .on_mouse_down(MouseButton::Left, |event, window, _| match event.click_count {
                2 => window.titlebar_double_click(),
                _ => window.start_window_move(),
            })
            .on_mouse_down(MouseButton::Right, cx.listener(|workbench, event: &MouseDownEvent, window, cx| workbench.show_title_bar_context_menu(MenuId::TitleBarContext, event.position, window, cx)))
    }

    /// windowTitle.ts `getFullWindowTitle` with the macOS `window.title` default
    /// `${activeEditorShort}${separator}${rootName}${separator}${folderLocation}${separator}${profileName}`, the last two
    /// empty here, or `nameLong` when it is empty.
    fn window_title(&self) -> String {
        let root_name = self.workspace().map(Workspace::label).unwrap_or_default();
        let active_editor = self.editors.active().map(|editor| self.editor_name(editor)).unwrap_or_default();
        let parts: Vec<_> = [active_editor, root_name].into_iter().filter(|part| !part.is_empty()).collect();
        match parts.is_empty() {
            true => menubar::NAME_LONG.to_string(),
            false => parts.join(" \u{2014} "),
        }
    }

    /// commandCenterControl.ts `getTooltip`; its submenu has no keybinding, so "Search {0} \u{2014} {1}".
    fn command_center_tooltip(&self) -> String {
        let root_name = self.workspace().map(Workspace::label).unwrap_or_default();
        format!("Search {root_name} \u{2014} {}", self.window_title())
    }

    fn side_bar_visible(&self) -> bool {
        self.is_part_visible(layout::Part::SideBar)
    }

    fn activity_bar(&self, cx: &mut Context<Self>) -> Div {
        let views = [
            ("source-control", "Source Control", "workbench.view.scm"),
            ("debug-alt", "Run and Debug", "workbench.view.debug"),
            ("extensions", "Extensions", "workbench.view.extensions"),
        ];
        // The side bar is on the left, so the hovers go right of the bar (activitybarPart.ts).
        let hover = HoverOptions::composite_bar(HoverPosition::Right);
        let inactive = self.color("activityBar.inactiveForeground");
        let view_icon = |icon: &str, view: SideBarView, command: &'static str| {
            let active = self.side_bar_view == view && self.side_bar_visible();
            let icon = if active { self.active_view_icon(icon) } else { self.icon(icon, 36., 24., inactive) };
            icon.id(command).cursor_pointer().on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
                // paneCompositeBar.ts `ViewContainerActivityAction.run`: a double click runs once, and the active
                // view's icon hides the side bar unless `workbench.activityBar.iconClickBehavior` is `focus`.
                if event.click_count() > 1 {
                    return;
                }
                let toggles = active && workbench.setting("workbench.activityBar.iconClickBehavior") != "focus";
                let command = if toggles { "workbench.action.closeSidebar" } else { command };
                workbench.execute_command(command, Vec::new(), window, cx);
            }))
        };
        let explorer = view_icon("files", SideBarView::Explorer, "workbench.view.explorer");
        let search = view_icon("search-large", SideBarView::Search, "workbench.view.search");
        div()
            .w(px(44.))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(3.))
            .border_1()
            .when(self.side_bar_visible(), |bar| bar.rounded_l(px(8.)))
            .when(!self.side_bar_visible(), |bar| bar.rounded(px(8.)))
            .bg(self.color("activityBar.background"))
            .border_color(self.color("surface.border"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(explorer.child(self.action_hover("Explorer", "workbench.view.explorer", hover)))
                    .child(search.child(self.action_hover("Search", "workbench.view.search", hover)))
                    .children(views.map(|(name, title, command)| self.icon(name, 36., 24., inactive).cursor_pointer().child(self.action_hover(title, command, hover)))),
            )
            .child(self.icon("settings-gear", 36., 24., inactive).cursor_pointer().child(self.hover_target("manage", HoverContent::Text("Manage".into()), hover)))
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
        let foreground = self.foreground_or("sideBar.foreground");
        if self.side_bar_view == SideBarView::Search {
            let title = self.side_bar_title("Search").child(self.search_title_actions(cx));
            return self.side_bar_frame().text_color(foreground).child(title).child(self.search_view(window, cx));
        }
        let title = self
            .side_bar_title("Explorer")
            // actionbar.css: .monaco-action-bar .action-item is cursor: pointer.
            .child(self.icon("ellipsis", 22., 16., foreground).cursor_pointer());
        // emptyView.ts draws its welcome content (Open Folder, Open Recent) under the header: milestone 11 item 2.
        let Some(tree) = self.folder.as_ref() else {
            return self.side_bar_frame().text_color(foreground).child(title).child(self.section_header("No Folder Opened", true, false));
        };
        // explorerviewlet.css: in high contrast an explorer item has a 20px line from the row top, 1px above center.
        let label_line = if self.theme.kind.is_high_contrast() { 20. } else { 22. };
        let edit_focused = self.explorer.input_focus.is_focused(window);
        let tree_focused = self.explorer.focus.is_focused(window);
        // listWidget.ts: the focused tree draws the active selection colors and outlines its focused row.
        let (selection, selection_foreground) = match tree_focused {
            true => (self.color("list.activeSelectionBackground"), self.theme.color("list.activeSelectionForeground")),
            false => (self.color("list.inactiveSelectionBackground"), self.theme.color("list.inactiveSelectionForeground")),
        };
        // `.context-menu-visible .monaco-list.last-focused`: the tree's context menu keeps the focused row outlined.
        let outlined = tree_focused || self.context_menu.as_ref().is_some_and(|menu| menu.focus_to_return.as_ref() == Some(&self.explorer.focus));
        let focus_outline = self.theme.color("list.focusOutline").or(self.theme.color("focusBorder")).filter(|_| outlined);
        let edited = self.explorer.edit.as_ref().map(|edit| edit.renamed.clone());
        // listWidget.ts: the drop target folder and the rows below it take list.dropBackground.
        let drop_target = self.explorer.drop_target.as_ref().filter(|_| cx.has_active_drag());
        let drop_background = self.color("list.dropBackground");
        let tree_rows = tree.rows();
        // explorerViewer.ts `onDragOver`: over a compact row's folder other than its last, that name alone is the
        // drop target, without feedback on the rows.
        let segment_drop_target = drop_target.filter(|target| tree_rows.iter().any(|row| row.compressed.iter().rev().skip(1).any(|(path, _)| path == *target)));
        // listWidget.ts: a selected row gets a 1px dotted listSelectionOutline (activeContrastBorder) inside it.
        let selection_outline = self.theme.color("contrastActiveBorder");
        let folder_name = tree.workspace.label();
        let has_folder_icons = self.file_icons.as_ref().is_some_and(FileIconTheme::has_folder_icons);
        let rows = tree_rows.into_iter().enumerate().map(|(index, row)| {
            let twistie = match (row.is_dir || row.has_nests, row.expanded) {
                (true, true) => Some("chevron-down"),
                (true, false) => Some("chevron-right"),
                (false, _) => None,
            };
            let navigation = tree.navigation(&row);
            let path = row.path;
            let is_dir = row.is_dir;
            let is_edited = edited.as_ref().is_some_and(|renamed| match renamed {
                Some(renamed) => *renamed == path || row.compressed.iter().any(|(folder, _)| folder == renamed),
                None => row.is_new,
            });
            let is_selected = !row.is_new && tree.selected.contains(&path);
            let is_focused = !row.is_new && tree.focused.as_ref() == Some(&path);
            let name = match (is_edited, navigation) {
                (true, _) => self.explorer_input_box(label_line, edit_focused, cx).into_any_element(),
                (false, Some(navigation)) => self.compressed_label(&row.compressed, &navigation, is_focused, segment_drop_target, cx).into_any_element(),
                (false, None) => self.explorer_find_label(row.name, &path, (is_dir || row.has_nests) && !row.expanded),
            };
            let is_cut = tree.is_cut(&path);
            let is_drop_target = !row.is_new && segment_drop_target.is_none() && drop_target.is_some_and(|target| path.starts_with(target));
            let drag = self.explorer_drag(&path).filter(|_| !is_edited);
            // views.css align-icons-and-twisties: with file icons and no folder icons a file's empty twistie collapses
            // to its 8px indent and the icon takes its place, so file and folder names line up.
            // explorerViewer.ts renderInputBox: the icon follows the name while it is typed.
            let icon_path = match self.explorer.edit.as_ref().filter(|_| is_edited) {
                Some(edit) => PathBuf::from(&edit.input.value),
                None => path.clone(),
            };
            let file_icon = self.file_icons.as_ref().and_then(|icons| match is_dir {
                true => {
                    let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
                    let folder = FolderIcon { name: &name, is_root: tree.is_root(&path), expanded: row.expanded };
                    icons.folder_icon(folder, self.theme.kind, 13., label_line, foreground)
                }
                false => icons.file_icon(&icon_path, self.languages.language_of(&icon_path, ""), self.theme.kind, 13., label_line, foreground),
            });
            let indent = 8. * (row.depth - 1) as f32;
            let has_nests = row.has_nests;
            let (twistie_path, row_expanded) = (path.clone(), row.expanded);
            let twistie_element = |chevron| {
                self.icon(chevron, 16., 16., foreground).id(("explorer-twistie", index)).on_click(cx.listener(move |workbench, _: &ClickEvent, _, cx| {
                    // explorerView.ts `expandOnlyOnTwistieClick`: a nest's row opens the file, its twistie toggles.
                    cx.stop_propagation();
                    workbench.folder.as_mut().expect("rows exist only with a folder").set_expanded(&twistie_path, !row_expanded);
                    cx.notify();
                }))
            };
            let hover = self.hover_target(("explorer-hover", index), HoverContent::Text(crate::quick_input::path_label(&path, None)), HoverOptions::mouse());
            div()
                .id(index)
                // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
                .cursor_pointer()
                .relative()
                .when(!row.is_new && !is_edited, |row| row.child(hover))
                .h(px(22.))
                .mx(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .map(|row| match (file_icon, twistie.is_some() || has_folder_icons) {
                    (Some(icon), false) => row.pl(px(8. + indent)).child(icon.self_start()).child(div().self_start().flex_1().ml(px(6.)).line_height(px(label_line)).child(name)),
                    // monaco-tl-twistie: 3px row inset, 8px base indent plus 8px per level, a 16px icon, 6px before the label.
                    (icon, _) => {
                        let label_margin = if icon.is_some() { 6. } else { 3. };
                        row.pl(px(11. + indent))
                        .child(match twistie {
                            Some(chevron) if has_nests => twistie_element(chevron).into_any_element(),
                            Some(chevron) => self.icon(chevron, 16., 16., foreground).into_any_element(),
                            None => div().size(px(16.)).flex_none().into_any_element(),
                        })
                        .children(icon.map(|icon| icon.self_start().ml(px(3.))))
                            .child(div().self_start().flex_1().ml(px(label_margin)).line_height(px(label_line)).child(name))
                    }
                })
                // explorerviewlet.css: .explorer-item.cut is at half opacity. Its 0.3 for the other rows while a name is
                // edited needs `.explorer-folders-view` twice in the ancestry, which the reference DOM never has.
                .when(is_cut, |row| row.opacity(0.5))
                .when(is_selected, |row| {
                    // shortcut: dashed, GPUI has no dotted border.
                    // An overlay, since a border on the row would push its content 1px like a CSS border, not an outline.
                    let outline = selection_outline.filter(|_| !is_edited).map(|color| div().absolute().inset_0().rounded(px(4.)).border_1().border_dashed().border_color(color));
                    row.relative().bg(selection).when_some(selection_foreground, |row, color| row.text_color(color)).children(outline)
                })
                .when(is_drop_target, |row| row.bg(drop_background))
                .when_some(focus_outline.filter(|_| is_focused && !is_edited), |row, color| {
                    row.relative().child(div().absolute().inset_0().rounded(px(4.)).border_1().border_color(color))
                })
                .when_some(drag, |row, drag| row.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())))
                .when(!row.is_new, |row| {
                    let over = path.clone();
                    let external_over = path.clone();
                    row.on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<DraggedItems>, window, cx| {
                        if event.bounds.contains(&event.event.position) {
                            let items = event.drag(cx).clone();
                            workbench.explorer_drag_over(Some(&over), &items, window, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(move |workbench, event: &DragMoveEvent<ExternalPaths>, window, cx| {
                        if event.bounds.contains(&event.event.position) {
                            workbench.explorer_external_drag_over(Some(&external_over), window, cx);
                        }
                    }))
                    .on_drop(cx.listener(|workbench, items: &DraggedItems, window, cx| {
                        cx.stop_propagation();
                        workbench.explorer_drop(items, window, cx);
                    }))
                    .on_drop(cx.listener(|workbench, paths: &ExternalPaths, window, cx| {
                        cx.stop_propagation();
                        workbench.explorer_import(paths, window, cx);
                    }))
                })
                .when(!is_edited, |row| {
                    let target = path.clone();
                    row.on_mouse_down(MouseButton::Right, cx.listener(move |workbench, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        // explorerView.ts `onContextMenu`: a compact row right-clicked off its names acts on its last folder.
                        let tree = workbench.folder.as_mut().expect("rows exist only with a folder");
                        if let Some(mut navigation) = tree.navigation_at(&target) {
                            navigation.last();
                            tree.set_navigation(&navigation);
                        }
                        workbench.show_explorer_context_menu(Some(&target), event.position, window, cx);
                    }))
                    .on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| {
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
        let rows: Vec<_> = rows.collect();
        self.side_bar_frame()
            .text_color(foreground)
            .child(title)
            .child(div().flex_1().overflow_hidden().flex().flex_col().child(self.section_header(&folder_name, true, false)).child(
                div().relative().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .id("explorer-tree")
                    .track_focus(&self.explorer.focus)
                    .overflow_y_scroll()
                    .track_scroll(&self.explorer.scroll)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Right, cx.listener(|workbench, event: &MouseDownEvent, window, cx| workbench.show_explorer_context_menu(None, event.position, window, cx)))
                    // abstractTree.ts `AbstractFindController.open`: `paddingTop: 30` while the find widget shows.
                    .when(self.explorer.find.is_open(), |tree| tree.pt(px(30.)))
                    .children(rows)
                    .child(self.explorer_empty_area(cx)),
                ).children(self.explorer_find_widget(window, cx))))
            .child(self.section_header("Outline", false, true))
            .child(self.section_header("Timeline", false, true))
            // The pane view is laid out 5px shorter than the side bar content.
            .child(div().h(px(5.)).flex_none())
    }

    fn side_bar_frame(&self) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
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
    }

    /// The composite title bar: the view's name and its actions.
    fn side_bar_title(&self, name: &'static str) -> Div {
        div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(4.))
            .when_some(self.theme.color("sideBarTitle.border"), |title, border| title.border_b_1().border_color(border))
            // The h2 keeps its 32px line under the HC border-bottom, so it is laid out from the top, not centered.
            .child(div().self_start().pl(px(8.)).line_height(px(32.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(self.foreground_or("sideBarTitle.foreground")).child(name))
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

    fn status_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let (foreground, background) = match self.folder {
            Some(_) => (self.color("statusBar.foreground"), self.color("statusBar.background")),
            None => (self.color("statusBar.noFolderForeground"), self.color("statusBar.noFolderBackground")),
        };
        // statusbarpart.css: pointer only on items with a command; status.message has none.
        let problems = self.problems_status_item(foreground);
        div()
            .relative()
            .h(px(28.))
            .flex_none()
            .track_focus(&self.status_bar_focus)
            .flex()
            .justify_between()
            .items_center()
            .px(px(12.))
            .bg(background)
            .text_color(foreground)
            .text_size(px(12.))
            // statusbarpart.css: font-variant-numeric: tabular-nums.
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            // statusbarpart.css: a 1px `statusBar.focusBorder` outline inside the focused part, its bottom corners rounded on macOS.
            .when_some(self.theme.color("statusBar.focusBorder").filter(|_| self.status_bar_focus.is_focused(window)), |bar, border| {
                let radius = px(if is_tahoe_or_newer() { 16. } else { 10. });
                bar.child(div().absolute().inset_0().border_1().rounded_bl(radius).rounded_br(radius).border_color(border))
            })
            .child(div().h(px(24.)).flex().child(self.icon("remote", 28., 16., foreground).h_full().cursor_pointer().child(self.hover_target("status.host", HoverContent::Text("Open a Remote Window".into()), HoverOptions::status_bar()))).children(self.notifications_status_item(true, foreground, cx)).child(self.extension_status_items(true, |priority| priority > PROBLEMS_PRIORITY, foreground, cx)).child(problems).child(self.extension_status_items(true, |priority| priority <= PROBLEMS_PRIORITY, foreground, cx)).children(self.status_bar_message()).children(self.window_progress_entry(cx.background_executor().now()).map(|entry| self.extension_status_item(&entry, foreground, cx))))
            .child(div().h(px(24.)).flex().child(self.extension_status_items(false, |priority| priority > EDITOR_STATUS_PRIORITY, foreground, cx)).child(self.editor_status_items(foreground, window, cx)).child(self.extension_status_items(false, |priority| priority <= EDITOR_STATUS_PRIORITY, foreground, cx)).children(self.notifications_status_item(false, foreground, cx)))
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
            .relative()
            .size(px(box_size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().external_path(path.to_string_lossy().into_owned()).size(px(icon_size)).text_color(color))
    }
}

fn show_command_center_context_menu(workbench: &mut Workbench, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Workbench>) {
    cx.stop_propagation();
    workbench.show_command_center_context_menu(event.position, window, cx);
}

/// platform.ts `isTahoeOrNewer`: Darwin 25 is macOS 26.
fn is_tahoe_or_newer() -> bool {
    static TAHOE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 26);
    *TAHOE
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use config::workspace::Workspace;
    use exthost::scan::scan_extension;
    use gpui::{Modifiers, TestAppContext, point, px};
    use serde_json::json;

    use super::Workbench;
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
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), Some(Workspace::folder(folder)), languages, crate::default_settings(), host, cx));
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
    fn inspect_key_mappings_json_opens_pinned_untitled_editors_with_the_layout(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let host = HostSync::new(std::sync::mpsc::channel().0);
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host, cx));
        for _ in 0..2 {
            workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.inspectKeyMappingsJSON", Vec::new(), window, cx)));
        }
        cx.run_until_parked();

        let editors = workbench.read_with(cx, |workbench, _| {
            workbench.editors.editors.iter().map(|editor| (editor.document.uri_string(), editor.preview, editor.document.lines[..2].join("\n"))).collect::<Vec<_>>()
        });
        let expected = |name: &str| (format!("untitled:{name}"), false, "{\n\t\"layout\": {".to_string());
        assert_eq!(editors, [expected("Untitled-1"), expected("Untitled-2")]);
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
    fn saving_runs_the_text_save_participants_of_the_settings(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-save-participants-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "ab  \ncd\n\n\n").unwrap();
        std::fs::create_dir_all(dir.join("User")).unwrap();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
        let (sender, receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), settings, HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        workbench.update(cx, |workbench, _| {
            workbench.update_setting("files.trimTrailingWhitespace", json!(true));
            workbench.update_setting("files.trimFinalNewlines", json!(true));
        });
        receiver.try_iter().count();

        cx.simulate_keystrokes("cmd-s");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ab\ncd\n");
        assert!(workbench.read_with(cx, |workbench, _| !workbench.editors.active().unwrap().is_dirty()));
        let methods: Vec<_> = receiver.try_iter().map(|message| message.method).collect();
        assert_eq!(methods.iter().filter(|method| **method == "$acceptModelChanged").count(), 2);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n")), "ab  \ncd\n\n\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn in_tab_focus_mode_tab_moves_focus_to_the_status_bar_instead_of_indenting(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-tab-focus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "ab").unwrap();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench.watch_editor_focus(window, cx);
            workbench
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let state = |cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, _| {
                let moves_focus = workbench.key_context(window).get("editorTabMovesFocus").cloned();
                let editor = workbench.editors.active().unwrap();
                (editor.document.lines.join("\n"), editor.highlights.len(), workbench.status_bar_focus.is_focused(window), moves_focus)
            })
        };

        // wordHighlighter.ts stops the highlights once no code editor has the focus.
        cx.simulate_keystrokes("ctrl-shift-m tab");
        assert_eq!(state(cx), ("ab".to_string(), 0, true, Some(json!(true))));
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(state(cx), ("ab".to_string(), 1, false, Some(json!(true))));
        cx.simulate_keystrokes("ctrl-shift-m tab");
        assert_eq!(state(cx), ("    ab".to_string(), 0, false, Some(json!(false))));
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
        cx.simulate_keystrokes("cmd-k cmd-w");
        assert!(workbench.read_with(cx, |workbench, _| workbench.editors.editors.is_empty()), "workbench.action.closeAllEditors");
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

    #[gpui::test]
    fn selection_anchor_keys_select_from_the_anchor(cx: &mut TestAppContext) {
        use crate::editor::{Position, Selection};
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-anchor-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "hello\nworld").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let state = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| {
                let editor = workbench.editors.active().unwrap();
                (editor.selection_anchor, editor.selections.clone())
            })
        };

        cx.simulate_keystrokes("right cmd-k cmd-b");
        assert_eq!(state(cx).0, Some(Position::new(0, 1)), "editor.action.setSelectionAnchor");
        cx.simulate_input("ab");
        assert_eq!(state(cx).0, Some(Position::new(0, 3)), "typing at the anchor pushes it");
        cx.simulate_keystrokes("escape");
        assert_eq!(state(cx).0, None, "editor.action.cancelSelectionAnchor");
        cx.simulate_keystrokes("cmd-k cmd-b down cmd-k cmd-k");
        assert_eq!(state(cx), (None, vec![Selection { anchor: Position::new(0, 3), head: Position::new(1, 3) }]), "editor.action.selectFromAnchorToCursor");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn dragging_selected_text_moves_it_and_a_key_cancels_the_drag(cx: &mut TestAppContext) {
        use crate::editor::{Position, Selection};
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-text-drag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "Port notes for\nthe editor").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let notes = Selection { anchor: Position::new(0, 5), head: Position::new(0, 10) };
        let alt = gpui::Modifiers { alt: true, ..Default::default() };
        workbench.update(cx, |workbench, cx| {
            workbench.editors.active_mut().unwrap().set_selections(vec![notes]);
            assert!(!workbench.start_text_drag(0, 7., alt, 1), "Alt adds a cursor instead");
            assert!(!workbench.start_text_drag(0, 7., Default::default(), 2), "a double click selects a word");
            assert!(!workbench.start_text_drag(0, 2., Default::default(), 1), "outside the selection");
            assert!(workbench.start_text_drag(0, 7., Default::default(), 1));
            workbench.drag_text(0, 8., false, cx);
            assert_eq!(workbench.text_drag.as_ref().unwrap().target, None, "no target inside the selection");
            workbench.drag_text(1, 7., false, cx);
            assert_eq!(workbench.text_drag.as_ref().unwrap().target, Some(Position::new(1, 7)));
            assert_eq!(workbench.text_cursor_style(), gpui::CursorStyle::Arrow);
            workbench.drop_text(1, 7., Default::default(), cx);
            let editor = workbench.editors.active().unwrap();
            assert_eq!(editor.document.lines, ["Port  for", "the edinotestor"]);
            assert_eq!(editor.selections, [Selection { anchor: Position::new(1, 7), head: Position::new(1, 12) }]);
            assert!(workbench.start_text_drag(1, 9., Default::default(), 1));
            workbench.drag_text(1, 9., true, cx);
            workbench.drag_text(0, 0., true, cx);
            assert_eq!(workbench.text_cursor_style(), gpui::CursorStyle::DragCopy);
        });
        cx.simulate_keystrokes("escape");
        workbench.read_with(cx, |workbench, _| assert!(workbench.text_drag.is_none(), "a key press cancels the drag"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn comment_keys_toggle_the_comments_of_the_file_language(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let typescript = scan_extension(&app_root.join("extensions/typescript-basics"), true).unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-comment-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        std::fs::write(&file, "let a = 1;\n    let b = 2;").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[typescript]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let lines = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.clone());

        cx.simulate_keystrokes("cmd-/");
        assert_eq!(lines(cx), ["// let a = 1;", "    let b = 2;"], "editor.action.commentLine");
        cx.simulate_keystrokes("cmd-/");
        assert_eq!(lines(cx), ["let a = 1;", "    let b = 2;"], "editor.action.commentLine removes");
        cx.simulate_keystrokes("cmd-a cmd-k cmd-c");
        assert_eq!(lines(cx), ["// let a = 1;", "//     let b = 2;"], "editor.action.addCommentLine");
        cx.simulate_keystrokes("cmd-k cmd-u");
        assert_eq!(lines(cx), ["let a = 1;", "    let b = 2;"], "editor.action.removeCommentLine");
        cx.simulate_keystrokes("shift-alt-a");
        assert_eq!(lines(cx), ["/* let a = 1;", "    let b = 2; */"], "editor.action.blockComment");
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(lines(cx), ["let a = 1;", "    let b = 2;"], "undo");

        let second = dir.join("b.ts");
        std::fs::write(&second, "export function main() {\n\treturn 42;\n}\n").unwrap();
        cx.update(|window, cx| workbench.update(cx, |workbench, cx| workbench.open_file(second.clone(), true, window, cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("down end cmd-/ shift-alt-a cmd-a cmd-k cmd-c");
        assert_eq!(lines(cx), ["// export function main() {", "// \t// return 42;/*  */", "// }", ""], "the reference's steps");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn line_operation_keys_edit_the_lines(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-line-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree").unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(sender), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let lines = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.clone());

        cx.simulate_keystrokes("alt-down");
        assert_eq!(lines(cx), ["two", "one", "three"], "editor.action.moveLinesDownAction");
        cx.simulate_keystrokes("shift-alt-down");
        assert_eq!(lines(cx), ["two", "one", "one", "three"], "editor.action.copyLinesDownAction");
        cx.simulate_keystrokes("ctrl-j");
        assert_eq!(lines(cx), ["two", "one", "one three"], "editor.action.joinLines");
        cx.simulate_keystrokes("shift-cmd-k");
        assert_eq!(lines(cx), ["two", "one"], "editor.action.deleteLines");
        cx.simulate_keystrokes("cmd-]");
        assert_eq!(lines(cx), ["two", "    one"], "editor.action.indentLines");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn the_find_widget_searches_what_its_inputs_hold(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "the cat\nand the dog\nthe end").unwrap();
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), dir.join("keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        let run = |command: &str, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(command, vec![], window, cx));
        let found = |cx: &mut gpui::VisualTestContext| {
            workbench.read_with(cx, |workbench, _| {
                let state = &workbench.find.controller.state;
                let selection = workbench.editors.active().unwrap().selection();
                (state.matches_position, state.matches_count, (selection.anchor.line, selection.anchor.column))
            })
        };

        run("actions.find", cx);
        cx.run_until_parked();
        cx.simulate_input("the");
        assert_eq!(found(cx), (1, 3, (0, 0)));
        let zone = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| (workbench.editors.viewport.top_whitespace, workbench.editors.active().unwrap().scroll.top));
        assert_eq!(zone(cx), (33., 0.), "the match on line 1 stays revealed below the zone");
        // findInputFocussed binds Enter to Find Next.
        cx.simulate_keystrokes("enter");
        assert_eq!(found(cx), (2, 3, (1, 4)));
        // findDecorations.ts: the ruler and minimap marks, the current match last.
        let marks = workbench.read_with(cx, |workbench, _| {
            let marks = workbench.find_marks(workbench.editors.active().unwrap(), 400.);
            marks.iter().map(|mark| (mark.range.start.line, mark.in_minimap, mark.current)).collect::<Vec<_>>()
        });
        assert_eq!(marks, [(0, true, false), (2, true, false), (1, true, true)]);
        run("editor.action.goToMatchFindAction", cx);
        cx.simulate_input("-1");
        assert_eq!(found(cx), (3, 3, (2, 0)));
        assert!(workbench.read_with(cx, |workbench, _| workbench.find.go_to_match_highlight.is_some()));
        cx.simulate_input("9");
        let message = workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().unwrap().validation_message.clone());
        assert_eq!(message.as_deref(), Some("Please type a number between 1 and 3"));
        cx.simulate_keystrokes("backspace backspace backspace 2 enter");
        assert_eq!(found(cx), (2, 3, (1, 4)));
        assert!(workbench.read_with(cx, |workbench, _| workbench.quick_inputs.current().is_none() && workbench.find.go_to_match_highlight.is_none()));
        cx.simulate_keystrokes("backspace backspace");
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.find.controller.state.search_string.clone()), "t");

        // The find input has the focus, so the replace input takes it.
        run("editor.action.startFindReplaceAction", cx);
        cx.simulate_input("a");
        cx.simulate_keystrokes("shift-tab");
        cx.simulate_input("the");
        cx.simulate_keystrokes("alt-cmd-enter");
        assert_eq!(zone(cx).0, 62.);
        let text = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document.lines.join("\n"));
        assert_eq!(text, "a cat\nand a dog\na end");

        cx.simulate_keystrokes("escape");
        assert!(workbench.read_with(cx, |workbench, _| !workbench.find.controller.state.is_revealed));
        assert_eq!(zone(cx), (0., 0.));

        let args = serde_json::json!({ "searchString": "a", "isRegex": true });
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("editor.actions.findWithArgs", vec![args], window, cx));
        let state = workbench.read_with(cx, |workbench, _| {
            let state = &workbench.find.controller.state;
            (state.search_string.clone(), state.is_regex, state.is_replace_revealed, state.matches_count)
        });
        assert_eq!(state, ("a".to_string(), true, false, 5));

        // saveQueryState and the history delayer write the workspace storage.
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        let stored = |key: &str, cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.workspace_storage.lock().unwrap().get(key).map(str::to_owned));
        assert_eq!(stored("editor.isRegex", cx).as_deref(), Some("true"));
        assert_eq!(stored("workbench.find.history", cx).as_deref(), Some(r#"["a"]"#));
        cx.simulate_input("b");
        cx.simulate_keystrokes("up");
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.find.controller.state.search_string.clone()), "a");
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.find.find_history.elements().len()), 2);
        // loadQueryState and the history's load on the next start.
        let restored = workbench.update(cx, |workbench, _| {
            workbench.find.find_history.clear();
            workbench.restore_workspace_state(workbench.workspace_storage.clone());
            (workbench.find.controller.state.is_regex, workbench.find.find_history.elements().len())
        });
        assert_eq!(restored, (true, 2));

        use crate::editor::{Position, Selection};
        // wordHighlighter.ts: a match found from the find input keeps the editor's highlighted word.
        workbench.update(cx, |workbench, _| workbench.editors.active_mut().unwrap().set_selections(vec![Selection::caret(Position::new(0, 3))]));
        cx.simulate_input("n");
        cx.simulate_keystrokes("enter");
        let highlights = workbench.read_with(cx, |workbench, _| {
            let editor = workbench.editors.active().unwrap();
            (workbench.find.controller.state.search_string.clone(), editor.selection().head, editor.highlights.clone())
        });
        assert_eq!(highlights, ("an".to_string(), Position::new(1, 2), vec![(0, 2..5)]));
        // onDidFocusEditorText: closing the widget focuses the editor, whose selection's word is highlighted.
        cx.simulate_keystrokes("escape");
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().highlights.clone()), vec![(1, 0..3)]);

        // SimpleButton: a focused button triggers on Enter.
        run("actions.find", cx);
        cx.simulate_input("a");
        let position = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.find.controller.state.matches_position);
        let before = position(cx);
        workbench.update_in(cx, |workbench, window, cx| window.focus(&workbench.find.button_focus["find-next"].clone(), cx));
        cx.simulate_keystrokes("enter");
        assert_eq!(position(cx), before + 1);

        // A click on the widget's background, between the sash and the replace toggle, blurs and stops the highlights.
        workbench.update(cx, |workbench, cx| {
            // The slide in runs on the wall clock.
            workbench.find.toggled = None;
            workbench.editors.active_mut().unwrap().highlights = vec![(0, 0..1)];
            cx.notify();
        });
        cx.run_until_parked();
        let widget = cx.debug_bounds("find-widget").unwrap();
        cx.simulate_click(widget.origin + point(px(3.5), px(16.)), Modifiers::none());
        let blurred = workbench.update_in(cx, |workbench, window, cx| (window.focused(cx).is_none(), workbench.editors.active().unwrap().highlights.is_empty()));
        assert_eq!(blurred, (true, true));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
