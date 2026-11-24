//! The native app: the workbench, themed from the built-in theme files, over the running extension host.
//! Usage: code-gpui [folder [file]] [--theme <settings id>] [--user-data-dir <dir>] [--screenshot <png>] [--node]

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
use std::time::Duration;

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
    let user_data = flag_value("--user-data-dir").map(PathBuf::from);
    let screenshot = flag_value("--screenshot").map(PathBuf::from);
    // Settings id of workbench.colorTheme; the reference default theme is not ported yet, so Dark Modern is.
    let theme_id = flag_value("--theme").unwrap_or_else(|| "Dark Modern".into());
    let value_flags = ["--user-data-dir", "--screenshot", "--theme"];
    let positional: Vec<&String> = args.iter().enumerate().skip(1).filter(|(i, arg)| !arg.starts_with("--") && !value_flags.contains(&args[i - 1].as_str())).map(|(_, arg)| arg).collect();
    let canonical = |path: &String| std::fs::canonicalize(path).with_context(|| format!("opening {path}"));
    let folder = positional.first().map(|path| canonical(path)).transpose()?;
    let file = positional.get(1).map(|path| canonical(path)).transpose()?;

    let app_root = std::env::current_dir()?;
    let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
    let extensions = scan_builtin_extensions(&extensions_dir)?;
    let theme = load_theme(&extensions, &theme_id)?;

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
                let mut workbench = Workbench::new(theme, app_root.clone(), folder.clone().map(FileTree::new), languages, host);
                if let Some(file) = file {
                    workbench.open_file(file, true, cx);
                }
                std::thread::Builder::new()
                    .name("exthost-session".into())
                    .spawn(move || {
                        if let Err(error) = run_extension_host(runtime, app_root, extensions, user_data, folder, host_receiver, &sender) {
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
fn run_extension_host(
    runtime: Runtime,
    app_root: PathBuf,
    extensions: Vec<ScannedExtension>,
    user_data: Option<PathBuf>,
    folder: Option<PathBuf>,
    workbench_messages: Receiver<HostMessage>,
    status: &UnboundedSender<String>,
) -> anyhow::Result<()> {
    let user_data = match user_data {
        Some(dir) => dir,
        None => default_user_data()?,
    };
    let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace_folder: folder.as_deref() };
    let init = init_data(&paths, &extensions, "1.147.0");
    let entry = app_root.join("out-exthost/bootstrap-exthost.js");

    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let _ = status.unbounded_send(format!("handshake done, pid {}", host.child.id()));
    let mut session = Session::new(host);
    session.start(&extensions, folder.as_deref().map(|folder| workspace_data(&init, folder)))?;
    let reply = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let _ = status.unbounded_send(format!("main -> host: $test_latency(7) = {reply:?}"));
    let _ = status.unbounded_send(format!("host -> main: {} requests so far", session.log.host_calls.values().sum::<u32>()));
    let _ = status.unbounded_send(format!("{ACTIVATED_PREFIX}{}", session.log.activated.join(", ")));
    // The short pump timeout bounds how long a workbench change waits before the host hears of it.
    loop {
        for message in workbench_messages.try_iter() {
            session.notify(message.proxy, message.method, message.args);
        }
        if !session.pump(Duration::from_millis(20))? {
            break;
        }
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

fn default_user_data() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/vscode-gpui");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
