//! The native app: the workbench, themed from the built-in theme files, over the running extension host.
//! Usage: vsrustium [folder or .code-workspace file [file]] [--theme <settings id>] [--user-data-dir <dir>] [--screenshot <png>]
//! [--screenshot-keys "<keystroke> ..."] [--screenshot-input <steps>] [--node] [--export-default-keybindings [<folder>]]
//!   [--extensionDevelopmentPath=<dir> [--extensionTestsPath=<path>]]: the app exits with the test runner's code.

#[cfg_attr(not(test), expect(dead_code, reason = "the code action menu of milestone 13 is its first desktop caller"))]
mod action_list;
mod accessibility_signals;
mod anything_quick_access;
mod backups;
mod brackets;
mod bulk_edit_preview;
mod clipboard;
mod codicons;
mod command_palette;
mod commands;
mod custom_tree;
mod decorations;
mod contextkey;
mod debug;
mod default_settings;
mod diff_editor;
mod dialog;
mod file_dialog;
mod editor;
mod editor_grid;
mod navigation;
mod editor_history;
mod explorer;
mod extension_enablement;
mod extension_recommendations;
mod extensions_model;
mod file_actions;
mod file_icons;
mod filters;
mod history;
mod fuzzy_scorer;
mod hierarchy_model;
mod host_sync;
mod input_box;
mod keybinding_editing;
mod keybindings;
mod keybindings_editor;
mod keyboard_layout;
mod keyboard_mapper;
mod system_wide_keybindings;
mod language_features;
mod languages;
mod launch;
mod list_tree;
mod link_computer;
mod local_history;
mod main_thread;
mod main_thread_commands;
mod main_thread_github;
mod main_thread_errors;
mod main_thread_message;
mod main_thread_output;
mod main_thread_progress;
mod main_thread_quick_open;
mod main_thread_status_bar;
mod markdown;
mod markers;
mod markers_filter;
mod menus;
mod merge_editor;
mod multi_diff_editor;
mod nls;
mod outline_model;
mod metered_connection;
mod notifications;
mod output;
mod perf;
mod power;
// The process explorer (milestone 19 handoff) is its first caller; until then only its tests build it.
mod ps;
mod product;
mod profile_template;
mod timer;
mod walkthroughs;
mod policy;
mod quick_input;
mod recently_opened;
mod references_model;
mod search_editor;
mod scm;
mod scm_history;
mod search_model;
mod semantic_tokens;
mod default_document_colors;
mod ghost_text;
mod language_detection;
mod settings_editor;
mod shell_env;
mod settings_import;
mod smart_snippet_inserter;
mod snippets;
mod split_view;
mod symbol_kinds;
mod suggest;
mod symbol_icons;
mod syntax;
mod tasks;
mod terminal;
mod testing;
mod tf_idf;
mod theme;
mod timeline;
mod trusted_domains;
mod undo_redo;
mod untitled;
mod variables;
mod webview;
mod uri;
mod user_data_profile;
mod windows_state;
mod workbench;
mod workspace_trust;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::configuration::Overrides;
use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::extension_development::ExtensionDevelopment;
use exthost::extension_management::{UserExtensions, WindowExtensions, add_user_extensions};
use exthost::main_thread_configuration::MainThreadConfiguration;
use exthost::main_thread_files::MainThreadFiles;
use exthost::main_thread_secret_state::MainThreadSecretState;
use exthost::main_thread_storage::MainThreadStorage;
use exthost::main_thread_workspace::MainThreadWorkspace;
use exthost::rpc::Arg;
use exthost::scan::{ScannedExtension, scan_builtin_extensions};
use exthost::session::{ForwardedRequest, Session, SessionPaths, init_data, workspace_data};
use exthost::{ExtensionHost, HostOptions, Runtime};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Bounds, Keystroke, TitlebarOptions, Window, WindowBounds, WindowOptions, point, prelude::*, px, size};
use gpui_platform::application;
use serde_json::json;
use storage::state::FileStorage;
use anyhow::Context as _;
use host_sync::HostSync;
use languages::Languages;
use main_thread::{HostRequest, WorkbenchMainThread};
use theme::load_theme;
use workbench::Workbench;

/// The flags followed by a value; the other arguments without `--` are the folder and the file.
pub const VALUE_FLAGS: [&str; 11] = ["--profile", "--user-data-dir", "--shared-data-dir", "--extensions-dir", "--locale", "--screenshot", "--screenshot-keys", "--screenshot-input", "--theme", "--open-url", "--export-default-keybindings"];

/// The indexes of the folder and file arguments: those that are neither a flag nor a flag's value.
pub(crate) fn positional_indexes(args: &[String]) -> Vec<usize> {
    (1..args.len()).filter(|&index| !args[index].starts_with("--") && !VALUE_FLAGS.contains(&args[index - 1].as_str())).collect()
}

fn main() -> anyhow::Result<()> {
    perf::mark("code/didStartMain");
    let args: Vec<String> = std::env::args().collect();
    let runtime = if args.iter().any(|arg| arg == "--node") { Runtime::Node } else { Runtime::Bun };
    let flag_value = |flag: &str| args.iter().position(|arg| arg == flag).and_then(|i| args.get(i + 1)).cloned();
    let user_data = match flag_value("--user-data-dir") {
        Some(dir) => PathBuf::from(dir),
        None => default_user_data()?,
    };
    std::fs::create_dir_all(&user_data).with_context(|| format!("creating {}", user_data.display()))?;
    let socket = launch::socket_path(&user_data);
    if launch::send_to_running_instance(&socket, &args, &std::env::current_dir()?) {
        return Ok(());
    }
    let launches = launch::listen(&socket);
    let screenshot = flag_value("--screenshot").map(PathBuf::from);
    let screenshot_input = flag_value("--screenshot-input").unwrap_or_default();
    // Typed before the capture, as port/tools/ref-screenshot types KEYS into the reference.
    let screenshot_keys: Vec<Keystroke> = flag_value("--screenshot-keys").unwrap_or_default().split_whitespace().map(Keystroke::parse).collect::<Result<_, _>>()?;
    let positional: Vec<&String> = positional_indexes(&args).into_iter().map(|index| &args[index]).collect();
    // windowsMainService.ts keeps the path as given (no realpath), so /var stays /var on macOS.
    let resolve = |path: &String| std::fs::metadata(path).and_then(|_| std::path::absolute(path)).with_context(|| format!("opening {path}"));
    let mut paths = positional.iter().map(|path| resolve(path)).collect::<anyhow::Result<Vec<_>>>()?.into_iter().peekable();
    // windowsMainService.ts: a file path alone opens an empty window with the file.
    let opens_workspace = paths.peek().is_some_and(|path| path.is_dir() || path.extension().is_some_and(|extension| extension == config::workspace::WORKSPACE_EXTENSION));
    let workspace = paths.next_if(|_| opens_workspace).map(Workspace::open);
    let file = paths.next();
    let development = ExtensionDevelopment::from_args(&args)?;
    // shortcut: the port is no app bundle, so no CFBundleURLTypes routes `code-oss://` here; `--open-url <url>` does.
    let open_url = flag_value("--open-url");
    apply_shell_env(args.iter().any(|arg| arg == "--force-disable-user-env"));

    // Packaged as VSRustium.app (port/tools/package-app) the app files sit in Contents/Resources/app; run from the
    // checkout they are the working directory.
    let bundled = std::env::current_exe()?.parent().and_then(|macos| macos.parent()).map(|contents| contents.join("Resources/app")).filter(|app| app.join("product.json").is_file());
    let working_dir = std::env::current_dir()?;
    let app_root = bundled.unwrap_or_else(|| working_dir.clone());
    product::load(&app_root);
    // main.ts: `--export-default-keybindings [folder]`, a folder relative to the working directory, else `<appRoot>/doc`.
    let export_default_keybindings = args.iter().any(|arg| arg == "--export-default-keybindings").then(|| {
        flag_value("--export-default-keybindings").filter(|folder| !folder.starts_with("--")).map_or_else(|| app_root.join("doc"), |folder| working_dir.join(folder))
    });
    // Before any scan: the display language decides the manifests' messages.
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let locale = nls::user_locale(flag_value("--locale").as_deref(), &output::log_levels::argv_file(&home));
    perf::mark("code/willGenerateNls");
    let resolved = exthost::language_packs::resolve_nls(&user_data, &locale);
    nls::init(&resolved);
    exthost::language_packs::set_nls(&user_data, resolved);
    perf::mark("code/didGenerateNls");
    let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
    perf::mark("code/willLoadExtensions");
    let mut builtins = scan_builtin_extensions(&extensions_dir)?;
    perf::mark("code/didScanExtensions");
    builtins.extend(exthost::scan::scan_dev_system_extensions(&app_root)?);
    // Artur: user extensions live in ~/.vsrustium/extensions, the default user data dir's extensions folder.
    let extensions_folder = flag_value("--extensions-dir").map_or_else(|| user_data.join("extensions"), PathBuf::from);
    UserExtensions::new(extensions_folder.clone(), VERSION).clean_up()?;
    // storageMainService.ts: no storage during extension tests, so each test run starts empty.
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    let shared_storage = match extension_tests {
        true => Arc::new(Mutex::new(storage::Storage::in_memory())),
        false => Arc::new(Mutex::new(open_shared_storage(flag_value("--shared-data-dir").map(PathBuf::from))?)),
    };
    // windowsMainService.ts: a start without paths, or any start with `preserve`, opens the last session's windows.
    let restored = match (development.is_some() || screenshot.is_some() || export_default_keybindings.is_some() || open_url.is_some(), positional.is_empty()) {
        (true, _) => Vec::new(),
        (false, no_paths) => {
            let user_settings = Settings::load(Registry::with_extensions(&[]), user_data.join("User"), None);
            let setting = user_settings.get_value(Some("window.restoreWindows"), &Overrides::default()).and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_else(|| "all".into());
            let restore_fullscreen = user_settings.get_value(Some("window.restoreFullscreen"), &Overrides::default()) == Some(json!(true));
            let (last_active, opened) = windows_state::load(&user_data);
            let mut windows = match no_paths || setting == "preserve" {
                true => windows_state::to_restore(&setting, last_active, opened),
                false => Vec::new(),
            };
            // windowsStateHandler.ts `getNewWindowState`: fullscreen comes back only with `window.restoreFullscreen`.
            for ui in windows.iter_mut().filter_map(|window| window.ui.as_mut()).filter(|ui| ui.mode == windows_state::FULLSCREEN && !restore_fullscreen) {
                ui.mode = windows_state::NORMAL;
            }
            windows
        }
    };
    let app = AppState { args: args.clone(), runtime, user_data, app_root, builtins, user_extensions_folder: extensions_folder, shared_storage };
    let startup = (!positional.is_empty() || restored.is_empty()).then(|| WindowRequest { workspace, file, development, open_url, screenshot, screenshot_keys, screenshot_input, export_default_keybindings, ..WindowRequest::default() });
    let application = application();
    // A click on the Dock icon without a window opens an empty one, as `app.on('activate')` does in app.ts.
    application.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_window_or_log(WindowRequest::default(), cx);
        }
    });
    application.run(move |cx: &mut App| {
        perf::mark("code/mainAppReady");
        cx.set_global(app);
        for window in restored {
            let workspace = window.workspace.clone().or(window.folder.clone()).map(Workspace::open);
            open_window_or_log(WindowRequest { workspace, backup_folder: window.backup_path, ui: window.ui, ..WindowRequest::default() }, cx);
        }
        if let Some(startup) = startup
            && let Err(error) = open_window(startup, cx)
        {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        workbench::menubar::install(cx);
        if let Some(launches) = launches {
            launch::handle_launches(launches, cx);
        }
        cx.activate(true);
    });
    Ok(())
}

/// What every window shares: the command line, the folders, the built-in extensions scanned at startup and the
/// application storage.
pub(crate) struct AppState {
    args: Vec<String>,
    runtime: Runtime,
    pub user_data: PathBuf,
    app_root: PathBuf,
    builtins: Vec<ScannedExtension>,
    user_extensions_folder: PathBuf,
    shared_storage: Arc<Mutex<storage::Storage>>,
}

impl gpui::Global for AppState {}

/// A window to open: its folder or workspace and file; the first window also takes the command line's extension
/// development, URL, capture and export flags.
#[derive(Default)]
pub(crate) struct WindowRequest {
    pub workspace: Option<Workspace>,
    pub file: Option<PathBuf>,
    development: Option<ExtensionDevelopment>,
    open_url: Option<String>,
    screenshot: Option<PathBuf>,
    screenshot_keys: Vec<Keystroke>,
    screenshot_input: String,
    export_default_keybindings: Option<PathBuf>,
    /// The empty window backup folder of a restored window.
    pub backup_folder: Option<PathBuf>,
    /// The size, place and mode of a restored window.
    pub ui: Option<windows_state::UiState>,
}

/// `open_window` from a window's command: a failure is logged, the other windows stay.
pub(crate) fn open_window_or_log(request: WindowRequest, cx: &mut App) {
    if let Err(error) = open_window(request, cx) {
        eprintln!("[windows] opening a window: {error:#}");
    }
}

/// windowsMainService.ts `openInBrowserWindow`: a window with its own settings, storage, trust, extension host and
/// workbench, as each reference window has.
pub(crate) fn open_window(request: WindowRequest, cx: &mut App) -> anyhow::Result<gpui::WindowHandle<Workbench>> {
    let WindowRequest { workspace, file, development, open_url, screenshot, screenshot_keys, screenshot_input, export_default_keybindings, backup_folder, ui } = request;
    let app = cx.global::<AppState>();
    let (args, runtime, user_data, app_root, builtins, shared_storage) = (app.args.clone(), app.runtime, app.user_data.clone(), app.app_root.clone(), app.builtins.clone(), app.shared_storage.clone());
    let flag_value = |flag: &str| args.iter().position(|arg| arg == flag).and_then(|i| args.get(i + 1)).cloned();
    let profile = user_data_profile::ProfileState::resolve(&user_data, &app.user_extensions_folder, workspace.as_ref(), flag_value("--profile").as_deref(), args.iter().any(|arg| arg == "--profile-temp"), development.is_some());
    let user_extensions = UserExtensions { extensions_file: profile.current.extensions_resource.clone(), profiles_home: Some(profile.profiles.profiles_home.clone()), ..UserExtensions::new(app.user_extensions_folder.clone(), VERSION) };
    let mut extensions = builtins.clone();
    add_user_extensions(&mut extensions, user_extensions.scan()?);
    if let Some(development) = &development {
        development.add_extension(&mut extensions)?;
    }
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    perf::mark("code/willInitStorage");
    let storage = match extension_tests {
        true => MainThreadStorage::in_memory(),
        false => MainThreadStorage::open(&SessionPaths { app_root: &app_root, user_data: &user_data, workspace: workspace.as_ref() }, &profile.current.global_storage_home)?,
    };
    let application_storage = match extension_tests || profile.current.is_default {
        true => storage.profile.clone(),
        false => {
            let home = &profile.profiles.default_profile().global_storage_home;
            std::fs::create_dir_all(home)?;
            Arc::new(Mutex::new(storage::Storage::open(&home.join("state.vscdb"))))
        }
    };
    perf::mark("code/didInitStorage");
    // The extensions the user disabled stay out of the host and the registries (extensionEnablementService.ts).
    let developed: Vec<String> = extensions.iter().filter(|extension| extension.description["isUnderDevelopment"] == true).map(|extension| extension.id.clone()).collect();
    let installed = extensions.clone();
    let disabled = {
        let mut profile = storage.profile.lock().unwrap();
        let mut workspace_storage = storage.workspace.lock().unwrap();
        workbench::extensions_view::disabled_at_startup(&installed, &mut profile, workspace.is_some().then_some(&mut *workspace_storage), &developed)
    };
    extensions.retain(|extension| !disabled.contains(&extension.id));
    let load_settings = |extensions: &[ScannedExtension]| {
        let described: Vec<(&str, &serde_json::Value)> = extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
        let mut settings = Settings::load_with_profile(Registry::with_extensions(&described), user_data.join("User"), profile.user_files(), workspace.clone());
        settings.set_policy_values(policy::read_native_policies(&settings.registry, policy::BUNDLE_IDENTIFIER));
        settings
    };
    perf::mark("code/willInitWorkspaceService");
    let mut settings = load_settings(&extensions);
    perf::mark("code/didInitWorkspaceService");
    let setting = |key: &str| settings.get_value(Some(key), &Overrides::default());
    let new_window_dimensions = setting("window.newWindowDimensions").and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default();
    add_recently_opened(&mut shared_storage.lock().unwrap(), workspace.as_ref(), file.as_deref());
    let workspace_trust = workspace_trust::WorkspaceTrust::new(shared_storage.clone(), workspace_trust::Startup {
        // workspaceTrust.ts isWorkspaceTrustEnabled: only an explicit false turns the setting off.
        enabled: !args.iter().any(|arg| arg == "--disable-workspace-trust") && setting("security.workspace.trust.enabled") != Some(json!(false)),
        workspace: workspace.as_ref(),
        empty_window_trusted: setting("security.workspace.trust.emptyWindow") == Some(json!(true)),
        workspace_storage: &mut storage.workspace.lock().unwrap(),
    });
    if !workspace_trust.is_trusted() {
        let configured = setting("extensions.supportUntrustedWorkspaces").unwrap_or_default();
        let count = extensions.len();
        extensions.retain(|extension| !workspace_trust::disabled_when_untrusted(extension, workspace.as_ref(), &configured));
        if extensions.len() != count {
            settings = load_settings(&extensions);
        }
        settings.update_workspace_trust(false);
    }
    let window_extensions = WindowExtensions { builtins: builtins.clone(), user: user_extensions.clone_for_profile(), running: extensions.clone() };
    let theme = match flag_value("--theme") {
        Some(theme_id) => load_theme(&extensions, &theme_id)?,
        None => settings_theme(&extensions, &settings)?,
    };
    let file_icons = settings_file_icons(&extensions, &settings)?;
    let color_themes = theme::color_themes(&extensions)?;
    let settings = Arc::new(Mutex::new(settings));
    let keybindings_file = profile.current.keybindings_resource.clone();
    let session_closed = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Same size as the reference capture in port/tools/ref-screenshot, so pixel diffs line up.
    let mut bounds = Bounds::centered(None, size(px(1024.), px(768.)), cx);
    // A half-point origin (an odd display height, e.g. 1650 px over screen sharing) makes the drawable 1 point taller.
    bounds.origin = point(bounds.origin.x.round(), bounds.origin.y.round());
    let open_windows: Vec<Bounds<gpui::Pixels>> = cx.windows().into_iter().filter_map(|handle| handle.downcast::<Workbench>()).filter_map(|handle| handle.update(cx, |_, window, _| window.bounds()).ok()).collect();
    let location = windows_state::WindowState {
        workspace: workspace.as_ref().and_then(|workspace| workspace.file.clone()),
        folder: workspace.as_ref().filter(|workspace| workspace.file.is_none()).map(|workspace| workspace.folders[0].path.clone()),
        backup_path: backup_folder.clone(),
        ui: None,
    };
    let ui = ui.or_else(|| windows_state::stored_ui_state(&user_data, &location, open_windows.is_empty(), cx).filter(|_| !extension_tests));
    // windowsStateHandler.ts `doGetNewWindowState` for a window without a stored state: `window.newWindowDimensions`.
    let last_active = cx.window_stack().unwrap_or_else(|| cx.windows()).into_iter().find_map(|handle| handle.downcast::<Workbench>());
    let last_active_bounds = last_active.and_then(|handle| handle.update(cx, |_, window, _| window.window_bounds()).ok());
    let default_bounds = match (new_window_dimensions.as_str(), last_active_bounds) {
        ("maximized", _) => WindowBounds::Maximized(bounds),
        ("fullscreen", _) => WindowBounds::Fullscreen(bounds),
        ("inherit", Some(inherited)) => inherited,
        ("offset", Some(WindowBounds::Windowed(inherited))) => WindowBounds::Windowed(windows_state::cascade(inherited, &open_windows)),
        ("offset", Some(inherited)) => inherited,
        _ => WindowBounds::Windowed(windows_state::cascade(bounds, &open_windows)),
    };
    // The workbench draws its own title bar, as the reference does with window.titleBarStyle custom.
    let titlebar = TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(point(px(10.), px(10.))) };
    let (display_id, window_bounds) = ui.and_then(|ui| windows_state::window_bounds(ui, cx)).map_or((None, default_bounds), |(display, bounds)| (Some(display), bounds));
    // window.ts `WindowMinimumSize`.
    let window_min_size = Some(size(px(400.), px(270.)));
    let options = WindowOptions { window_bounds: Some(window_bounds), display_id, titlebar: Some(titlebar), app_owns_titlebar_drag: true, window_min_size, ..Default::default() };
    perf::mark("code/willOpenNewWindow");
    perf::mark("code/willCreateCodeWindow");
    let window = {
        cx.open_window(options, |window, cx| {
            perf::mark("code/willStartWorkbench");
            cx.new(|cx| {
                let (sender, mut receiver) = unbounded::<String>();
                let (host_sender, host_receiver) = channel();
                let (request_sender, requests) = unbounded::<ForwardedRequest>();
                let (reply_sender, replies) = channel::<(u32, Result<serde_json::Value, serde_json::Value>)>();
                let (main_thread_sender, main_thread_requests) = unbounded::<HostRequest>();
                let main_thread = WorkbenchMainThread { requests: main_thread_sender, messages: host_receiver, waiting: Vec::new(), content_providers: Default::default(), extensions: window_extensions };
                let languages = Languages::new(&extensions);
                let mut host = HostSync::new(host_sender);
                host.configuration_scopes = development.is_some();
                host.send_color_theme(theme.kind);
                perf::mark("code/willCreateWorkbenchContributions/1");
                host.send_language_ids(&languages);
                let mut workbench = Workbench::new(theme, app_root.clone(), workspace.clone(), languages, settings.clone(), host, cx);
                perf::mark("code/didCreateWorkbench");
                perf::mark("code/didCreateWorkbenchContributions/2");
                cx.observe_window_appearance(window, |workbench, _, cx| workbench.host_color_scheme_changed(cx)).detach();
                let window_logs = exthost::session::window_logs_path(&user_data);
                workbench.register_window_log(window_logs.join("renderer.log"));
                let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                workbench.start_logs(output::log_levels::argv_file(&home), window_logs.parent().expect("a session folder").to_path_buf(), cx);
                workbench.skip_dialogs = development.as_ref().is_some_and(|development| development.tests_path.is_some());
                workbench.refresh_keyboard_mapper();
                workbench.extension_development = development.is_some();
                workbench.load_keybindings(keybindings::default_keybindings(&extensions), keybindings_file.clone(), cx);
                workbench.watch_settings_files(cx);
                workbench.watch_policies(cx);
                workbench.start_file_watcher(cx);
                workbench.start_system_wide_keybindings(window, cx);
                workbench.register_update_actions(window, cx);
                workbench.set_command_palette_menu(command_palette::menu(&extensions));
                workbench.menus.extend(menus::extension_menus(&extensions));
                workbench.command_icons = menus::command_icons(&extensions);
                workbench.status_entries = main_thread_status_bar::static_entries(&extensions);
                workbench.urls.extensions = extensions.iter().map(|extension| (extension.id.to_lowercase(), extension.description["displayName"].as_str().unwrap_or(&extension.id).to_owned())).collect();
                workbench.file_icons = file_icons;
                workbench.color_themes = color_themes;
                workbench.set_tree_views(&extensions);
                workbench.host.set_running_extensions(&extensions);
                workbench.extensions = extensions.clone();
                workbench.register_webview_view_commands();
                workbench.start_tasks();
                if let Some(folder) = &export_default_keybindings {
                    match workbench.export_default_keybindings(folder) {
                        Ok(path) => eprintln!("[workbench.contrib.keybindingsExport] Wrote {}", path.display()),
                        Err(error) => eprintln!("[workbench.contrib.keybindingsExport] Failed to generate default keybindings {error}"),
                    }
                    cx.quit();
                }
                workbench.restore_profile_state(storage.profile.clone(), application_storage);
                workbench.restore_workspace_state(storage.workspace.clone());
                let workspace_trusted = workspace_trust.is_trusted();
                workbench.workspace_trust = workspace_trust;
                workbench.shared_storage = shared_storage;
                workbench.watch_recently_opened(window, cx);
                workbench.watch_window_focus(window, cx);
                workbench.watch_window_close(window, cx);
                workbench.set_installed_extensions(builtins.clone(), user_extensions.clone_for_profile(), developed.clone(), cx);
                workbench.set_user_data_profiles(profile, cx);
                workbench.offer_settings_import(window, cx);
                workbench.start_terminal_host();
                workbench.watch_editor_focus(window, cx);
                workbench.show_trust_modal_on_start(window, cx);
                workbench.start_backups(&user_data, backup_folder, cx);
                perf::mark("code/willRestoreEditors");
                if let Some(file) = file {
                    workbench.open_file(file, true, window, cx);
                }
                workbench.run_startup_page(args.iter().any(|arg| arg == "--skip-welcome"), window, cx);
                perf::set_startup_editors(workbench.visible_editor_ids());
                perf::mark("code/didRestoreEditors");
                if let Some(url) = &open_url {
                    workbench.open_protocol_url(url, window, cx);
                }
                let (session_ended, ended) = futures::channel::oneshot::channel::<()>();
                workbench.watch_session_end(session_closed.clone(), ended, cx);
                std::thread::Builder::new()
                    .name("exthost-session".into())
                    .spawn(move || {
                        let runs_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
                        if let Err(error) = run_extension_host(runtime, app_root, extensions, settings, storage, workspace_trusted, user_data, workspace, development, main_thread, request_sender, replies, &sender, &session_closed) {
                            if runs_tests {
                                eprintln!("extension host failed: {error:#}");
                                std::process::exit(1);
                            }
                            let _ = sender.unbounded_send(format!("extension host failed: {error:#}"));
                        }
                        let _ = session_ended.send(());
                    })
                    .expect("spawn exthost session thread");
                workbench::extension_host_restart::serve_host_requests(requests, reply_sender, window, cx);
                workbench::extension_host_restart::serve_main_thread_requests(main_thread_requests, window, cx);
                cx.spawn_in(window, async move |workbench, cx| {
                    while let Some(line) = receiver.next().await {
                        eprintln!("{line}");
                        if line.starts_with(HANDSHAKE_PREFIX) {
                            let _ = workbench.update(cx, |workbench, _| workbench.host.installed_extensions_registered = true);
                            perf::mark("code/didLoadExtensions");
                        }
                        let (true, Some(path)) = (line.starts_with(ACTIVATED_PREFIX), screenshot.as_ref()) else { continue };
                        // Captured once eager extensions are up, so the frame shows the workbench as the host left it.
                        cx.background_executor().timer(Duration::from_millis(500)).await;
                        if !screenshot_keys.is_empty() {
                            cx.update(|window, cx| {
                                for keystroke in &screenshot_keys {
                                    window.dispatch_keystroke(keystroke.clone(), cx);
                                }
                            })
                            .ok();
                        }
                        for step in screenshot_input.split(';').filter(|step| !step.trim().is_empty()) {
                            let input = screenshot_step(step).with_context(|| format!("--screenshot-input step {step:?}"));
                            let dispatched = input.and_then(|input| cx.update(|window, cx| window.dispatch_event(input, cx)));
                            if let Err(error) = dispatched {
                                eprintln!("{error:#}");
                                std::process::exit(1);
                            }
                            cx.background_executor().timer(Duration::from_millis(300)).await;
                        }
                        // The editor's scrollbars show at open and after a scroll, hide 500ms later and fade out
                        // over 800ms; ref-screenshot waits the same.
                        cx.background_executor().timer(Duration::from_millis(1500)).await;
                        // The capture reads the last frame: draw one now without the input carets.
                        workbench.update(cx, |workbench, _| workbench.input_carets_hidden = true).ok();
                        cx.update(|window, _| window.refresh()).ok();
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                        let saved = cx.update(|window, _| save_screenshot(window, path)).and_then(|result| result);
                        if let Err(error) = saved {
                            eprintln!("screenshot failed: {error:#}");
                            std::process::exit(1);
                        }
                        cx.update(|_, cx| cx.quit()).ok();
                    }
                })
                .detach();
                perf::mark("code/didStartWorkbench");
                workbench
            })
        })?
    };
    perf::mark("code/didCreateCodeWindow");
    Ok(window)
}

/// Owns the session for the life of the app: handshake, one call each way, then serves host requests and
/// forwards what the workbench tells the host.
// It takes what main parsed for the session thread; a struct for one call site would only rename the list.
#[allow(clippy::too_many_arguments)]
fn run_extension_host(
    runtime: Runtime,
    app_root: PathBuf,
    extensions: Vec<ScannedExtension>,
    settings: Arc<Mutex<Settings>>,
    mut storage: MainThreadStorage,
    workspace_trusted: bool,
    user_data: PathBuf,
    workspace: Option<Workspace>,
    development: Option<ExtensionDevelopment>,
    main_thread: WorkbenchMainThread,
    workbench_requests: UnboundedSender<ForwardedRequest>,
    workbench_replies: Receiver<(u32, Result<serde_json::Value, serde_json::Value>)>,
    status: &UnboundedSender<String>,
    window_closed: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace: workspace.as_ref() };
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    let mut init = init_data(&paths, &extensions, VERSION);
    // StateService of main.ts: `storage.json` in the global storage folder.
    let global_storage = user_data.join("User/globalStorage");
    std::fs::create_dir_all(&global_storage)?;
    init["telemetryInfo"] = storage.telemetry_info(&mut FileStorage::open(&global_storage.join("storage.json")));
    if let Some(development) = &development {
        development.add_to_init_data(&mut init);
    }
    let entry = app_root.join("out-exthost/bootstrap-exthost.js");

    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let _ = status.unbounded_send(format!("{HANDSHAKE_PREFIX}{}", host.child.id()));
    let mut session = Session::new(host);
    let folders: Vec<PathBuf> = workspace.iter().flat_map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone())).collect();
    let mut watched = Vec::new();
    for folder in &folders {
        let excludes = settings.lock().unwrap().get_value(Some("files.watcherExclude"), &Overrides { resource: Some(folder), ..Overrides::default() });
        let excludes: Vec<String> = excludes.as_ref().and_then(|excludes| excludes.as_object()).into_iter().flatten().filter(|(_, on)| **on == true).map(|(pattern, _)| pattern.clone()).collect();
        watched.push((folder.clone(), excludes));
    }
    session.files = Some(MainThreadFiles::new(&watched, &extensions, settings.clone())?);
    session.workspace = Some(MainThreadWorkspace::new(folders, settings.clone(), &app_root));
    session.configuration = Some(MainThreadConfiguration::new(settings, development.is_some()));
    session.storage = Some(storage);
    session.workspace_trusted = workspace_trusted;
    // secrets.ts: without encryption the secret storage falls back to memory.
    session.secrets = Some(match extension_tests {
        true => MainThreadSecretState::in_memory(),
        false => MainThreadSecretState::keychain().unwrap_or_else(|error| {
            eprintln!("[secrets] the Keychain is unavailable, secrets stay in memory: {error}");
            MainThreadSecretState::in_memory()
        }),
    });
    session.forward(&["MainThreadCommands", "MainThreadComments", "MainThreadCustomEditors", "MainThreadDebugService", "MainThreadDecorations", "MainThreadDialogs", "MainThreadDiagnostics", "MainThreadDownloadService", "MainThreadEditorTabs", "MainThreadGitHub", "MainThreadLanguageFeatures", "MainThreadLanguages.$removeLanguageStatus", "MainThreadLanguages.$setLanguageStatus", "MainThreadLogger", "MainThreadMessageService", "MainThreadOutputService", "MainThreadProgress", "MainThreadQuickDiff", "MainThreadQuickOpen", "MainThreadSCM", "MainThreadStatusBar", "MainThreadTask", "MainThreadTerminalService", "MainThreadTerminalShellIntegration", "MainThreadTesting", "MainThreadTimeline", "MainThreadTreeViews", "MainThreadUriOpeners", "MainThreadUrls", "MainThreadUrls.$registerUriHandler", "MainThreadUrls.$unregisterUriHandler", "MainThreadWebviewPanels", "MainThreadWebviewViews", "MainThreadWebviews", "MainThreadWindow.$openUri", "MainThreadWorkspace"], move |request| {
        let _ = workbench_requests.unbounded_send(request);
    }, workbench_replies);
    session.main_thread = Some(Box::new(main_thread));
    session.notify("ExtHostStatusBar", "$acceptStaticEntries", vec![json!(main_thread_status_bar::static_entry_dtos(&extensions))]);
    perf::mark("code/willStartExtensionHost");
    session.start(&extensions, workspace.as_ref().map(|workspace| workspace_data(&init, workspace)))?;
    perf::mark("code/didStartExtensionHost");
    let reply = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let _ = status.unbounded_send(format!("main -> host: $test_latency(7) = {reply:?}"));
    let _ = status.unbounded_send(format!("host -> main: {} requests so far", session.log.host_calls.values().sum::<u32>()));
    let _ = status.unbounded_send(format!("{ACTIVATED_PREFIX}{}", session.log.activated.join(", ")));
    if development.is_some_and(|development| development.tests_path.is_some()) {
        session.execute_extension_tests();
    }
    // The short pump timeout bounds how long a workbench change waits before the host hears of it.
    loop {
        // A closed window's host goes with its session (ExtensionHost's drop terminates it).
        if window_closed.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        session.flush_main_thread();
        if !session.pump(Duration::from_millis(20))? {
            break;
        }
    }
    if let Some(code) = session.extension_tests_exit_code() {
        drop(session);
        std::process::exit(code);
    }
    let _ = status.unbounded_send("extension host exited".into());
    Ok(())
}

/// package.json `version`.
const VERSION: &str = "1.147.0";

const ACTIVATED_PREFIX: &str = "eager extensions activated: ";
const HANDSHAKE_PREFIX: &str = "handshake done, pid ";

/// A step of `--screenshot-input`: `right X Y` (a right click), `move X Y` or `key KEYSTROKE`, in window points.
fn screenshot_step(step: &str) -> anyhow::Result<gpui::PlatformInput> {
    let parts: Vec<&str> = step.split_whitespace().collect();
    let position = || -> anyhow::Result<gpui::Point<gpui::Pixels>> { Ok(point(px(parts.get(1).context("no x")?.parse()?), px(parts.get(2).context("no y")?.parse()?))) };
    let modifiers = gpui::Modifiers::default();
    Ok(match parts[0] {
        "right" => gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Right, position: position()?, modifiers, click_count: 1, first_mouse: false }),
        "move" => gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: position()?, pressed_button: None, modifiers }),
        "key" => gpui::PlatformInput::KeyDown(gpui::KeyDownEvent { keystroke: gpui::Keystroke::parse(parts.get(1).context("no key")?)?, is_held: false, prefer_character_input: false }),
        other => anyhow::bail!("unknown step {other}"),
    })
}

/// Reads the last rendered frame back from Metal, so capture needs no Screen Recording permission.
#[cfg(feature = "screenshot")]
fn save_screenshot(window: &Window, path: &std::path::Path) -> anyhow::Result<()> {
    window.render_to_image()?.save(path)?;
    Ok(())
}

#[cfg(not(feature = "screenshot"))]
fn save_screenshot(_window: &Window, _path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("built without the screenshot feature: cargo build --features screenshot")
}

/// The color theme setting for the host scheme (`getColorThemeSettingId`); a theme no extension contributes falls back to the default, as ThemeService does.
fn settings_theme(extensions: &[ScannedExtension], settings: &Settings) -> anyhow::Result<theme::Theme> {
    let key = workbench::themes::color_theme_setting_id(settings, workbench::themes::host_color_scheme());
    let id = settings.get_value(Some(key), &Overrides::default());
    if let Some(theme) = id.as_ref().and_then(|id| id.as_str()).and_then(|id| load_theme(extensions, id).ok()) {
        return Ok(theme);
    }
    let default = &settings.registry.property(key).expect("the color theme settings are registered").default;
    load_theme(extensions, default.as_str().expect("the default theme is a settings id"))
}

/// workbenchThemeService.ts `initializeFileIconTheme`: `null` is the built-in "no icons" theme, an unknown id the default.
fn settings_file_icons(extensions: &[ScannedExtension], settings: &Settings) -> anyhow::Result<Option<file_icons::FileIconTheme>> {
    let id = settings.get_value(Some("workbench.iconTheme"), &Overrides::default());
    let id = match id {
        Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(id)) => id,
        _ => file_icons::DEFAULT_ICON_THEME.to_string(),
    };
    let theme = file_icons::FileIconTheme::load(extensions, &id).or_else(|_| file_icons::FileIconTheme::load(extensions, file_icons::DEFAULT_ICON_THEME))?;
    Ok(Some(theme))
}

#[cfg(test)]
fn default_settings() -> Arc<Mutex<Settings>> {
    Arc::new(Mutex::new(Settings::load(Registry::with_extensions(&[]), PathBuf::from("/nonexistent/User"), None)))
}

#[test]
fn icon_theme_follows_workbench_icon_theme() {
    let extensions = scan_builtin_extensions(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../extensions")).unwrap();
    let user_home = std::env::temp_dir().join(format!("code-gpui-icon-theme-{}", std::process::id()));
    std::fs::create_dir_all(&user_home).unwrap();
    let icons_with = |settings_json: &str| {
        std::fs::write(user_home.join("settings.json"), settings_json).unwrap();
        let settings = Settings::load(Registry::with_extensions(&[]), user_home.clone(), None);
        settings_file_icons(&extensions, &settings).unwrap().is_some()
    };
    assert!(icons_with("{}"));
    assert!(!icons_with(r#"{ "workbench.iconTheme": null }"#));
    assert!(icons_with(r#"{ "workbench.iconTheme": "no-such-theme" }"#));
    std::fs::remove_dir_all(&user_home).unwrap();
}

/// windowsMainService.ts `open` with `addRecentlyOpened`: the window's folder or workspace and its file.
fn add_recently_opened(shared: &mut storage::Storage, workspace: Option<&Workspace>, file: Option<&Path>) {
    let mut opened = Vec::new();
    if let Some(workspace) = workspace {
        opened.push(match &workspace.file {
            Some(file) => recently_opened::entry(recently_opened::Kind::Workspace, file, None),
            None => recently_opened::entry(recently_opened::Kind::Folder, &workspace.folders[0].path, None),
        });
    }
    opened.extend(file.map(|file| recently_opened::entry(recently_opened::Kind::File, file, None)));
    if !opened.is_empty() {
        recently_opened::add(shared, opened);
    }
}

/// The APPLICATION_SHARED storage, `sharedStorage/state.vscdb` in `--shared-data-dir`, else in
/// `~/.vscode-gpui-shared` as the reference keeps it in `~/<sharedDataFolderName>`.
fn open_shared_storage(shared_data: Option<PathBuf>) -> anyhow::Result<storage::Storage> {
    let shared_data = match shared_data {
        Some(dir) => dir,
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".vscode-gpui-shared"),
    };
    let dir = shared_data.join("sharedStorage");
    std::fs::create_dir_all(&dir)?;
    Ok(storage::Storage::open(&dir.join("state.vscdb")))
}

/// main.ts `resolveShellEnv` merged into `process.env`: a Dock launch gets the login shell's PATH for the host,
/// terminals and tasks. Runs before any thread starts, so changing the environment is sound.
// shortcut: the timeout is the 10 s default, `application.shellEnvironmentResolutionTimeout` is read later than this; read it here once settings load first.
fn apply_shell_env(force_disable: bool) {
    let env: std::collections::HashMap<String, String> = std::env::vars_os().filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?))).collect();
    match shell_env::resolve_shell_env(&env, force_disable, std::time::Duration::from_secs(10)) {
        Some(Ok(resolved)) => {
            for (key, value) in resolved {
                // SAFETY: called from main before the host, the UI or any other thread starts.
                unsafe { std::env::set_var(key, value) };
            }
        }
        Some(Err(message)) => eprintln!("[shellEnv] {message}"),
        None => {}
    }
}

fn default_user_data() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    // Artur: all user data lives in ~/.vsrustium, product.json `dataFolderName`.
    let dir = PathBuf::from(home).join(".vsrustium");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
