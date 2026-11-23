//! The native app. Milestone 1: one GPUI window that starts the extension host and shows the seam working.

use std::path::PathBuf;
use std::time::Duration;

use exthost::rpc::Arg;
use exthost::scan::scan_builtin_extensions;
use exthost::session::{Session, SessionPaths, init_data};
use exthost::{ExtensionHost, HostOptions, Runtime};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size};
use gpui_platform::application;
use serde_json::json;

struct Workbench {
    status: Vec<SharedString>,
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .p_4()
            .gap_1()
            .bg(rgb(0x1f1f1f))
            .text_color(rgb(0xcccccc))
            .text_sm()
            .children(self.status.iter().cloned())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let runtime = if args.iter().any(|arg| arg == "--node") { Runtime::Node } else { Runtime::Bun };
    let user_data = args.iter().position(|arg| arg == "--user-data-dir").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() }, |_, cx| {
            cx.new(|cx| {
                let (sender, mut receiver) = unbounded::<String>();
                std::thread::Builder::new()
                    .name("exthost-session".into())
                    .spawn(move || {
                        if let Err(error) = run_extension_host(runtime, user_data, &sender) {
                            let _ = sender.unbounded_send(format!("extension host failed: {error:#}"));
                        }
                    })
                    .expect("spawn exthost session thread");
                cx.spawn(async move |this, cx| {
                    while let Some(line) = receiver.next().await {
                        let updated = this.update(cx, |workbench: &mut Workbench, cx| {
                            workbench.status.push(line.into());
                            cx.notify();
                        });
                        if updated.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                Workbench { status: vec![format!("starting extension host on {runtime:?}").into()] }
            })
        })
        .unwrap();
        cx.activate(true);
    });
}

/// Owns the session for the life of the app: handshake, one call each way, then serves host requests.
fn run_extension_host(runtime: Runtime, user_data: Option<PathBuf>, status: &UnboundedSender<String>) -> anyhow::Result<()> {
    let app_root = std::env::current_dir()?;
    let extensions_dir = std::env::var_os("VSCODE_GPUI_EXTENSIONS").map(PathBuf::from).unwrap_or_else(|| app_root.join("extensions"));
    let user_data = match user_data {
        Some(dir) => dir,
        None => default_user_data()?,
    };
    let extensions = scan_builtin_extensions(&extensions_dir)?;
    let paths = SessionPaths { app_root: &app_root, user_data: &user_data, workspace_folder: None };
    let init = init_data(&paths, &extensions, "1.147.0");
    let entry = app_root.join("out-exthost/bootstrap-exthost.js");

    let host = ExtensionHost::start(HostOptions { runtime, entry: &entry, extra_env: vec![], extra_args: vec![] }, &init)?;
    let _ = status.unbounded_send(format!("handshake done, pid {}", host.child.id()));
    let mut session = Session::new(host);
    session.start(&extensions, None)?;
    let reply = session.call("ExtHostExtensionService", "$test_latency", vec![Arg::Json(json!(7))], Duration::from_secs(10))?;
    let _ = status.unbounded_send(format!("main -> host: $test_latency(7) = {reply:?}"));
    let _ = status.unbounded_send(format!("host -> main: {} requests so far", session.log.host_calls.values().sum::<u32>()));
    let _ = status.unbounded_send(format!("eager extensions activated: {}", session.log.activated.join(", ")));
    while session.pump(Duration::from_secs(1))? {}
    let _ = status.unbounded_send("extension host exited".into());
    Ok(())
}

fn default_user_data() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/vscode-gpui");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
