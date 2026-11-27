//! The workbench as the reference draws it in the modern floating layout
//! (src/vs/workbench/browser/media/floatingPanels.css), empty or with a folder in the Explorer.
//! Sizes are the reference's CSS pixels.

pub mod accessibility;
pub mod accessible_view;
pub mod new_file;
mod actions;
mod breakpoint_widget;
mod bulk_text_edits;
mod configuration;
mod configuration_migration;
mod content_hover;
mod content_hover_view;
mod editor_groups;
pub(crate) mod auxiliary_windows;
mod history_actions;
mod configuration_resolver;
mod context_menu;
pub mod custom_editors;
mod debug_commands;
mod debug_console;
mod problems_quick_fix;
mod problems_table;
mod modal_editor_group;
mod problems_view;
mod refactor_preview;
mod view_containers;
mod view_visibility;
mod debug_toolbar;
pub(crate) mod debug_service;
mod debug_view;
pub(crate) mod testing_service;
mod testing_view;
mod define_keybinding;
mod exception_widget;
mod floating_menu;
mod diagnostics;
mod dialog_view;
mod backup_tracker;
mod banner;
mod dialogs;
mod lifecycle;
mod editor_commands;
mod editor_menus;
mod side_by_side_editor;
mod editor_type_picker;
mod editor_message;
pub mod editor_status;
mod editor_view;
mod explorer_actions;
mod extension_management;
pub(crate) mod extension_host_restart;
mod user_data_profile;
mod profiles_editor_view;
mod icon_select_box;
mod settings_import;
pub use profiles_editor_view::NEW_WINDOW_PROFILE_SETTING;
mod explorer_find;
mod explorer_keyboard;
mod extension_editor;
mod extensions_list_view;
mod workspace_recommendations;
pub(crate) mod localization;
pub(crate) mod extensions_view;
mod bulk_undo;
mod file_compare;
mod file_dialogs;
mod file_participants;
mod find_options_widget;
mod find_widget;
mod folding_providers;
mod auto_save;
pub(crate) mod host;
mod power;
mod relauncher;
mod update;
pub(crate) mod perf_view;
mod proxy_auth;
mod user_activity;
mod process_explorer;
mod views_welcome;
mod welcome_page;
pub mod walkthrough_part;
mod hover;
pub(crate) mod host_requests;
mod input_box_view;
mod keybindings_editor_view;
mod keyboard;
mod markdown_document;
mod markdown_view;
mod empty_view;
mod layout;
mod list_scroll;
mod search_list;
mod tree_guides;
mod local_history_actions;
mod document_highlights;
mod document_symbols;
mod goto_definition_link;
mod goto_symbol;
mod marker_navigation;
mod hierarchy_peek;
mod outline_pane;
mod peek_view;
mod references_peek;
pub mod menubar;
mod middle_scroll;
mod minimap_view;
pub mod logs_actions;
pub mod interactive;
mod kernel_picker;
mod notebook_editor;
mod repl_notebook;
mod notebook_outline;
mod notebook_status_bar;
mod notebooks;
pub(crate) use notebooks::notebook_cells;
mod notifications_view;
mod panel;
mod part_grid;
mod zen_mode;
mod activity;
mod centered_layout;
pub mod output_actions;
mod output_view;
mod view_filter;
mod preferences;
pub(crate) mod terminal_view;
pub(crate) mod opener;
mod terminal_host;
mod external_terminal;
mod terminal_contrib;
mod terminal_find;
mod terminal_links;
mod terminal_suggest;
mod terminal_quick_access;
mod terminal_history;
mod terminal_decorations;
mod terminal_environment;
mod terminal_notifications;
mod terminal_initial_hint;
mod terminal_quick_fix;
mod terminal_resize_overlay;
mod output_compound;
mod terminal_sticky_scroll;
mod task_configure;
pub mod task_matcher_prompt;
pub mod task_quick_pick;
pub(crate) mod task_service;
pub mod quick_access;
mod quick_open;
mod readonly;
mod search_actions;
mod side_bar;
mod view_locations;
mod view_panes;
mod search_editor_view;
mod search_editor_widget;
mod search_view;
mod scroll_locking;
mod settings_editor_view;
mod status_entries;
mod parameter_hints;
mod action_widget;
mod commenting_ranges;
mod inline_progress;
mod paste_edits;
mod code_actions;
mod code_lens;
mod color_detector;
mod inline_completions;
mod language_detection;
pub mod snippet_actions;
mod emmet;
mod tab_completion;
mod open_editors_view;
mod outline_actions;
mod inlay_hints_accessibility;
mod comment_navigation;
mod comment_node;
mod comment_thread_floating;
mod comment_thread_floating_layout;
mod comment_thread_widget;
mod comments;
mod comments_view;
mod color_picker;
mod standalone_color_picker;
mod inlay_hints;
mod rename;
mod links;
mod selection_ranges;
mod semantic_tokens;
mod linked_editing;
mod language_status;
mod format;
mod snippet_completions;
mod snippet_controller;
mod suggest_controller;
mod suggest_widget;
use status_entries::{EDITOR_STATUS_PRIORITY, PROBLEMS_PRIORITY};
pub mod themes;
mod quick_input_view;
mod text_files;
mod trusted_publishers;
pub(crate) mod text_model_resolver;
mod control_characters;
mod editor_placeholder;
mod large_file;
mod inspect_tokens;
mod gutter_mouse;
mod word_wrap;
mod smooth_scrolling;
mod sticky_outline;
mod save_conflict;
mod editor_state;
mod empty_editor_hint;
mod text_input;
mod timeline_view;
mod title_bar_actions;
mod type_navigation;
mod unicode_highlight;
mod unusual_line_terminators;
pub(crate) mod scm_actions;
pub mod scm_graph_view;
mod scm_tree;
mod scm_view;
mod tree_view;
mod file_decorations;
mod url_handlers;
mod webview_views;
mod media_preview;
mod editor_insets;
mod image_carousel;
pub(crate) mod webview_panels;
mod custom_documents;
mod url_actions;
pub(crate) mod url_handler;
mod workspace_actions;
mod workspace_editing;
mod workspace_trust_editor;
mod workspace_trust_view;

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use config::service::Settings;
use config::workspace::Workspace;
use storage::Storage;

use gpui::{ClickEvent, Context, Div, DragMoveEvent, ExternalPaths, FocusHandle, FontFeatures, FontWeight, MouseButton, MouseDownEvent, Rgba, Stateful, Task, Window, div, prelude::*, px, rgb, svg, transparent_black};
use serde_json::Value;

use crate::commands::Commands;
use crate::contextkey;
use crate::menus::MenuId;
use crate::editor::EditorGroup;
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
    /// `None` is the Default product icon theme: the codicons.
    pub product_icons: Option<crate::product_icons::ProductIconTheme>,
    pub editors: EditorGroup,
    /// The window's settings, shared with the extension host thread that reloads them on file changes.
    pub settings: Arc<Mutex<Settings>>,
    pub(crate) host: HostSync,
    pub commands: Commands,
    /// MainThreadCommands `_commandRegistrations`: the host's registration of each id it registered.
    pub(crate) extension_commands: HashMap<String, crate::commands::Registration>,
    /// Extension tests run: dialogs answer their default without showing (abstractFileDialogService.ts skipDialogs).
    pub skip_dialogs: bool,
    pub host_colors: themes::HostColorScheme,
    /// `IEnvironmentService.isExtensionDevelopment`.
    pub extension_development: bool,
    proxy_auth: proxy_auth::ProxyAuth,
    /// The command line profile this window was opened with, kept by a reload as the window's configuration is.
    pub cli_profile: crate::CliProfile,
    /// workbench.ts `setUnexpectedErrorHandler(createUnexpectedErrorHandler(logService))`.
    pub unexpected_errors: crate::output::logger::UnexpectedErrorHandler,
    pub keybindings: keyboard::Keybindings,
    pub keybindings_editor: keybindings_editor_view::KeybindingsEditor,
    pub trust_editor: workspace_trust_editor::TrustEditor,
    pub settings_editor: settings_editor_view::SettingsEditor,
    keybinding_marks: define_keybinding::KeybindingMarks,
    unsupported_settings_render: Option<Task<()>>,
    diagnostics: diagnostics::Diagnostics,
    accessibility: accessibility::AccessibilityState,
    metered: power::MeteredConnection,
    relauncher: relauncher::Relauncher,
    user_activity: user_activity::UserActivity,
    welcome: welcome_page::WelcomePage,
    playground: walkthrough_part::Playground,
    /// Context keys set with the `setContext` command.
    context_keys: HashMap<String, Value>,
    pub output: crate::output::Output,
    output_view: output_view::OutputView,
    terminal_view: terminal_view::TerminalView,
    tasks: task_service::TaskService,
    /// The status bar message and the task that hides it.
    status_message: Option<(String, Task<()>)>,
    scrollbars: editor_view::Scrollbars,
    fold_controls: editor_view::FoldControls,
    sticky_scroll: editor_view::sticky_scroll_view::StickyScroll,
    minimap: minimap_view::Minimap,
    unicode_highlight: unicode_highlight::UnicodeHighlight,
    unusual_line_terminators: unusual_line_terminators::UnusualLineTerminators,
    auto_save: auto_save::AutoSave,
    text_files: text_files::TextFiles,
    find: find_widget::FindWidget,
    /// The line and whether on its folding control a press may toggle a fold (folding.ts `mouseDownInfo`).
    fold_mouse_down: Option<(usize, bool)>,
    /// Window x of the text area, recorded at paint, to turn a click into a column.
    text_area_left: Rc<Cell<f32>>,
    /// The diff editor's width as last laid out, for the inline breakpoint.
    diff_part_width: Rc<Cell<f32>>,
    /// The text area's window bounds at paint, where content widgets that may overflow it lay out.
    text_area_bounds: Rc<Cell<gpui::Bounds<f32>>>,
    editor_message: editor_message::EditorMessages,
    inspect_tokens: inspect_tokens::InspectTokens,
    marker_navigation: marker_navigation::MarkerNavigation,
    goto_symbol: goto_symbol::GotoSymbol,
    definition_link: goto_definition_link::DefinitionLink,
    document_highlights: document_highlights::DocumentHighlights,
    document_symbols: document_symbols::DocumentSymbols,
    references_peek: references_peek::ReferencesPeek,
    outline_pane: outline_pane::OutlinePane,
    open_editors_focus: FocusHandle,
    /// The Open Editors list's focused row by position, moved by the keyboard; `None` follows the active editor
    /// (openEditorsView.ts `focusActiveEditor` on each refresh).
    open_editors_focused_row: Option<usize>,
    open_editors_scroll: gpui::ScrollHandle,
    hierarchy_peek: hierarchy_peek::HierarchyPeek,
    content_hover: content_hover::ContentHover,
    suggest: suggest_controller::Suggest,
    parameter_hints: parameter_hints::ParameterHints,
    code_actions: code_actions::CodeActions,
    action_widget: action_widget::ActionWidget,
    paste_edits: paste_edits::PasteEdits,
    code_lenses: code_lens::CodeLenses,
    inlay_hints: inlay_hints::InlayHints,
    color_detector: color_detector::ColorDetector,
    inline_completions: inline_completions::InlineCompletions,
    language_detection: language_detection::LanguageDetection,
    rename: rename::Rename,
    links: links::Links,
    semantic_tokens: semantic_tokens::SemanticTokens,
    linked_editing: linked_editing::LinkedEditing,
    language_status: language_status::LanguageStatus,
    formatting: format::Formatting,
    snippet_files: snippet_completions::SnippetFiles,
    /// The snippet choice the controller last showed the suggest widget for.
    snippet_choice_shown: Option<(crate::editor::Position, crate::editor::Position)>,
    /// The providers extensions registered through MainThreadLanguageFeatures.
    pub(crate) language_features: crate::language_features::LanguageFeatures,
    folding_providers: folding_providers::FoldingProviders,
    sticky_outline: sticky_outline::StickyOutline,
    cancellable_operations: editor_state::CancellableOperations,
    readonly_overrides: readonly::SessionOverrides,
    /// The editor labels of opened local history entries (`openEntry`), by model path.
    local_history_labels: HashMap<PathBuf, String>,
    /// localHistoryCommands.ts `itemSelectedForCompare`: the timeline item of Select for Compare.
    local_history_selected_for_compare: Option<Value>,
    pub(crate) urls: url_handler::Urls,
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
    /// The line range a gutter press selected, which a drag extends (`selectionStart` of a line selection).
    line_select_start: Option<(crate::editor::Position, crate::editor::Position)>,
    text_drag: Option<editor_view::drag_and_drop::TextDrag>,
    /// editorOptions.ts `stopRenderingLineAfter` (10000) of the group's code editor; longLinesHelper.ts clears it.
    stop_rendering_line_after: Option<usize>,
    editor_zoom: crate::editor::zoom::EditorZoom,
    middle_scroll: Option<middle_scroll::MiddleScroll>,
    smooth_scrolling: Option<smooth_scrolling::SmoothScrolling>,
    pub quick_inputs: quick_input_view::QuickInputs,
    pub quick_access: quick_access::QuickAccess,
    pub explorer: explorer_actions::ExplorerState,
    pub side_bar_view: search_view::SideBarView,
    pub search: search_view::SearchView,
    list_scrolls: list_scroll::ListScrolls,
    pub search_editors: search_editor_view::SearchEditors,
    pub search_editor_widget: search_editor_widget::SearchEditorWidget,
    pub webview_panels: webview_panels::WebviewPanels,
    notebook_views: notebook_editor::NotebookViews,
    /// breadcrumbsControl.ts `_updateEditorTypeControl` per group: computed when the group's active editor (or the
    /// two settings) changes, keyed by that state, never when extensions register editors afterwards.
    editor_type_controls: std::cell::RefCell<HashMap<u32, (String, Option<editor_type_picker::EditorTypes>)>>,
    pub scm: scm_view::ScmView,
    timeline: timeline_view::TimelineView,
    scm_graph: scm_graph_view::ScmGraph,
    tree_views: tree_view::TreeViews,
    decorations: std::cell::RefCell<crate::decorations::Decorations>,
    quick_diffs: editor_view::quick_diff::QuickDiffs,
    pub(crate) github_clients: crate::main_thread_github::GitHubClients,
    /// `contributes.commands` icons, for the view toolbars.
    pub command_icons: HashMap<String, crate::menus::CommandIcon>,
    url_handlers: url_handlers::UrlHandlers,
    webview_views: webview_views::WebviewViews,
    pub extensions_viewlet: extensions_view::ExtensionsViewlet,
    pub extension_editors: Vec<extension_editor::ExtensionEditor>,
    /// `MenuRegistry`.
    pub menus: Vec<crate::menus::MenuItem>,
    pub(crate) context_menu: Option<context_menu::ContextMenu>,
    /// The activity bar's Manage button, the anchor of its menu.
    manage_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    context_menus_shown: usize,
    pub(crate) notifications: notifications_view::NotificationsUi,
    /// Extension status bar entries, in statusbarModel.ts order.
    pub(crate) status_entries: Vec<crate::main_thread_status_bar::StatusEntry>,
    pub hovers: hover::Hovers,
    /// What the application menu shows now.
    installed_menubar: Option<menubar::Menubar>,
    /// StorageScope.PROFILE of the current profile, shared with the extension host's MainThreadStorage.
    profile_storage: Arc<Mutex<Storage>>,
    /// StorageScope.APPLICATION: the default profile's storage, the same as `profile_storage` in that profile.
    application_storage: Arc<Mutex<Storage>>,
    /// StorageScope.WORKSPACE of the opened folder, shared with the extension host's MainThreadStorage.
    workspace_storage: Arc<Mutex<Storage>>,
    pub(crate) user_data_profiles: user_data_profile::UserDataProfiles,
    part_grid: part_grid::PartGrid,
    zen_mode: zen_mode::ZenMode,
    activities: activity::Activities,
    centered_layout: centered_layout::CenteredLayout,
    side_by_side_splits: side_by_side_editor::SideBySideSplits,
    editor_type_picker: editor_type_picker::EditorTypePicker,
    edit_setting: preferences::edit_setting::EditSetting,
    editor_groups: editor_groups::EditorGroups,
    auxiliary_windows: auxiliary_windows::AuxiliaryWindows,
    history: history_actions::History,
    panel: panel::Panel,
    explorer_panes: view_panes::ContainerPanes,
    scm_panes: view_panes::ContainerPanes,
    /// The pane views of the containers generated for moved views, by container id.
    generated_panes: HashMap<&'static str, view_panes::ContainerPanes>,
    /// The panes of moved views, built with `&mut self` before a container draws them.
    moved_view_panes: std::cell::RefCell<Vec<view_panes::ViewPane>>,
    pub workspace_trust: crate::workspace_trust::WorkspaceTrust,
    /// fileCommands.ts `globalResourceToCompare`, set by Select for Compare (`resourceSelectedForCompare`).
    resource_to_compare: Option<crate::diff_editor::Side>,
    /// fileActions.ts `CompareWithClipboardAction.SCHEME_COUNTER`.
    clipboard_compare_count: u32,
    /// The multi-file text edits of `workspace.applyEdit`, the newest last.
    workspace_text_edits: Vec<bulk_undo::WorkspaceTextEdit>,
    /// The APPLICATION_SHARED storage: trust and the recently opened list.
    pub shared_storage: Arc<Mutex<Storage>>,
    /// The recently opened list as last read, which the menu bar shows.
    recently_opened: crate::recently_opened::RecentlyOpened,
    /// Custom dialogs, the shown one first.
    dialogs: Vec<dialog_view::ShownDialog>,
    lifecycle: lifecycle::Lifecycle,
    banner: banner::Banner,
    pub debug: debug_service::DebugService,
    debug_view: debug_view::DebugView,
    debug_console: debug_console::DebugConsole,
    problems: problems_view::ProblemsView,
    problems_quick_fixes: problems_quick_fix::ProblemsQuickFixes,
    comments: comments::Comments,
    problems_focus: FocusHandle,
    refactor_preview: refactor_preview::RefactorPreview,
    breakpoint_widget: Option<breakpoint_widget::BreakpointWidget>,
    pub testing: testing_service::TestingService,
    testing_view: testing_view::TestingView,
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cursors move and editors switch on many paths; every one of them renders.
        self.close_editor_message_on_cursor_change(cx);
        self.stop_inspect_tokens_on_model_change();
        self.follow_marker_navigation_cursor();
        self.follow_references_peek_editor();
        self.follow_hierarchy_peek_editor(window, cx);
        self.lay_out_floating_comment_widgets(window);
        self.follow_symbol_navigation();
        self.follow_definition_link(cx);
        self.refresh_selection_highlights(window);
        self.refresh_document_highlights(window, cx);
        self.refresh_outline_pane(cx);
        self.update_webview_panels(window, cx);
        self.update_notebook_views(window, cx);
        self.update_diff_editor();
        self.update_merge_editor(cx);
        self.update_multi_diff_editor();
        self.update_diff_symbols(cx);
        self.update_quick_diff(cx);
        self.follow_active_editor_repository();
        self.update_timeline(cx);
        self.update_scm_graph(cx);
        self.update_extension_editor_webviews(window, cx);
        self.auto_save_models_changed(cx);
        self.backup_models_changed(cx);
        self.send_notebook_visible_ranges();
        self.follow_content_hover_editor();
        self.follow_rename_editor_state(window, cx);
        self.folding_model_changed(cx);
        self.sticky_outline_frame(cx);
        self.accessibility_frame(cx);
        self.update_metered_connection(cx);
        self.check_relaunch_settings(window, cx);
        self.welcome_page_frame(window, cx);
        let viewport = window.viewport_size();
        let window_border = self.theme.color("window.activeBorder");
        // The reference lays the grid out at the full window size and shifts it inside the 1px window border.
        let inset = if window_border.is_some() { 1. } else { 0. };
        self.editors.sync_editors();
        self.auxiliary_windows.main_window = Some(window.window_handle());
        self.close_auxiliary_windows_of_closed_parts(cx);
        let editor = self.in_part(editor_groups::MAIN_PART, |workbench| workbench.editor_groups_part(window, cx));
        // The title and the status bar's editor items follow the main part's active editor (windowTitle.ts and
        // editorStatus.ts are scoped to the window's part).
        let main_group = self.part_active_group(editor_groups::MAIN_PART).expect("the main part has a group");
        let status_bar = self.is_status_bar_visible().then(|| self.with_group(main_group, |workbench| workbench.status_bar(window, cx)));
        let grid = div()
            .absolute()
            .left(px(inset))
            .top(px(inset))
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .flex_col()
            .child(self.with_group(main_group, |workbench| workbench.title_bar(f32::from(viewport.width), window.is_window_active(), None, cx)))
            .children(self.banner_part(window, cx))
            .child(self.middle_section(viewport, editor, window, cx))
            .children(status_bar);
        let modal_editor = self.keybindings_editor_widget(window, cx).or_else(|| self.settings_editor_widget(window, cx)).or_else(|| self.profiles_editor_widget(window, cx)).or_else(|| self.workspace_trust_editor_widget(window, cx)).or_else(|| self.modal_group_widget(window, cx));
        // The overlays show in the active window of the workbench, this one unless an editor window is.
        let overlays_here = self.auxiliary_windows.active().is_none();
        let quick_input = if overlays_here { self.quick_input_widget(window, cx) } else { None };
        let notifications = self.notifications_widget(viewport, window, cx);
        let context_menu = if overlays_here { self.context_menu_widget(cx) } else { None };
        let action_widget = if overlays_here { self.action_widget(window, cx) } else { None };
        let hover = if overlays_here { self.hover_widget(window, cx) } else { None };
        let dialog = if overlays_here { self.custom_dialog_widget(window, cx) } else { None };
        let accessible_view = self.accessible_view_widget(window, cx);
        self.update_menubar(window, cx);
        self.with_group(main_group, |workbench| workbench.sync_window_title(window));
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(self.shell_background(window.is_window_active()))
            .text_color(self.color("foreground"))
            .text_size(px(13.))
            .child(grid)
            .children(self.debug_toolbar(window, cx))
            .when(self.debug_view.toolbar_drag.is_some(), |root| {
                root.on_mouse_move(cx.listener(|workbench, event, window, cx| workbench.drag_debug_toolbar(event, window, cx)))
                    .on_mouse_up(gpui::MouseButton::Left, cx.listener(|workbench, _, _, _| workbench.end_debug_toolbar_drag()))
            })
            .children(modal_editor)
            // The quick input lives in the workbench container, inside the window border as the grid.
            .children(quick_input.map(|widget| div().absolute().left(px(inset)).top(px(inset)).child(widget)))
            .children(notifications)
            .children(accessible_view)
            .children(context_menu)
            .children(dialog)
            .children(action_widget)
            .children(hover)
            .child(self.cross_window_drag(cx))
            .on_modifiers_changed(cx.listener(|workbench, event: &gpui::ModifiersChangedEvent, _, cx| {
                workbench.hover_modifiers_changed(event.modifiers.alt, cx);
                workbench.terminal_link_modifiers_changed(event.modifiers, cx);
            }))
            .child(self.aria_live_region())
            .capture_any_mouse_down(cx.listener(|workbench, _, _, cx| workbench.user_activity(cx)))
            // style.css: .macos-tahoe windows round 16px, older macOS 10px.
            .when_some(window_border, |root, border| root.child(div().absolute().inset_0().border_1().rounded(px(if is_tahoe_or_newer() { 16. } else { 10. })).border_color(border)))
    }
}

impl Workbench {
    pub fn new(theme: Theme, app_root: PathBuf, workspace: Option<Workspace>, languages: Languages, settings: Arc<Mutex<Settings>>, host: HostSync, cx: &mut Context<Self>) -> Self {
        let accessibility = accessibility::AccessibilityState::new(&app_root, cx);
        let mut workbench = Workbench {
            theme,
            color_themes: Vec::new(),
            extensions: Vec::new(),
            app_root,
            folder: workspace.map(|workspace| FileTree::new(workspace, settings.clone())),
            languages,
            file_icons: None,
            product_icons: None,
            editors: EditorGroup::default(),
            settings,
            host,
            commands: Commands::default(),
            extension_commands: HashMap::new(),
            skip_dialogs: false,
            host_colors: themes::host_color_scheme(),
            extension_development: false,
            proxy_auth: proxy_auth::ProxyAuth::default(),
            cli_profile: Default::default(),
            unexpected_errors: Default::default(),
            keybindings: keyboard::Keybindings::default(),
            keybindings_editor: keybindings_editor_view::KeybindingsEditor::new(cx),
            trust_editor: Default::default(),
            settings_editor: settings_editor_view::SettingsEditor::new(cx),
            keybinding_marks: Default::default(),
            unsupported_settings_render: None,
            diagnostics: Default::default(),
            accessibility,
            metered: Default::default(),
            relauncher: Default::default(),
            user_activity: Default::default(),
            welcome: Default::default(),
            playground: Default::default(),
            context_keys: HashMap::new(),
            output: Default::default(),
            output_view: output_view::OutputView::new(cx),
            terminal_view: terminal_view::TerminalView::new(cx),
            tasks: Default::default(),
            status_message: None,
            scrollbars: Default::default(),
            fold_controls: Default::default(),
            sticky_scroll: editor_view::sticky_scroll_view::StickyScroll::new(cx),
            minimap: Default::default(),
            unicode_highlight: Default::default(),
            unusual_line_terminators: Default::default(),
            auto_save: Default::default(),
            text_files: Default::default(),
            find: find_widget::FindWidget::new(cx),
            fold_mouse_down: None,
            text_area_left: Rc::default(),
            diff_part_width: Rc::default(),
            text_area_bounds: Rc::default(),
            editor_message: Default::default(),
            inspect_tokens: Default::default(),
            marker_navigation: Default::default(),
            goto_symbol: Default::default(),
            definition_link: Default::default(),
            document_highlights: Default::default(),
            document_symbols: Default::default(),
            references_peek: references_peek::ReferencesPeek::new(cx),
            outline_pane: outline_pane::OutlinePane::new(cx),
            open_editors_focus: cx.focus_handle(),
            open_editors_focused_row: None,
            open_editors_scroll: Default::default(),
            hierarchy_peek: hierarchy_peek::HierarchyPeek::new(cx),
            content_hover: Default::default(),
            suggest: Default::default(),
            parameter_hints: Default::default(),
            code_actions: Default::default(),
            action_widget: Default::default(),
            paste_edits: Default::default(),
            code_lenses: Default::default(),
            inlay_hints: Default::default(),
            color_detector: Default::default(),
            inline_completions: Default::default(),
            language_detection: Default::default(),
            rename: rename::Rename::new(cx),
            links: links::Links::default(),
            semantic_tokens: Default::default(),
            linked_editing: Default::default(),
            language_status: Default::default(),
            formatting: Default::default(),
            snippet_files: Default::default(),
            snippet_choice_shown: None,
            language_features: Default::default(),
            folding_providers: Default::default(),
            sticky_outline: Default::default(),
            cancellable_operations: Default::default(),
            readonly_overrides: Default::default(),
            local_history_labels: HashMap::new(),
            local_history_selected_for_compare: None,
            urls: url_handler::Urls::default(),
            editor_focus: cx.focus_handle(),
            unicode_banner_focus: cx.focus_handle(),
            banner: banner::Banner::new(cx),
            status_bar_focus: cx.focus_handle(),
            tab_focus_toggle: None,
            overtype: false,
            editor_focus_listeners: Vec::new(),
            input_carets_hidden: false,
            marked_text: None,
            selecting_text: false,
            line_select_start: None,
            text_drag: None,
            stop_rendering_line_after: Some(10_000),
            editor_zoom: Default::default(),
            middle_scroll: None,
            smooth_scrolling: None,
            quick_inputs: quick_input_view::QuickInputs::new(cx),
            quick_access: quick_access::QuickAccess::default(),
            explorer: explorer_actions::ExplorerState::new(cx),
            side_bar_view: search_view::SideBarView::Explorer,
            search: search_view::SearchView::new(cx),
            list_scrolls: Default::default(),
            search_editors: HashMap::new(),
            search_editor_widget: search_editor_widget::SearchEditorWidget::new(cx),
            webview_panels: Default::default(),
            notebook_views: notebook_editor::NotebookViews::new(cx),
            editor_type_controls: Default::default(),
            scm: scm_view::ScmView::new(cx),
            timeline: timeline_view::TimelineView::new(cx),
            scm_graph: Default::default(),
            tree_views: Default::default(),
            decorations: Default::default(),
            quick_diffs: Default::default(),
            github_clients: Default::default(),
            command_icons: HashMap::new(),
            url_handlers: Default::default(),
            webview_views: Default::default(),
            extensions_viewlet: extensions_view::ExtensionsViewlet::new(cx),
            extension_editors: Vec::new(),
            menus: crate::menus::workbench_menus(),
            context_menu: None,
            manage_bounds: Rc::default(),
            context_menus_shown: 0,
            notifications: notifications_view::NotificationsUi::new(cx),
            status_entries: Vec::new(),
            hovers: hover::Hovers::default(),
            installed_menubar: None,
            profile_storage: Arc::new(Mutex::new(Storage::in_memory())),
            application_storage: Arc::new(Mutex::new(Storage::in_memory())),
            workspace_storage: Arc::new(Mutex::new(Storage::in_memory())),
            user_data_profiles: Default::default(),
            part_grid: part_grid::PartGrid::default(),
            zen_mode: Default::default(),
            activities: Default::default(),
            centered_layout: Default::default(),
            side_by_side_splits: Default::default(),
            editor_type_picker: Default::default(),
            edit_setting: Default::default(),
            editor_groups: Default::default(),
            auxiliary_windows: Default::default(),
            history: Default::default(),
            panel: panel::Panel::default(),
            explorer_panes: view_panes::ContainerPanes::default(),
            scm_panes: view_panes::ContainerPanes::default(),
            generated_panes: HashMap::new(),
            moved_view_panes: Default::default(),
            workspace_trust: Default::default(),
            resource_to_compare: None,
            clipboard_compare_count: 0,
            workspace_text_edits: Vec::new(),
            shared_storage: Arc::new(Mutex::new(Storage::in_memory())),
            recently_opened: Default::default(),
            dialogs: Vec::new(),
            lifecycle: Default::default(),
            debug: Default::default(),
            debug_view: debug_view::DebugView::new(cx),
            debug_console: debug_console::DebugConsole::new(cx),
            problems: problems_view::ProblemsView::new(cx),
            problems_quick_fixes: Default::default(),
            comments: comments::Comments::new(cx),
            problems_focus: cx.focus_handle(),
            refactor_preview: Default::default(),
            breakpoint_widget: None,
            testing: Default::default(),
            testing_view: Default::default(),
        };
        crate::main_thread_commands::register_commands_documentation(&mut workbench);
        workbench.editors.disposed.settings = Some(workbench.settings.clone());
        workbench.register_workbench_actions();
        workbench.register_notebook_commands();
        workbench.register_interactive_commands();
        workbench.register_repl_commands();
        workbench.register_diff_editor_commands();
        workbench.register_file_compare_commands();
        workbench.register_diff_gutter_commands();
        workbench.register_unchanged_region_commands();
        workbench.register_scroll_locking_commands();
        workbench.register_scm_view_mode_commands();
        workbench.register_merge_editor_commands();
        workbench.register_multi_diff_editor_commands();
        workbench.register_timeline_commands();
        workbench.register_scm_commands();
        workbench.register_scm_actions();
        workbench.register_text_file_commands();
        workbench.register_save_conflict_commands();
        workbench.register_cancel_operation_command();
        workbench.register_large_file_commands();
        workbench.register_media_preview_commands();
        workbench.register_image_carousel_commands();
        workbench.register_inspect_tokens_commands();
        workbench.register_word_wrap_commands();
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
        workbench.register_explorer_keyboard_commands();
        workbench.register_explorer_find_commands();
        workbench.register_type_navigation_commands();
        workbench.register_list_scroll_commands();
        workbench.register_search_actions();
        workbench.register_layout_actions();
        workbench.register_zen_mode_actions();
        workbench.register_centered_layout_actions();
        workbench.register_editor_group_actions();
        workbench.register_webview_actions();
        workbench.register_history_actions();
        workbench.register_search_editor_actions();
        workbench.register_search_editor_widget_actions();
        workbench.register_file_participant_commands();
        workbench.register_keybindings_editor_commands();
        workbench.register_settings_editor_commands();
        workbench.register_readonly_commands();
        workbench.register_preferences_commands();
        workbench.register_side_by_side_commands();
        workbench.register_editor_message_commands();
        workbench.register_content_hover_commands();
        workbench.register_suggest();
        workbench.register_settings_suggest_commands();
        workbench.register_parameter_hints();
        workbench.register_code_actions();
        workbench.register_code_lens();
        workbench.register_inlay_hints();
        workbench.register_color_detector();
        workbench.register_standalone_color_picker();
        workbench.register_inline_completions();
        workbench.register_language_detection();
        workbench.register_rename();
        workbench.register_links();
        workbench.register_linked_editing();
        workbench.register_language_status();
        workbench.update_markers_file_decorations();
        workbench.register_semantic_tokens();
        workbench.register_format();
        workbench.register_snippet_completions();
        workbench.register_snippet_actions();
        workbench.register_emmet();
        workbench.register_tab_completion();
        workbench.register_outline_actions();
        workbench.register_folding_providers();
        workbench.register_inlay_hints_accessibility();
        workbench.register_snippet_commands();
        workbench.register_marker_navigation_commands();
        workbench.register_quick_diff_commands();
        workbench.register_goto_symbol_commands();
        workbench.register_definition_link_commands();
        workbench.register_document_highlight_commands();
        workbench.register_document_symbol_commands();
        workbench.register_references_peek_commands();
        workbench.register_hierarchy_commands();
        workbench.register_define_keybinding_command();
        workbench.register_notifications_commands();
        workbench.register_output_actions();
        workbench.register_logs_actions();
        workbench.register_terminal_actions();
        workbench.register_problems_panel_view();
        workbench.register_output_panel_view();
        workbench.register_debug_console_panel_view();
        workbench.register_test_results_panel_view();
        workbench.register_terminal_panel_view();
        // mainThreadComments.ts registers the Comments container at order 10, after the others.
        workbench.register_comments_panel_view();
        workbench.register_refactor_preview();
        workbench.register_perf_view();
        workbench.register_accessibility_actions();
        workbench.start_system_events(cx);
        workbench.user_activity(cx);
        workbench.register_extension_management_commands();
        workbench.register_trusted_publisher_commands();
        workbench.register_extension_host_commands();
        workbench.register_user_data_profile_commands();
        workbench.register_profiles_editor_commands();
        workbench.register_settings_import_commands();
        workbench.register_workspace_trust_commands();
        workbench.register_workspace_trust_editor_commands();
        workbench.register_host_commands();
        workbench.register_workspace_commands();
        workbench.register_workspace_editing_commands();
        workbench.register_dialog_commands();
        workbench.register_lifecycle_commands();
        workbench.register_banner_commands();
        workbench.register_url_commands();
        workbench.register_local_history_commands();
        workbench.register_extensions_view_commands();
        workbench.register_view_focus_commands();
        workbench.register_move_view_actions();
        workbench.register_workspace_recommendations_commands();
        workbench.register_localization_commands();
        workbench.register_new_file_commands();
        workbench.register_welcome_page_commands();
        workbench.register_playground_commands();
        workbench.register_process_explorer_commands();
        workbench.register_auxiliary_window_commands();
        workbench.register_debug_commands();
        workbench.register_debug_console_commands();
        workbench.register_breakpoint_widget_commands();
        workbench.register_testing_commands();
        // Keybindings resolve before the focused element sees the key, and with nothing focused too.
        let entity = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            // Every window's keys come here: this workbench takes those of its own windows.
            if auxiliary_windows::window_workbench(window, cx).is_none_or(|owner| owner.entity_id() != entity.entity_id()) {
                return;
            }
            let _ = entity.update(cx, |workbench, cx| {
                workbench.user_activity(cx);
                workbench.dispatch_key(&event.keystroke, window, cx)
            });
        })
        .detach();
        cx.on_app_quit(|workbench, _| {
            workbench.store_explorer_view_state();
            workbench.store_editor_view_states();
            workbench.save_editor_part_state();
            workbench.store_editor_resolver_cache();
            async {}
        })
        .detach();
        cx.on_release(|workbench, _| {
            workbench.store_explorer_view_state();
            workbench.store_editor_view_states();
            workbench.store_editor_resolver_cache();
        })
        .detach();
        workbench.customize_theme();
        workbench
    }

    /// Switches to the profile's and the application's storage and reads the command history, the quick input
    /// position and the notification filters from them.
    pub fn restore_profile_state(&mut self, profile: Arc<Mutex<Storage>>, application: Arc<Mutex<Storage>>) {
        self.profile_storage = profile;
        self.application_storage = application;
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
            self.close_modal_editor(window, cx);
        }
        self.leave_side_group();
        self.leave_locked_group("file", &path);
        self.open_file_in_active_group(path, pinned, window, cx);
    }

    pub(super) fn open_file_in_active_group(&mut self, path: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        // The group of a side by side editor opens the file, not its secondary side's.
        self.leave_side_group();
        if path.to_string_lossy().ends_with(crate::search_editor::SEARCH_EDITOR_EXT) {
            return self.open_search_editor_file(path, pinned, window, cx);
        }
        if let Some(view_type) = self.custom_editor_for("file", &path) {
            let options = serde_json::json!({ "pinned": pinned, "preserveFocus": !pinned });
            drop(self.open_with_resolver(&exthost::scan::file_uri(&path), Some(&view_type), options, window, cx));
            return;
        }
        self.open_text_file(path, pinned, window, cx);
    }

    /// Opens or activates the file in the text editor, whatever editor its resource resolves to.
    pub(super) fn open_text_file(&mut self, path: PathBuf, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.editors.find(&path) {
            Some(index) => {
                self.editors.activate(index);
                if pinned {
                    self.editors.pin(index);
                }
            }
            // A model without an editor, as a search replace leaves it, keeps its text.
            None => match self.editors.models.iter().position(|model| model.document().is_file(&path)) {
                Some(index) => self.editors.show_model(index, !pinned),
                None => {
                    let document = self.read_file_editor_document(&path, false, false);
                    self.editors.open(document, !pinned);
                }
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
        if std::mem::take(&mut self.editors.closed_last_editor) {
            self.close_group_if_empty(self.editors.id);
        }
        self.editors.columns = if self.editors.has_several_groups() { self.groups_in_grid_order() } else { Vec::new() };
        for (group, path, index) in std::mem::take(&mut self.editors.closed) {
            self.remember_closed_editor(group, path, index);
        }
        self.record_navigation(None);
        self.editors.panel_editor_shown = self.output_view_visible() && self.output_editor().is_some();
        self.open_editors_focused_row = None;
        self.reveal_active_file();
        self.record_editor_history();
        self.capture_notebook_status_editor();
        self.drop_closed_search_editors();
        self.drop_closed_playground();
        self.load_search_editor_widget();
        self.find_editor_changed();
        self.schedule_keybinding_marks(cx);
        self.schedule_unsupported_settings_markers(cx);
        self.update_marker_decorations(None);
        self.code_actions_model_changed(cx);
        self.code_lens_model_changed(cx);
        self.sync_comment_widgets();
        self.refresh_commenting_ranges(false, cx);
        self.inlay_hints_model_changed(cx);
        self.color_detector_model_changed(cx);
        self.inline_completions_model_changed(cx);
        self.language_detection_model_changed(cx);
        self.links_model_changed(cx);
        self.linked_editing_reinitialize(true, cx);
        self.semantic_tokens_model_changed(cx);
        self.refresh_language_status(cx);
        self.sync_webview_panels();
        self.auto_lock_group();
        self.sync_editor_insets();
        self.text_files_editors_changed();
        self.leave_compact_mode();
        self.save_editor_part_state();
        self.close_auxiliary_windows_of_closed_parts(cx);
        self.show_editor_if_hidden(cx);
        self.send_notebook_changes();
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

    /// The command center's Go Back and Go Forward (`MenuId.CommandCenter` orders 1 and 2): enabled by their
    /// `canNavigateBack`/`canNavigateForward` precondition, pointer only then (actionbar.css `.disabled`).
    fn navigation_button(&self, icon: &'static str, title: &'static str, command: &'static str, enabled: bool, foreground: Rgba, disabled: Rgba, cx: &mut Context<Self>) -> Stateful<Div> {
        self.icon(icon, 22., 16., if enabled { foreground } else { disabled })
            .id(command)
            .rounded(px(4.))
            .when(enabled, |button| {
                button.cursor_pointer().hover(|button| button.bg(self.color("toolbar.hoverBackground"))).on_click(cx.listener(move |workbench, _, window, cx| {
                    workbench.execute_command(command, Vec::new(), window, cx);
                }))
            })
            .on_mouse_down(MouseButton::Right, cx.listener(show_command_center_context_menu))
            .child(self.action_hover(title, command, HoverOptions::instant()))
    }

    /// The window's title bar; an editor window's has its compact mode and always on top toggles in place of the
    /// layout controls, and its border. A compact one has no command center and shows the editor actions.
    fn title_bar(&self, viewport_width: f32, active: bool, editor_window: Option<auxiliary_windows::EditorWindowTitle>, cx: &mut Context<Self>) -> Div {
        let auxiliary = editor_window.is_some();
        let compact = editor_window.is_some_and(|state| state.compact);
        // titlebarPart.ts `updateStyles` and titlebarpart.css `.inactive`: inactive colors, the contents at 0.6 opacity.
        let foreground = match active {
            true => self.color("titleBar.activeForeground"),
            false => self.theme.color("titleBar.inactiveForeground").unwrap_or_else(|| self.color("foreground")),
        };
        let background = self.theme.opaque_on_workbench(if active { "titleBar.activeBackground" } else { "titleBar.inactiveBackground" }).or_else(|| self.theme.opaque_on_workbench("titleBar.activeBackground"));
        let opacity = if active { 1. } else { 0.6 };
        let command_center_visible = !compact && self.setting("window.commandCenter") != Value::Bool(false);
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
            .border_color(self.color(if active { "commandCenter.border" } else { "commandCenter.inactiveBorder" }))
            .text_color(if active { self.color("commandCenter.foreground") } else { foreground })
            .hover(|center| center.bg(self.color("commandCenter.activeBackground")).border_color(self.color("commandCenter.activeBorder")).text_color(self.color("commandCenter.activeForeground")))
            .text_size(px(12.))
            // windowTitle.ts `getCommandCenterLabel`: the workspace name after the title decorations.
            .child(self.title_decorated(self.workspace().map_or("Search".into(), Workspace::label)))
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
                        .child(self.navigation_button("arrow-left", "Go Back", "workbench.action.navigateBack", self.can_navigate(true), foreground, disabled, cx))
                        .child(self.navigation_button("arrow-right", "Go Forward", "workbench.action.navigateForward", self.can_navigate(false), foreground, disabled, cx).ml(px(4.)).mr(px(10.)))
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
        let center = div().absolute().top(px(6.5)).w_full().h(px(24.)).flex().justify_center().items_center().opacity(opacity).child(title);
        let side_bar_toggle = if self.side_bar_visible() { "layout-sidebar-left" } else { "layout-sidebar-left-off" };
        let layout_actions = [
            ("layout", "Customize Layout...", "workbench.action.customizeLayout"),
            (side_bar_toggle, "Toggle Primary Side Bar", "workbench.action.toggleSidebarVisibility"),
            (if self.is_part_visible(layout::Part::Panel) { "layout-panel" } else { "layout-panel-off" }, "Toggle Panel", "workbench.action.togglePanel"),
            (if self.is_part_visible(layout::Part::AuxiliaryBar) { "layout-sidebar-right" } else { "layout-sidebar-right-off" }, "Toggle Secondary Side Bar", "workbench.action.toggleAuxiliaryBar"),
        ];
        let right = div()
            .absolute()
            .top(px(7.5))
            .right(px(12.))
            .opacity(opacity)
            .flex()
            .gap(px(4.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!auxiliary, |right| right.children(self.notifications_title_bar_item(foreground, cx)))
            // titlebarPart.ts: a compact editor window shows `MenuId.CompactWindowEditorTitle` (a diff's Previous and
            // Next Change).
            .when(compact, |right| right.children(self.diff_title_actions(cx).into_iter().filter(|(order, _)| *order < 12.).map(|(_, action)| action)))
            .when_some(editor_window.filter(|_| layout_control_enabled), |right, state| right.children(self.editor_window_title_actions(state, foreground, cx)))
            .when(!auxiliary && layout_control_enabled, |right| right.children(layout_actions.map(|(name, title, command)| self.icon(name, 22., 16., foreground).cursor_pointer().child(self.action_hover(title, command, HoverOptions::instant())))))
            // titlebarPart.ts: with the activity bar on top or bottom, its global Manage action moves to the title bar.
            .when(!auxiliary && self.compact_activity_bar_location().is_some(), |right| right.child(self.icon("settings-gear", 22., 16., foreground).cursor_pointer().child(self.hover_target("manage-title", HoverContent::Text("Manage".into()), HoverOptions::instant()))));
        // NativeTitlebarPart.minimumHeight: DEFAULT_CUSTOM_TITLEBAR_HEIGHT with the command center, else macTitlebarSize.
        let height = match (command_center_visible, is_tahoe_or_newer()) {
            (true, _) => 35.,
            (false, true) => 32.,
            (false, false) => 28.,
        };
        // titlebarPart.ts `updateStyles`: an auxiliary title bar keeps `titleBar.border` under the modern UI.
        let border = self.theme.color("titleBar.border").filter(|_| auxiliary);
        div()
            .relative()
            .h(px(height))
            .flex_none()
            .when_some(background, |bar, background| bar.bg(background))
            .when_some(border, |bar, border| bar.border_b_1().border_color(border))
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

    /// The modern UI shell around the floating parts (floatingPanels.css), opaque as titlebarPart.ts sets it.
    fn shell_background(&self, active: bool) -> Rgba {
        let shell = if active { "modernUI.shellBackground" } else { "modernUI.inactiveShellBackground" };
        self.theme.opaque_on_workbench(shell).or_else(|| self.theme.opaque_on_workbench("modernUI.shellBackground")).unwrap_or_else(|| self.color("titleBar.activeBackground"))
    }

    /// windowTitle.ts `getFullWindowTitle` with the macOS `window.title` default
    /// `${activeEditorShort}${separator}${rootName}${separator}${folderLocation}${separator}${profileName}`, the last two
    /// empty here, or `nameLong` when it is empty, with the title decorations.
    pub(crate) fn window_title(&self) -> String {
        let root_name = self.workspace().map(Workspace::label).unwrap_or_default();
        let active_editor = self.editors.active().map(|editor| self.editor_name(editor)).unwrap_or_default();
        let parts: Vec<_> = [active_editor, root_name].into_iter().filter(|part| !part.is_empty()).collect();
        let title = match parts.is_empty() {
            true => menubar::NAME_LONG.to_string(),
            false => parts.join(" \u{2014} "),
        };
        self.title_decorated(title)
    }

    /// windowTitle.ts `getTitleDecorations`: "[Extension Development Host]" before the title in an extension
    /// development window.
    fn title_decorated(&self, title: String) -> String {
        match self.extension_development {
            true => format!("[Extension Development Host] {title}"),
            false => title,
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

    fn activity_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        // activitybarPart.ts: the hovers go away from the side bar's edge of the window.
        let left = self.arrangement().side_bar == layout::Position::Left;
        let hover = HoverOptions::composite_bar(if left { HoverPosition::Right } else { HoverPosition::Left });
        let inactive = self.color("activityBar.inactiveForeground");
        let containers = side_bar::VIEW_CONTAINERS.into_iter().filter(|(_, _, _, command, _)| !self.container_hidden_if_empty(command));
        let items: Vec<_> = containers.map(|(icon, _, title, command, view)| {
            let active = view.is_some_and(|view| self.side_bar_view == view) && self.side_bar_visible();
            let icon = if active { self.active_view_icon(icon) } else { self.icon(icon, 36., 24., inactive) };
            icon.id(command)
                .debug_selector(move || command.to_owned())
                .cursor_pointer()
                .child(self.action_hover(title, command, hover))
                .relative()
                .children(self.container_activity_badge(command))
                .when(view.is_some(), |icon| icon.on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.activity_item_clicked(active, command, event, window, cx))))
                // compositeBar.ts `CompositeBarDndCallbacks.drop` on an item: the view moves into that container.
                .when(command != "workbench.view.extensions", |icon| {
                    icon.on_drop(cx.listener(move |workbench, dragged: &view_panes::DraggedPane, _, cx| {
                        cx.stop_propagation();
                        workbench.drop_view_on_container(dragged.id, command, cx);
                    }))
                })
        }).collect();
        // floatingPanels.css and activitybarPart.ts: compact density narrows the lane to 4px and the item gap to 4px.
        let compact = self.compact_density();
        div()
            .w(px(if compact { 40. } else { 44. }))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(if compact { 1. } else { 3. }))
            .border_1()
            // activitybarPart.ts `updateStyles` with the modern UI: the inactive background while the window is not focused.
            .bg(self.color(if window.is_window_active() { "modernActivityBar.background" } else { "modernActivityBar.inactiveBackground" }))
            .map(|bar| match compact {
                // A left rail and a right rail after the side bar draw the seam beside them.
                true => bar.border_color(gpui::transparent_black()).child(self.compact_card_strokes("modernActivityBar.border", [true, true, true, left || self.side_bar_visible()], true)),
                false => bar
                    .border_color(self.color("modernActivityBar.border"))
                    .when(self.side_bar_visible() && left, |bar| bar.rounded_l(px(8.)))
                    .when(self.side_bar_visible() && !left, |bar| bar.rounded_r(px(8.)))
                    .when(!self.side_bar_visible(), |bar| bar.rounded(px(8.))),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(if compact { 4. } else { 8. }))
                    .children(items)
                    .children(self.testing_activity_item(hover, inactive, cx))
                    .children(self.contributed_activity_items(hover, inactive, cx))
                    .children(self.generated_activity_items(hover, inactive, cx)),
            )
            // compositeBar.ts `CompositeBarDndCallbacks.drop`: a view dropped on the bar moves to a new side bar entry.
            .on_drop(cx.listener(|workbench, dragged: &view_panes::DraggedPane, _, cx| workbench.drop_view_on_location(dragged.id, layout::Part::SideBar, cx)))
            .child(
                self.icon(self.manage_activity_icon(), 36., 24., inactive)
                    .cursor_pointer()
                    .children(self.profile_badge())
                    .child(output_view::bounds_tracker(self.manage_bounds.clone()))
                    .child(self.hover_target("manage", HoverContent::Text(self.manage_activity_title()), hover))
                    // globalCompositeBar.ts: a left mouse down opens MenuId.GlobalActivity beside the button.
                    .on_mouse_down(MouseButton::Left, cx.listener(|workbench, _, window, cx| {
                        let context = workbench.key_context(window);
                        let actions = crate::menus::menu_actions(&workbench.menus, MenuId::GlobalActivity, &context);
                        workbench.show_context_menu_beside(actions, &context, workbench.manage_bounds.get(), window, cx);
                    })),
            )
    }

    /// The Testing container shows once a controller registers (testing.contribution.ts `TestingContextKeys.providerCount`).
    fn testing_activity_item(&self, hover: HoverOptions, inactive: Rgba, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        if self.testing.controllers.is_empty() {
            return None;
        }
        let active = self.side_bar_view == SideBarView::Testing && self.side_bar_visible();
        let icon = if active { self.active_view_icon("beaker") } else { self.icon("beaker", 36., 24., inactive) };
        let command = "workbench.view.testing";
        Some(icon.id(command).cursor_pointer().child(self.action_hover("Testing", command, hover)).on_click(cx.listener(move |workbench, event: &ClickEvent, window, cx| workbench.activity_item_clicked(active, command, event, window, cx))))
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

    /// style.css `.hc-black .monaco-list:not(.element-focused):focus:before`: in high contrast a focused list
    /// without a focused row is outlined in focusBorder, above its sticky rows.
    fn high_contrast_list_outline(&self, list_focused: bool, has_focused_row: bool) -> Option<Div> {
        let color = self.theme.color("focusBorder").filter(|_| self.theme.kind.is_high_contrast() && list_focused && !has_focused_row)?;
        Some(div().absolute().inset_0().border_1().border_color(color))
    }

    /// The Explorer: title, then the folder's view with its tree, Outline and Timeline as panes.
    fn side_bar_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let foreground = self.foreground_or("sideBar.foreground");
        if let SideBarView::Contributed(id) = self.side_bar_view {
            return self.contributed_side_bar(id, window, cx);
        }
        if let SideBarView::Generated(id) = self.side_bar_view {
            let title = self.generated_container_view(id).map_or("", |view| view.name);
            return self.view_pane_container(view_panes::PaneContainer::Generated(id), title, Vec::new(), window, cx).child(div().h(px(5.)).flex_none());
        }
        if self.side_bar_view == SideBarView::Scm {
            return self.scm_side_bar(window, cx);
        }
        if self.side_bar_view == SideBarView::Extensions {
            let title = self.side_bar_title_text(self.extensions_title()).child(self.extensions_title_actions(cx));
            return self.side_bar_frame().text_color(foreground).child(title).child(self.extensions_view(window, cx));
        }
        if self.side_bar_view == SideBarView::Search {
            let container = self.view_pane_container(view_panes::PaneContainer::Search, "Search", vec![self.search_view_pane(window, cx)], window, cx);
            let merged = self.panes(view_panes::PaneContainer::Search).view.as_ref().is_some_and(|view| view.ids().len() == 1);
            return container.when(!merged, |container| container.child(div().h(px(5.)).flex_none()));
        }
        if self.side_bar_view == SideBarView::Debug {
            return self.debug_side_bar(window, cx);
        }
        if self.side_bar_view == SideBarView::Testing {
            return self.testing_side_bar(cx).children(self.testing_contributed_sections(window, cx));
        }
        let has_folder = self.folder.as_ref().is_some_and(|tree| !tree.workspace.folders.is_empty());
        self.explorer_view_container(window, cx)
            // shortcut: extension webview views sit below the Explorer's panes, not in view_panes.rs; milestone 11 moves them in.
            .when(has_folder, |container| container.children(self.webview_view_panes(cx)))
            // The pane view is laid out 5px shorter than the side bar content.
            .child(div().h(px(5.)).flex_none())
    }

    /// The folder's view with its tree, or the empty window's view.
    pub(in crate::workbench) fn folder_view_pane(&self, window: &Window, cx: &mut Context<Self>) -> view_panes::ViewPane {
        // explorerViewlet.ts: the empty view shows while `workspaceFolderCount` is 0, in an empty workspace too.
        let Some(tree) = self.folder.as_ref().filter(|tree| !tree.workspace.folders.is_empty()) else {
            return view_panes::ViewPane { id: view_panes::EMPTY_VIEW, title: "No Folder Opened".into(), description: None, actions: None, show_actions: false, body: self.empty_view(window, cx).into_any_element() };
        };
        let foreground = self.foreground_or("sideBar.foreground");
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
        let active: Vec<usize> = tree_rows.iter().enumerate().filter(|(_, row)| tree.focused.as_ref() == Some(&row.path) || tree.selected.contains(&row.path)).map(|(index, _)| index).collect();
        let guides = self.tree_guides(list_scroll::ListId::Explorer, tree_rows.iter().map(|row| row.depth).collect(), &active, window);
        // abstractTree.ts `AbstractFindController`: its 30px paddingTop keeps the sticky rows away while find is open.
        let sticky = if self.explorer.find.is_open() { Vec::new() } else { self.tree_sticky_rows(list_scroll::ListId::Explorer, &tree_rows.iter().map(|row| row.depth).collect::<Vec<_>>()) };
        let sticky_rows: Vec<_> = sticky.iter().map(|sticky| (sticky.index, tree_rows[sticky.index].clone())).collect();
        // abstractTree.ts `StickyScrollWidget` toggles only `focused` on its rows, never `selected`.
        let mut render_row = |(index, row): (usize, crate::explorer::Row), sticky: bool| {
            let row_guides = guides.row(index, 0.);
            let twistie = match (row.is_dir || row.has_nests, row.expanded) {
                (true, true) => Some("chevron-down"),
                (true, false) => Some("chevron-right"),
                (false, _) => None,
            };
            let navigation = tree.navigation(&row);
            // explorerViewer.ts `ExplorerAccessibilityProvider.getAriaLabel`: the name, a compact row's names joined.
            let aria_label = match row.compressed.is_empty() {
                true => row.name.clone(),
                false => row.compressed.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join("/"),
            };
            let (aria_level, aria_expanded) = (row.depth, (row.is_dir || row.has_nests).then_some(row.expanded));
            let path = row.path;
            let is_dir = row.is_dir;
            let is_edited = edited.as_ref().is_some_and(|renamed| match renamed {
                Some(renamed) => *renamed == path || row.compressed.iter().any(|(folder, _)| folder == renamed),
                None => row.is_new,
            });
            let is_selected = !sticky && !row.is_new && tree.selected.contains(&path);
            let is_focused = !sticky && !row.is_new && tree.focused.as_ref() == Some(&path);
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
            let indent = self.tree_indent() * (row.depth - 1) as f32;
            // explorerViewer.ts passes `explorer.decorations` as the label's `fileDecorations`.
            let decoration = self.file_decoration(&path, is_dir);
            let decoration_color = self.decoration_color(decoration.as_ref(), self.setting("explorer.decorations.colors") != Value::Bool(false));
            let badge = self.decoration_badge(decoration.as_ref(), foreground).filter(|_| self.setting("explorer.decorations.badges") != Value::Bool(false));
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
            let hover = self.hover_target(("explorer-hover", index), HoverContent::Text(match decoration.as_ref().and_then(|decoration| decoration.tooltip.as_deref()) {
                // labels.ts: the title, then the decoration's tooltip.
                Some(tooltip) => format!("{} \u{2022} {tooltip}", crate::quick_input::path_label(&path, None)),
                None => crate::quick_input::path_label(&path, None),
            }), HoverOptions::mouse());
            div()
                .id(index)
                .role(gpui::Role::TreeItem)
                .aria_label(aria_label)
                .aria_level(aria_level)
                .aria_selected(is_selected)
                .when_some(aria_expanded, |row, expanded| row.aria_expanded(expanded))
                // listWidget.ts: the list keeps the focus and points aria-activedescendant at the focused row.
                .when(is_focused, |row| row.aria_active_descendant())
                // list.css: .monaco-list.mouse-support .monaco-list-row is cursor: pointer.
                .cursor_pointer()
                .relative()
                .when(!row.is_new && !is_edited, |row| row.child(hover))
                .h(px(22.))
                .mx(px(self.pane_row_inset()))
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
                .children(badge)
                .when_some(decoration_color, |row, color| row.text_color(color))
                // explorerviewlet.css: .explorer-item.cut is at half opacity. Its 0.3 for the other rows while a name is
                // edited needs `.explorer-folders-view` twice in the ancestry, which the reference DOM never has.
                .children(row_guides)
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
                        let mode = workbench.list_click_mode();
                        let multi_select = if mode.alt_multi_select { modifiers.alt } else { modifiers.platform };
                        let double_click = event.click_count() == 2;
                        window.focus(&workbench.explorer.focus.clone(), cx);
                        let tree = workbench.folder.as_mut().expect("rows exist only with a folder");
                        tree.click_select(&path, multi_select, modifiers.shift);
                        cx.notify();
                        if multi_select || modifiers.shift {
                            return;
                        }
                        match is_dir {
                            true if mode.expand_on_double_click && !double_click => {}
                            true => tree.toggle(&path),
                            false if mode.open_on_double_click && !double_click => {}
                            false => workbench.open_file(path.clone(), double_click, window, cx),
                        }
                    }))
                })
        };
        let rows: Vec<_> = tree_rows.into_iter().enumerate().map(|row| render_row(row, false)).collect();
        let sticky_elements = sticky_rows.into_iter().map(|row| render_row(row, true).into_any_element()).collect();
        let sticky_container = self.tree_sticky_container(list_scroll::ListId::Explorer, &sticky, sticky_elements, cx);
        // explorerView.ts `setEditable`: no horizontal scrolling while a name is edited.
        let horizontal = self.horizontal_scrolling(list_scroll::ListId::Explorer) && self.explorer.edit.is_none();
        let explorer_focused = self.explorer.focus.is_focused(window);
        view_panes::ViewPane { id: view_panes::FILE_VIEW, title: folder_name, description: None, actions: None, show_actions: explorer_focused, body:
                div().id("explorer-pane").relative().flex_1().min_h_0().flex().flex_col()
                .child(
                div()
                    .id("explorer-tree")
                    .role(gpui::Role::Tree)
                    .aria_label("Files Explorer")
                    .track_focus(&self.explorer.focus)
                    .overflow_y_scroll()
                    .when(horizontal, |tree| tree.overflow_x_scroll())
                    .track_scroll(&self.explorer.scroll)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Right, cx.listener(|workbench, event: &MouseDownEvent, window, cx| workbench.show_explorer_context_menu(None, event.position, window, cx)))
                    // abstractTree.ts `AbstractFindController.open`: `paddingTop: 30` while the find widget shows.
                    .when(self.explorer.find.is_open(), |tree| tree.pt(px(30.)))
                    // `horizontalScrolling`: unwrapped rows as wide as the widest, at least the view.
                    .child(div().flex_none().flex().flex_col().when(horizontal, |rows| rows.self_start().min_w_full().whitespace_nowrap()).children(rows))
                    .child(self.explorer_empty_area(cx)),
                ).child(self.list_scroll_layer(list_scroll::ListId::Explorer, cx)).children(sticky_container).children(self.high_contrast_list_outline(tree_focused, tree.focused.is_some())).children(self.explorer_find_widget(window, cx)).into_any_element() }
    }

    /// The Explorer's `ViewPaneContainer`: Open Editors while shown, the folder's or the empty window's view, Outline
    /// and Timeline, the ones still in it (`layout_view_panes` leaves out the moved ones).
    fn explorer_view_container(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut panes = vec![self.folder_view_pane(window, cx), self.outline_view_pane(window, cx), self.timeline_pane(window, cx)];
        if self.explorer_panes.view.as_ref().is_some_and(|view| view.ids().contains(&view_panes::OPEN_EDITORS_VIEW)) {
            panes.push(self.open_editors_pane(window, cx));
        }
        self.view_pane_container(view_panes::PaneContainer::Explorer, "Explorer", panes, window, cx)
    }

    fn side_bar_frame(&self) -> Div {
        div().flex_1().min_h_0().flex().flex_col()
    }

    /// The composite title bar: the view's name and its actions.
    fn side_bar_title(&self, name: &'static str) -> Div {
        self.side_bar_title_text(name.to_owned())
    }

    fn side_bar_title_text(&self, name: String) -> Div {
        div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(4.))
            .when_some(self.theme.color("sideBarTitle.border"), |title, border| title.border_b_1().border_color(border))
            // The h2 keeps its 32px line under the HC border-bottom, so it is laid out from the top, not centered.
            // part.css `.title-label h2`: ellipsis where `.title-actions` (8px padding-left) takes the room.
            .child(div().self_start().min_w_0().truncate().pl(px(8.)).line_height(px(32.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(self.foreground_or("sideBarTitle.foreground")).child(crate::nls::localize(&name).into_owned()))
    }

    /// The letterpress of an empty group, `size` wide (256px, 200px in a part of several groups).
    fn watermark(&self, size: f32) -> Div {
        // The high contrast letterpress files paint their own fill; the others are black at the opacity the file sets (editorgroupview.css).
        let (file, color) = match self.theme.kind {
            ThemeKind::Dark => ("letterpress-dark.svg", rgb(0x000000)),
            ThemeKind::Light => ("letterpress-light.svg", rgb(0x000000)),
            ThemeKind::HighContrastDark => ("letterpress-hcDark.svg", rgb(0x3c3c3c)),
            ThemeKind::HighContrastLight => ("letterpress-hcLight.svg", rgb(0xd9d9d9)),
        };
        let path = self.app_root.join("src/vs/workbench/browser/parts/editor/media").join(file);
        // .watermark-container is 280px tall and centered; the letterpress sits at its top.
        div().h(px(size + 24.)).child(svg().external_path(path.to_string_lossy().into_owned()).size(px(size)).text_color(color))
    }

    fn status_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let (foreground, background) = match self.folder {
            _ if self.statusbar_in_debug_mode() => (self.color("statusBar.debuggingForeground"), self.color("statusBar.debuggingBackground")),
            Some(_) => (self.color("statusBar.foreground"), self.color("statusBar.background")),
            None => (self.color("statusBar.noFolderForeground"), self.color("statusBar.noFolderBackground")),
        };
        // statusbarpart.css: pointer only on items with a command; status.message has none.
        let problems = self.problems_status_item(foreground, cx);
        let height = self.status_bar_height();
        // floatingPanels.css: the edge labels' padding is 6px, 4px in compact density, around the 16px icon.
        let edge_item = if self.compact_density() { 24. } else { 28. };
        div()
            .relative()
            .h(px(height))
            .flex_none()
            .track_focus(&self.status_bar_focus)
            .flex()
            .justify_between()
            .items_center()
            .px(px(if self.compact_density() { 8. } else { 12. }))
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
            .child(div().h(px(height - 4.)).flex().child(self.icon("remote", edge_item, 16., foreground).h_full().cursor_pointer().child(self.hover_target("status.host", HoverContent::Text("Open a Remote Window".into()), HoverOptions::status_bar()))).children(self.restricted_mode_status_item(foreground, cx)).children(self.notifications_status_item(true, foreground, cx)).child(self.extension_status_items(true, |priority| priority > PROBLEMS_PRIORITY, foreground, cx)).child(problems).child(self.extension_status_items(true, |priority| priority <= PROBLEMS_PRIORITY, foreground, cx)).children(self.status_bar_message()).children(self.window_progress_entry(cx.background_executor().now()).map(|entry| self.extension_status_item(&entry, foreground, cx))).children(self.terminal_recording_status_entry().map(|entry| self.extension_status_item(&entry, foreground, cx))))
            .child(div().h(px(height - 4.)).flex().child(self.extension_status_items(false, |priority| priority > EDITOR_STATUS_PRIORITY, foreground, cx)).child(self.editor_status_items(foreground, window, cx)).child(self.extension_status_items(false, |priority| priority <= EDITOR_STATUS_PRIORITY, foreground, cx)).children(self.notifications_status_item(false, foreground, cx)))
    }

    /// The `status.message` entry: multi-chord mode, else the latest status message (notificationsStatus.ts).
    fn status_bar_message(&self) -> Option<Div> {
        let message = self.chord_status().or_else(|| self.status_message.as_ref().map(|(message, _)| message.clone()))?;
        let (margin, padding) = self.status_item_spacing();
        Some(div().h_full().ml(px(margin)).px(px(padding)).flex().items_center().child(message))
    }

    /// A codicon centered in a square box, or the product icon theme's glyph for it. Drawn as SVG: Wu's text system
    /// skips fonts without an 'm' glyph, so icon fonts cannot load (gpui_macos text_system.rs load_family).
    fn icon(&self, name: &str, box_size: f32, icon_size: f32, color: Rgba) -> Div {
        self.turned_icon(name, box_size, icon_size, color, 0.)
    }

    /// An icon rotated by `turns` of a full turn, clockwise.
    fn turned_icon(&self, name: &str, box_size: f32, icon_size: f32, color: Rgba, turns: f32) -> Div {
        let ids = crate::codicons::icon_ids(name);
        let icon = match self.product_icons.as_ref().and_then(|theme| theme.svg(&ids, icon_size)) {
            Some(glyph) => svg().data(glyph.as_bytes()),
            None => {
                let codicon = ids.last().expect("icon_ids starts with the id");
                svg().external_path(crate::codicons::svg_path(&self.app_root, crate::symbol_icons::svg_name(codicon)).to_string_lossy().into_owned())
            }
        };
        div()
            .relative()
            .size(px(box_size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(icon.size(px(icon_size)).text_color(color).with_transformation(gpui::Transformation::rotate(gpui::percentage(turns))))
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../port/fixtures/workspace");
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let host = HostSync::new(std::sync::mpsc::channel().0);
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host, cx));
        for _ in 0..2 {
            workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("workbench.action.inspectKeyMappingsJSON", Vec::new(), window, cx)));
        }
        cx.run_until_parked();

        let editors = workbench.read_with(cx, |workbench, _| {
            workbench.editors.editors.iter().map(|editor| (editor.document().uri_string(), editor.preview, editor.document().lines[..2].join("\n"))).collect::<Vec<_>>()
        });
        let expected = |name: &str| (format!("untitled:{name}"), false, "{\n\t\"layout\": {".to_string());
        assert_eq!(editors, [expected("Untitled-1"), expected("Untitled-2")]);
    }

    #[gpui::test]
    fn keys_and_typed_text_edit_the_focused_editor_and_reach_the_host(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let text = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"));

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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        assert_eq!(workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n")), "ab  \ncd\n\n\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn in_tab_focus_mode_tab_moves_focus_to_the_status_bar_instead_of_indenting(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
                (editor.document().lines.join("\n"), editor.highlights.len(), workbench.status_bar_focus.is_focused(window), moves_focus)
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
    fn folding_the_cursor_line_moves_the_cursor_to_the_fold_start(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../port/fixtures/folding/long.py");
        // A host that is up, as in the app: no extension declares the `onCommand` events of the keys.
        let (sender, host_receiver) = std::sync::mpsc::channel();
        let mut host = HostSync::new(sender);
        host.installed_extensions_registered = true;
        let (workbench, cx) = cx.add_window_view(|window, cx| {
            let mut workbench = Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host, cx);
            workbench.load_keybindings(crate::keybindings::default_keybindings(&[]), std::env::temp_dir().join("code-gpui-fold-cursor-keybindings.json"), cx);
            workbench.open_file(file.clone(), true, window, cx);
            workbench
        });
        cx.run_until_parked();
        // The keys of the pixel-check "folded" window, dispatched like `--screenshot-keys` in main.rs.
        cx.update(|window, cx| {
            for key in "cmd-down up alt-cmd-[ up up up up up up up up up up up up alt-cmd-[".split(' ') {
                window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx);
            }
        });
        // The host answers each event on its own turn, in the order it was asked.
        for message in host_receiver.try_iter().filter(|message| message.method == "$activateByEvent") {
            message.reply.unwrap().send(Ok(serde_json::Value::Null)).unwrap();
            cx.run_until_parked();
        }
        let (hidden, head) = workbench.read_with(cx, |workbench, _| {
            let editor = workbench.editors.active().unwrap();
            (editor.folding.hidden().to_vec(), editor.selection().head)
        });
        assert_eq!((hidden, head.line, head.column), (vec![75..89, 99..101], 74, 25), "`def merge` and `if __name__` folded, cursor on line 75");
    }

    #[gpui::test]
    fn selection_anchor_keys_select_from_the_anchor(cx: &mut TestAppContext) {
        use crate::editor::{Position, Selection};
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
            assert_eq!(editor.document().lines, ["Port  for", "the edinotestor"]);
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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let lines = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.clone());

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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let lines = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.clone());

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
        let app_root = Path::new(env!("VSCODE_SRC")).to_path_buf();
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
        let text = workbench.read_with(cx, |workbench, _| workbench.editors.active().unwrap().document().lines.join("\n"));
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
