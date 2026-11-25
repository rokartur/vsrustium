//! `workbench.action.restartExtensionHost` and the restart a profile switch needs (`stopExtensionHosts`, then
//! `startExtensionHosts`): the host is told to terminate, what it registered in the workbench is disposed as each
//! MainThread* `dispose` does, and a new host starts with the window's current extensions, settings and storage.

use std::sync::mpsc::{Sender, channel};

use config::configuration::Overrides;
use exthost::extension_development::ExtensionDevelopment;
use exthost::extension_management::{WindowExtensions, add_user_extensions};
use exthost::main_thread_storage::MainThreadStorage;
use exthost::scan::ScannedExtension;
use exthost::session::ForwardedRequest;
use exthost::Runtime;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::{Context, Window};
use serde_json::Value;

use super::Workbench;
use crate::commands::Outcome;
use crate::host_sync::HostSync;
use crate::main_thread::{HostRequest, WorkbenchMainThread};
use crate::markers::ReadOptions;


impl Workbench {
    pub(super) fn register_extension_host_commands(&mut self) {
        self.commands.register("workbench.action.restartExtensionHost", |workbench, _, window, cx| {
            workbench.restart_extension_host(window, cx);
            Outcome::done()
        });
    }

    /// `stopExtensionHosts` then `startExtensionHosts`, with the user extensions of the current profile.
    pub(crate) fn restart_extension_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_extension_host(cx);
        let extensions = self.extensions_to_run();
        self.set_running_extensions_after_restart(extensions.clone(), cx);
        self.start_extension_host(extensions, window, cx);
    }

    fn stop_extension_host(&mut self, cx: &mut Context<Self>) {
        self.host.stop_extension_host();
        // MainThreadCommands: the commands the host registered.
        for (id, registration) in std::mem::take(&mut self.extension_commands) {
            self.commands.dispose(&id, registration);
        }
        // MainThreadLanguageFeatures, MainThreadSCM, MainThreadWebview*, MainThreadUrls, MainThreadUriOpeners.
        self.language_features = Default::default();
        self.folding_providers = Default::default();
        self.scm.model = Default::default();
        self.webview_views = Default::default();
        self.url_handlers = Default::default();
        self.urls.openers.clear();
        // MainThreadDiagnostics: `changeAll(owner, [])` for each owner the host used.
        let host_markers = self.diagnostics.service.read(ReadOptions::default());
        let mut owners: Vec<String> = host_markers.iter().filter(|marker| marker.origin.as_deref() == Some(super::diagnostics::EXT_HOST_ID)).map(|marker| marker.owner.clone()).collect();
        owners.dedup();
        for owner in owners {
            self.clear_markers(&owner, cx);
        }
        self.trigger_folding_model_changed(cx);
    }

    /// The installed extensions less those disabled in the profile or workspace and those an untrusted workspace
    /// turns off, as main.rs picks them at startup.
    fn extensions_to_run(&mut self) -> Vec<ScannedExtension> {
        let viewlet = &self.extensions_viewlet;
        let mut installed = viewlet.builtins.clone();
        match viewlet.user.as_ref().map(|user| user.scan()).transpose() {
            Ok(user) => add_user_extensions(&mut installed, user.unwrap_or_default()),
            Err(error) => eprintln!("[extensions] scanning the user extensions: {error:#}"),
        }
        let development = development_without_tests();
        if let Some(development) = &development
            && let Err(error) = development.add_extension(&mut installed)
        {
            eprintln!("[extensions] {error:#}");
        }
        let developed = viewlet.development.clone();
        let disabled = {
            let mut profile = self.profile_storage.lock().unwrap();
            let mut workspace = self.workspace_storage.lock().unwrap();
            super::extensions_view::disabled_at_startup(&installed, &mut profile, self.folder.is_some().then_some(&mut *workspace), &developed)
        };
        installed.retain(|extension| !disabled.contains(&extension.id));
        if !self.workspace_trust.is_trusted() {
            let configured = self.settings.lock().unwrap().get_value(Some("extensions.supportUntrustedWorkspaces"), &Overrides::default()).unwrap_or_default();
            let workspace = self.workspace().cloned();
            installed.retain(|extension| !crate::workspace_trust::disabled_when_untrusted(extension, workspace.as_ref(), &configured));
        }
        installed
    }

    fn set_running_extensions_after_restart(&mut self, extensions: Vec<ScannedExtension>, cx: &mut Context<Self>) {
        let described: Vec<(&str, &Value)> = extensions.iter().map(|extension| (extension.id.as_str(), &extension.description)).collect();
        let before = self.model_creation_options();
        let change = self.settings.lock().unwrap().set_registry(config::registry::Registry::with_extensions(&described));
        self.status_entries = crate::main_thread_status_bar::static_entries(&extensions);
        self.set_running_extensions(extensions, cx);
        self.configuration_changed(&change, before, cx);
    }

    /// The channels, session thread and workbench loops of a new host, as main.rs starts the first one.
    fn start_extension_host(&mut self, extensions: Vec<ScannedExtension>, window: &mut Window, cx: &mut Context<Self>) {
        let (status, mut status_lines) = unbounded::<String>();
        let (host_sender, host_receiver) = channel();
        let (request_sender, requests) = unbounded::<ForwardedRequest>();
        let (reply_sender, replies) = channel();
        let (main_thread_sender, main_thread_requests) = unbounded::<HostRequest>();
        let user = self.extensions_viewlet.user.as_ref().expect("the user extensions are set at startup").clone_for_profile();
        let window_extensions = WindowExtensions { builtins: self.extensions_viewlet.builtins.clone(), user, running: extensions.clone() };
        let main_thread = WorkbenchMainThread { requests: main_thread_sender, messages: host_receiver, waiting: Vec::new(), content_providers: Default::default(), extensions: window_extensions };
        let mut host = HostSync::new(host_sender);
        host.configuration_scopes = self.host.configuration_scopes;
        host.untitled_names_from_content = self.host.untitled_names_from_content;
        host.search_tabs = std::mem::take(&mut self.host.search_tabs);
        host.send_color_theme(self.theme.kind);
        host.send_language_ids(&self.languages);
        self.host = host;
        self.host.send_changes(&self.editors);

        let runtime = if std::env::args().any(|arg| arg == "--node") { Runtime::Node } else { Runtime::Bun };
        let app_root = self.app_root.clone();
        let settings = self.settings.clone();
        let user_data = settings.lock().unwrap().user_home().parent().expect("the user home is <user data>/User").to_path_buf();
        let storage = MainThreadStorage { profile: self.profile_storage.clone(), workspace: self.workspace_storage.clone() };
        let trusted = self.workspace_trust.is_trusted();
        let workspace = self.workspace().cloned();
        std::thread::Builder::new()
            .name("exthost-session".into())
            .spawn(move || {
                let development = development_without_tests();
                let started = crate::run_extension_host(runtime, app_root, extensions, settings, storage, trusted, user_data, workspace, development, main_thread, request_sender, replies, &status);
                if let Err(error) = started {
                    let _ = status.unbounded_send(format!("extension host failed: {error:#}"));
                }
            })
            .expect("spawn exthost session thread");
        serve_host_requests(requests, reply_sender, window, cx);
        serve_main_thread_requests(main_thread_requests, window, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            while let Some(line) = status_lines.next().await {
                eprintln!("{line}");
                if line.starts_with(crate::HANDSHAKE_PREFIX) {
                    let _ = workbench.update(cx, |workbench, _| workbench.host.installed_extensions_registered = true);
                }
            }
        })
        .detach();
    }
}

/// The extension under development without its tests: a restarted host does not run them again.
fn development_without_tests() -> Option<ExtensionDevelopment> {
    let args: Vec<String> = std::env::args().collect();
    let development = ExtensionDevelopment::from_args(&args).ok().flatten()?;
    Some(ExtensionDevelopment { tests_path: None, ..development })
}

/// Answers the host requests the session forwards to the workbench, each on the window's thread.
pub fn serve_host_requests(mut requests: UnboundedReceiver<ForwardedRequest>, reply_sender: Sender<(u32, Result<Value, Value>)>, window: &mut Window, cx: &mut Context<Workbench>) {
    cx.spawn_in(window, async move |workbench, cx| {
        while let Some(request) = requests.next().await {
            let req = request.req;
            // A cancellation answers nothing itself; the request it cancels gets its answer. An observed call
            // was answered by the session.
            let cancel = request.method == "$cancel" || req == exthost::session::OBSERVED_REQ;
            let Ok(outcome) = workbench.update_in(cx, |workbench, window, cx| workbench.handle_host_request(request, window, cx)) else { break };
            let result = match outcome {
                _ if cancel => continue,
                Outcome::Ready(result) => result,
                // Requests from the host may overlap, so a pending answer is awaited off this loop.
                Outcome::Pending(pending) => {
                    let reply_sender = reply_sender.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let _ = reply_sender.send((req, pending.await.unwrap_or(Ok(Value::Null))));
                        })
                        .detach();
                    continue;
                }
            };
            let _ = reply_sender.send((req, result));
        }
    })
    .detach();
}

/// Runs the MainThread* calls the workbench implements (`WorkbenchMainThread`) on the window's thread.
pub fn serve_main_thread_requests(mut requests: UnboundedReceiver<HostRequest>, window: &mut Window, cx: &mut Context<Workbench>) {
    cx.spawn_in(window, async move |workbench, cx| {
        while let Some(request) = requests.next().await {
            let result = workbench.update_in(cx, |workbench, window, cx| workbench.host_request(&request.proxy, &request.method, &request.args, window, cx));
            let _ = request.reply.send(result.unwrap_or_else(|_| Err(exthost::main_thread::serialized_error("Canceled"))));
        }
    })
    .detach();
}


