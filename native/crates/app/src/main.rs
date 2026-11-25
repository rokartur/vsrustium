//! The native app: the workbench, themed from the built-in theme files, over the running extension host.
//! Usage: code-gpui [folder or .code-workspace file [file]] [--theme <settings id>] [--user-data-dir <dir>] [--screenshot <png>]
//! [--screenshot-keys "<keystroke> ..."] [--screenshot-input <steps>] [--node]
//!   [--extensionDevelopmentPath=<dir> [--extensionTestsPath=<path>]]: the app exits with the test runner's code.

#[cfg_attr(not(test), expect(dead_code, reason = "the code action menu of milestone 13 is its first desktop caller"))]
mod action_list;
mod anything_quick_access;
mod brackets;
mod clipboard;
mod command_palette;
mod commands;
mod contextkey;
mod dialog;
mod editor;
mod editor_history;
mod explorer;
mod file_actions;
mod file_icons;
mod filters;
mod history;
mod fuzzy_scorer;
mod host_sync;
mod input_box;
mod keybinding_editing;
mod keybindings;
mod keybindings_editor;
mod keyboard_layout;
mod system_wide_keybindings;
mod languages;
mod local_history;
mod main_thread;
mod main_thread_commands;
mod main_thread_message;
mod main_thread_progress;
mod main_thread_quick_open;
mod main_thread_status_bar;
mod markers;
mod menus;
mod notifications;
mod quick_input;
mod search_editor;
mod search_model;
mod smart_snippet_inserter;
mod split_view;
mod syntax;
mod tf_idf;
mod theme;
mod undo_redo;
mod workbench;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::configuration::Overrides;
use config::registry::Registry;
use config::service::Settings;
use config::workspace::Workspace;
use exthost::extension_development::ExtensionDevelopment;
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
use anyhow::Context as _;
use commands::Outcome;
use host_sync::HostSync;
use languages::Languages;
use main_thread::{HostRequest, WorkbenchMainThread};
use theme::load_theme;
use workbench::Workbench;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let runtime = if args.iter().any(|arg| arg == "--node") { Runtime::Node } else { Runtime::Bun };
    let flag_value = |flag: &str| args.iter().position(|arg| arg == flag).and_then(|i| args.get(i + 1)).cloned();
    let user_data = match flag_value("--user-data-dir") {
        Some(dir) => PathBuf::from(dir),
        None => default_user_data()?,
    };
    let screenshot = flag_value("--screenshot").map(PathBuf::from);
    let screenshot_input = flag_value("--screenshot-input").unwrap_or_default();
    // Typed before the capture, as port/tools/ref-screenshot types KEYS into the reference.
    let screenshot_keys: Vec<Keystroke> = flag_value("--screenshot-keys").unwrap_or_default().split_whitespace().map(Keystroke::parse).collect::<Result<_, _>>()?;
    let value_flags = ["--user-data-dir", "--screenshot", "--screenshot-keys", "--screenshot-input", "--theme"];
    let positional: Vec<&String> = args.iter().enumerate().skip(1).filter(|(i, arg)| !arg.starts_with("--") && !value_flags.contains(&args[i - 1].as_str())).map(|(_, arg)| arg).collect();
    let canonical = |path: &String| std::fs::canonicalize(path).with_context(|| format!("opening {path}"));
    let workspace = positional.first().map(|path| canonical(path)).transpose()?.map(Workspace::open);
    let file = positional.get(1).map(|path| canonical(path)).transpose()?;
    let development = ExtensionDevelopment::from_args(&args)?;

    let app_root = std::env::current_dir()?;
    let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
    let mut extensions = scan_builtin_extensions(&extensions_dir)?;
    if let Some(development) = &development {
        development.add_extension(&mut extensions)?;
    }
    let described: Vec<(&str, &serde_json::Value)> = extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
    let settings = Settings::load(Registry::with_extensions(&described), user_data.join("User"), workspace.clone());
    let theme = match flag_value("--theme") {
        Some(theme_id) => load_theme(&extensions, &theme_id)?,
        None => settings_theme(&extensions, &settings)?,
    };
    let file_icons = settings_file_icons(&extensions, &settings)?;
    let color_themes = theme::color_themes(&extensions)?;
    let settings = Arc::new(Mutex::new(settings));
    let keybindings_file = user_data.join("User/keybindings.json");
    // storageMainService.ts: no storage during extension tests, so each test run starts empty.
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    let storage = match extension_tests {
        true => MainThreadStorage::in_memory(),
        false => MainThreadStorage::open(&SessionPaths { app_root: &app_root, user_data: &user_data, workspace: workspace.as_ref() })?,
    };

    application().run(move |cx: &mut App| {
        // Same size as the reference capture in port/tools/ref-screenshot, so pixel diffs line up.
        let mut bounds = Bounds::centered(None, size(px(1024.), px(768.)), cx);
        // A half-point origin (an odd display height, e.g. 1650 px over screen sharing) makes the drawable 1 point taller.
        bounds.origin = point(bounds.origin.x.round(), bounds.origin.y.round());
        // The workbench draws its own title bar, as the reference does with window.titleBarStyle custom.
        let titlebar = TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(point(px(10.), px(10.))) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(titlebar), app_owns_titlebar_drag: true, ..Default::default() };
        let window = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let (sender, mut receiver) = unbounded::<String>();
                let (host_sender, host_receiver) = channel();
                let (request_sender, mut requests) = unbounded::<ForwardedRequest>();
                let (reply_sender, replies) = channel::<(u32, Result<serde_json::Value, serde_json::Value>)>();
                let (main_thread_sender, mut main_thread_requests) = unbounded::<HostRequest>();
                let main_thread = WorkbenchMainThread { requests: main_thread_sender, messages: host_receiver, waiting: Vec::new(), content_providers: Vec::new() };
                let languages = Languages::new(&extensions);
                let mut host = HostSync::new(host_sender);
                host.configuration_scopes = development.is_some();
                host.send_color_theme(theme.kind);
                let mut workbench = Workbench::new(theme, app_root.clone(), workspace.clone(), languages, settings.clone(), host, cx);
                workbench.skip_dialogs = development.as_ref().is_some_and(|development| development.tests_path.is_some());
                workbench.load_keybindings(keybindings::default_keybindings(&extensions), keybindings_file.clone(), cx);
                workbench.start_system_wide_keybindings(window, cx);
                workbench.set_command_palette_menu(command_palette::menu(&extensions));
                workbench.menus.extend(menus::extension_menus(&extensions));
                workbench.status_entries = main_thread_status_bar::static_entries(&extensions);
                workbench.file_icons = file_icons;
                workbench.color_themes = color_themes;
                workbench.restore_profile_state(storage.profile.clone());
                workbench.restore_workspace_state(storage.workspace.clone());
                workbench.watch_editor_focus(window, cx);
                if let Some(file) = file {
                    workbench.open_file(file, true, window, cx);
                }
                std::thread::Builder::new()
                    .name("exthost-session".into())
                    .spawn(move || {
                        let runs_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
                        if let Err(error) = run_extension_host(runtime, app_root, extensions, settings, storage, user_data, workspace, development, main_thread, request_sender, replies, &sender) {
                            if runs_tests {
                                eprintln!("extension host failed: {error:#}");
                                std::process::exit(1);
                            }
                            let _ = sender.unbounded_send(format!("extension host failed: {error:#}"));
                        }
                    })
                    .expect("spawn exthost session thread");
                cx.spawn_in(window, async move |workbench, cx| {
                    while let Some(request) = requests.next().await {
                        let req = request.req;
                        // A cancellation answers nothing itself; the request it cancels gets its answer.
                        let cancel = request.method == "$cancel";
                        let Ok(outcome) = workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx)) else { break };
                        let result = match outcome {
                            _ if cancel => continue,
                            Outcome::Ready(result) => result,
                            // Requests from the host may overlap, so a pending answer is awaited off this loop.
                            Outcome::Pending(pending) => {
                                let reply_sender = reply_sender.clone();
                                cx.background_executor()
                                    .spawn(async move {
                                        let _ = reply_sender.send((req, pending.await.unwrap_or(Ok(serde_json::Value::Null))));
                                    })
                                    .detach();
                                continue;
                            }
                        };
                        let _ = reply_sender.send((req, result));
                    }
                })
                .detach();
                cx.spawn_in(window, async move |workbench, cx| {
                    while let Some(request) = main_thread_requests.next().await {
                        let result = workbench.update_in(cx, |workbench, window, cx| workbench.host_request(&request.proxy, &request.method, &request.args, window, cx));
                        let _ = request.reply.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled"))));
                    }
                })
                .detach();
                cx.spawn_in(window, async move |workbench, cx| {
                    while let Some(line) = receiver.next().await {
                        eprintln!("{line}");
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
                workbench
            })
        })
        .unwrap();
        workbench::menubar::install(window, cx);
        cx.activate(true);
    });
    Ok(())
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
    user_data: PathBuf,
    workspace: Option<Workspace>,
    development: Option<ExtensionDevelopment>,
    main_thread: WorkbenchMainThread,
    workbench_requests: UnboundedSender<ForwardedRequest>,
    workbench_replies: Receiver<(u32, Result<serde_json::Value, serde_json::Value>)>,
    status: &UnboundedSender<String>,
) -> anyhow::Result<()> {
    let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace: workspace.as_ref() };
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    let mut init = init_data(&paths, &extensions, "1.147.0");
    init["telemetryInfo"] = storage.telemetry_info();
    if let Some(development) = &development {
        development.add_to_init_data(&mut init);
    }
    let entry = app_root.join("out-exthost/bootstrap-exthost.js");

    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let _ = status.unbounded_send(format!("handshake done, pid {}", host.child.id()));
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
    // secrets.ts: without encryption the secret storage falls back to memory.
    session.secrets = Some(match extension_tests {
        true => MainThreadSecretState::in_memory(),
        false => MainThreadSecretState::keychain().unwrap_or_else(|error| {
            eprintln!("[secrets] the Keychain is unavailable, secrets stay in memory: {error}");
            MainThreadSecretState::in_memory()
        }),
    });
    session.forward(&["MainThreadCommands", "MainThreadDiagnostics", "MainThreadEditorTabs", "MainThreadMessageService", "MainThreadProgress", "MainThreadQuickOpen", "MainThreadStatusBar", "MainThreadWorkspace"], move |request| {
        let _ = workbench_requests.unbounded_send(request);
    }, workbench_replies);
    session.main_thread = Some(Box::new(main_thread));
    session.notify("ExtHostStatusBar", "$acceptStaticEntries", vec![json!(main_thread_status_bar::static_entry_dtos(&extensions))]);
    session.start(&extensions, workspace.as_ref().map(|workspace| workspace_data(&init, workspace)))?;
    let reply = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let _ = status.unbounded_send(format!("main -> host: $test_latency(7) = {reply:?}"));
    let _ = status.unbounded_send(format!("host -> main: {} requests so far", session.log.host_calls.values().sum::<u32>()));
    let _ = status.unbounded_send(format!("{ACTIVATED_PREFIX}{}", session.log.activated.join(", ")));
    if development.is_some_and(|development| development.tests_path.is_some()) {
        session.execute_extension_tests();
    }
    // The short pump timeout bounds how long a workbench change waits before the host hears of it.
    loop {
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

const ACTIVATED_PREFIX: &str = "eager extensions activated: ";

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

/// `workbench.colorTheme`; a theme no extension contributes falls back to the default, as ThemeService does.
fn settings_theme(extensions: &[ScannedExtension], settings: &Settings) -> anyhow::Result<theme::Theme> {
    let id = settings.get_value(Some("workbench.colorTheme"), &Overrides::default());
    if let Some(theme) = id.as_ref().and_then(|id| id.as_str()).and_then(|id| load_theme(extensions, id).ok()) {
        return Ok(theme);
    }
    let default = &settings.registry.property("workbench.colorTheme").expect("workbench.colorTheme is registered").default;
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

fn default_user_data() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/vscode-gpui");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
