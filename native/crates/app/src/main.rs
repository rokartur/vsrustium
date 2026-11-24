//! The native app: the workbench, themed from the built-in theme files, over the running extension host.
//! Usage: code-gpui [folder [file]] [--theme <settings id>] [--user-data-dir <dir>] [--screenshot <png>] [--node]
//!   [--extensionDevelopmentPath=<dir> [--extensionTestsPath=<path>]]: the app exits with the test runner's code.

mod brackets;
mod editor;
mod explorer;
mod host_sync;
mod languages;
mod syntax;
mod theme;
mod workbench;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::configuration::Overrides;
use config::registry::Registry;
use config::service::Settings;
use exthost::extension_development::ExtensionDevelopment;
use exthost::main_thread_configuration::MainThreadConfiguration;
use exthost::main_thread_secret_state::MainThreadSecretState;
use exthost::main_thread_storage::MainThreadStorage;
use exthost::rpc::Arg;
use exthost::scan::{ScannedExtension, scan_builtin_extensions};
use exthost::session::{Session, SessionPaths, init_data, workspace_data};
use exthost::{ExtensionHost, HostOptions, Runtime};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Bounds, TitlebarOptions, Window, WindowBounds, WindowOptions, point, prelude::*, px, size};
use gpui_platform::application;
use serde_json::json;
use anyhow::Context as _;
use explorer::FileTree;
use host_sync::{HostMessage, HostSync};
use languages::Languages;
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
    let value_flags = ["--user-data-dir", "--screenshot", "--theme"];
    let positional: Vec<&String> = args.iter().enumerate().skip(1).filter(|(i, arg)| !arg.starts_with("--") && !value_flags.contains(&args[i - 1].as_str())).map(|(_, arg)| arg).collect();
    let canonical = |path: &String| std::fs::canonicalize(path).with_context(|| format!("opening {path}"));
    let folder = positional.first().map(|path| canonical(path)).transpose()?;
    let file = positional.get(1).map(|path| canonical(path)).transpose()?;
    let development = ExtensionDevelopment::from_args(&args)?;

    let app_root = std::env::current_dir()?;
    let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
    let mut extensions = scan_builtin_extensions(&extensions_dir)?;
    if let Some(development) = &development {
        development.add_extension(&mut extensions)?;
    }
    let described: Vec<(&str, &serde_json::Value)> = extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
    let settings = Settings::load(Registry::with_extensions(&described), user_data.join("User"), folder.clone());
    let theme = match flag_value("--theme") {
        Some(theme_id) => load_theme(&extensions, &theme_id)?,
        None => settings_theme(&extensions, &settings)?,
    };
    let settings = Arc::new(Mutex::new(settings));

    application().run(move |cx: &mut App| {
        // Same size as the reference capture in port/tools/ref-screenshot, so pixel diffs line up.
        let mut bounds = Bounds::centered(None, size(px(1024.), px(768.)), cx);
        // A half-point origin (an odd display height, e.g. 1650 px over screen sharing) makes the drawable 1 point taller.
        bounds.origin = point(bounds.origin.x.round(), bounds.origin.y.round());
        // The workbench draws its own title bar, as the reference does with window.titleBarStyle custom.
        let titlebar = TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(point(px(10.), px(10.))) };
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(titlebar), ..Default::default() };
        cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let (sender, mut receiver) = unbounded::<String>();
                let (host_sender, host_receiver) = channel();
                let languages = Languages::new(&extensions);
                let host = HostSync::new(host_sender);
                host.send_color_theme(theme.kind);
                let mut workbench = Workbench::new(theme, app_root.clone(), folder.clone().map(FileTree::new), languages, settings.clone(), host, cx);
                if let Some(file) = file {
                    workbench.open_file(file, true, window, cx);
                }
                std::thread::Builder::new()
                    .name("exthost-session".into())
                    .spawn(move || {
                        let runs_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
                        if let Err(error) = run_extension_host(runtime, app_root, extensions, settings, user_data, folder, development, host_receiver, &sender) {
                            if runs_tests {
                                eprintln!("extension host failed: {error:#}");
                                std::process::exit(1);
                            }
                            let _ = sender.unbounded_send(format!("extension host failed: {error:#}"));
                        }
                    })
                    .expect("spawn exthost session thread");
                cx.spawn_in(window, async move |_, cx| {
                    while let Some(line) = receiver.next().await {
                        eprintln!("{line}");
                        let (true, Some(path)) = (line.starts_with(ACTIVATED_PREFIX), screenshot.as_ref()) else { continue };
                        // Captured once eager extensions are up, so the frame shows the workbench as the host left it.
                        cx.background_executor().timer(Duration::from_millis(500)).await;
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
    user_data: PathBuf,
    folder: Option<PathBuf>,
    development: Option<ExtensionDevelopment>,
    workbench_messages: Receiver<HostMessage>,
    status: &UnboundedSender<String>,
) -> anyhow::Result<()> {
    let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace_folder: folder.as_deref() };
    // storageMainService.ts: no storage during extension tests, so each test run starts empty.
    let extension_tests = development.as_ref().is_some_and(|development| development.tests_path.is_some());
    let mut storage = if extension_tests { MainThreadStorage::in_memory() } else { MainThreadStorage::open(&paths)? };
    let mut init = init_data(&paths, &extensions, "1.147.0");
    init["telemetryInfo"] = storage.telemetry_info();
    if let Some(development) = &development {
        development.add_to_init_data(&mut init);
    }
    let entry = app_root.join("out-exthost/bootstrap-exthost.js");

    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let _ = status.unbounded_send(format!("handshake done, pid {}", host.child.id()));
    let mut session = Session::new(host);
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
    session.start(&extensions, folder.as_deref().map(|folder| workspace_data(&init, folder)))?;
    let reply = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let _ = status.unbounded_send(format!("main -> host: $test_latency(7) = {reply:?}"));
    let _ = status.unbounded_send(format!("host -> main: {} requests so far", session.log.host_calls.values().sum::<u32>()));
    let _ = status.unbounded_send(format!("{ACTIVATED_PREFIX}{}", session.log.activated.join(", ")));
    if development.is_some_and(|development| development.tests_path.is_some()) {
        session.execute_extension_tests();
    }
    // The short pump timeout bounds how long a workbench change waits before the host hears of it.
    loop {
        for message in workbench_messages.try_iter() {
            session.notify(message.proxy, message.method, message.args);
        }
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

#[cfg(test)]
fn default_settings() -> Arc<Mutex<Settings>> {
    Arc::new(Mutex::new(Settings::load(Registry::core(), PathBuf::from("/nonexistent/User"), None)))
}

fn default_user_data() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/vscode-gpui");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
