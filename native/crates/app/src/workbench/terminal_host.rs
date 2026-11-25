//! mainThreadTerminalService.ts and mainThreadTerminalShellIntegration.ts: the extension host's terminals
//! (`window.createTerminal`, pseudoterminals, `sendText`, the terminal events) and shell integration
//! (`TerminalShellIntegration`, the execution events), over the Terminal view's instances.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use exthost::session::ForwardedRequest;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::terminal_view::{EXIT_REASON_EXTENSION, EXIT_REASON_PROCESS};
use crate::commands::Outcome;
use crate::terminal::environment_collection::{EnvironmentCollections, STORAGE_KEY};
use crate::terminal::shell_integration::{Command, ShellEvent};
use crate::terminal::{Processed, ShellLaunchConfig, TerminalInstance, injection};

/// What the host last heard of a terminal, so a change is sent once.
#[derive(Default, PartialEq)]
pub(super) struct Reported {
    title: String,
    dimensions: (usize, usize),
    shell_type: Option<&'static str>,
    selection: Option<String>,
    /// The command whose start was sent, so a double event is sent once (`currentCommand`).
    executing: Option<Command>,
    /// The executing command's output, for `ITerminalCommandDto.output`.
    output: String,
}

impl Workbench {
    /// The constructors of the two main thread actors: restored environment variable collections and the default
    /// profile go to the host once at startup.
    pub fn start_terminal_host(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(STORAGE_KEY).map(str::to_owned);
        let collections = EnvironmentCollections::load(stored.as_deref());
        if !collections.is_empty() {
            self.host.send("ExtHostTerminalService", "$initEnvironmentVariableCollections", vec![collections.serialized()]);
        }
        self.terminal_view.host.environment_collections = collections;
        let profile = self.default_terminal_profile();
        let dto = json!({
            "profileName": profile.name,
            "path": profile.path,
            "args": profile.args,
            "env": profile.env,
            "isDefault": true,
            "isAutoDetected": profile.auto_detected,
            "icon": profile.icon,
            "color": profile.color,
        });
        // `allowAutomationShell` resolves `terminal.integrated.automationProfile.osx`, else the same default.
        let automation = self.setting("terminal.integrated.automationProfile.osx");
        let automation = if automation.is_object() { json!({ "profileName": "automationProfile", "path": automation["path"], "args": automation["args"], "isDefault": true }) } else { dto.clone() };
        self.host.send("ExtHostTerminalService", "$acceptDefaultProfile", vec![dto, automation]);
    }

    pub fn handle_terminal_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        match (request.proxy, request.method.as_str()) {
            ("MainThreadTerminalService", "$createTerminal") => {
                let launch = self.extension_launch_config(arg(0).as_str().unwrap_or_default(), &arg(1));
                let split_from = self.extension_terminal_parent(&arg(1)["location"]);
                self.add_terminal(launch, split_from, window, cx);
            }
            ("MainThreadTerminalService", "$show") => {
                if let Some(id) = self.extension_terminal_id(&arg(0)) {
                    self.show_extension_terminal(id, arg(1) == true, window, cx);
                }
            }
            ("MainThreadTerminalService", "$hide") => {
                // A terminal in the editor area is not hidden (`target !== TerminalLocation.Editor`); the port's
                // view is an editor tab, so hiding closes the tab while the terminals keep running.
                if self.extension_terminal_id(&arg(0)).is_some_and(|id| self.terminal_view.active_instance_id() == Some(id)) {
                    self.close_terminal_view(cx);
                }
            }
            ("MainThreadTerminalService", "$dispose") => {
                if let Some(id) = self.extension_terminal_id(&arg(0)) {
                    if let Some(instance) = self.terminal_view.instance_mut(id) {
                        instance.kill();
                    }
                    self.dispose_terminal(id, EXIT_REASON_EXTENSION, window, cx);
                }
            }
            ("MainThreadTerminalService", "$sendText") => {
                if let Some(instance) = self.extension_terminal_id(&arg(0)).and_then(|id| self.terminal_view.instance_mut(id)) {
                    instance.send_text(arg(1).as_str().unwrap_or_default(), arg(2) == true);
                }
                cx.notify();
            }
            ("MainThreadTerminalService", "$startSendingDataEvents") => self.terminal_view.host.data_events = true,
            ("MainThreadTerminalService", "$stopSendingDataEvents") => self.terminal_view.host.data_events = false,
            ("MainThreadTerminalService", "$startSendingCommandEvents") => self.terminal_view.host.command_events = true,
            ("MainThreadTerminalService", "$stopSendingCommandEvents") => self.terminal_view.host.command_events = false,
            ("MainThreadTerminalService", "$setEnvironmentVariableCollection") => {
                let collection = Some(arg(2)).filter(|collection| !collection.is_null());
                let collections = &mut self.terminal_view.host.environment_collections;
                collections.set(arg(0).as_str().unwrap_or_default(), arg(1) == true, collection.as_ref(), &arg(3));
                // `_persistCollectionsEventually` debounces; the port's storage writes are cheap, so it writes now.
                let serialized = collections.serialize_persistent();
                self.workspace_storage.lock().unwrap().set(STORAGE_KEY, serialized);
            }
            ("MainThreadTerminalService", "$sendProcessData") => {
                let data = arg(1).as_str().unwrap_or_default().to_owned();
                self.terminal_data(arg(0).as_u64().unwrap_or_default() as u32, data.as_bytes(), cx);
            }
            ("MainThreadTerminalService", "$sendProcessReady") => {
                let id = arg(0).as_u64().unwrap_or_default() as u32;
                let pid = arg(1).as_i64().map(|pid| pid as i32);
                if let Some(instance) = self.terminal_view.instance_mut(id) {
                    instance.pid = pid;
                }
                if let Some(pid) = pid {
                    self.host.send("ExtHostTerminalService", "$acceptTerminalProcessId", vec![json!(id), json!(pid)]);
                }
            }
            ("MainThreadTerminalService", "$sendProcessProperty") => {
                let (id, property) = (arg(0).as_u64().unwrap_or_default() as u32, arg(1));
                if property["type"] == "title"
                    && let Some(title) = property["value"].as_str()
                {
                    self.rename_terminal(id, title.to_owned());
                    cx.notify();
                }
            }
            ("MainThreadTerminalService", "$sendProcessExit") => {
                let id = arg(0).as_u64().unwrap_or_default() as u32;
                let code = arg(1).as_i64().map(|code| code as i32);
                if let Some(instance) = self.terminal_view.instance_mut(id) {
                    instance.exited(code);
                }
                self.terminal_process_exited(id, code, window, cx);
            }
            ("MainThreadTerminalShellIntegration", "$executeCommand") => {
                let id = arg(0).as_u64().unwrap_or_default() as u32;
                self.run_terminal_command(id, arg(1).as_str().unwrap_or_default().to_owned(), window, cx);
            }
            ("MainThreadTerminalService", "$registerCompletionProvider") => self.register_terminal_completion_provider(&request.args),
            ("MainThreadTerminalService", "$unregisterCompletionProvider") => self.unregister_terminal_completion_provider(arg(0).as_str().unwrap_or_default()),
            // Profile, quick fix and link providers register here; the port has no UI that asks them yet.
            _ => {}
        }
        Outcome::done()
    }

    /// mainThreadTerminalService.ts `$createTerminal` with terminalProfileResolverService.ts
    /// `resolveShellLaunchConfig`: no `shellPath` means the default profile; `ignoreConfigurationCwd` is set.
    fn extension_launch_config(&self, ext_host_terminal_id: &str, config: &Value) -> ShellLaunchConfig {
        let launch_env: HashMap<String, Option<String>> = config["env"].as_object().into_iter().flatten().map(|(key, value)| (key.clone(), value.as_str().map(str::to_owned))).collect();
        let (executable, args, profile_env) = match config["shellPath"].as_str() {
            Some(path) => {
                let args = match &config["shellArgs"] {
                    Value::Array(args) => args.iter().filter_map(|arg| arg.as_str().map(str::to_owned)).collect(),
                    Value::String(arg) => vec![arg.clone()],
                    _ => Vec::new(),
                };
                (PathBuf::from(path), args, HashMap::new())
            }
            None => {
                let profile = self.default_terminal_profile();
                (profile.path, profile.args, profile.env)
            }
        };
        let cwd = match &config["cwd"] {
            Value::String(cwd) => Some(PathBuf::from(cwd)),
            Value::Object(uri) => uri.get("path").and_then(Value::as_str).map(PathBuf::from),
            _ => None,
        };
        let cwd = cwd.unwrap_or_else(|| self.terminal_cwd_without_setting());
        let env = if config["strictEnv"] == true {
            launch_env.iter().filter_map(|(key, value)| Some((key.clone(), value.clone()?))).collect()
        } else {
            let mut merged = profile_env;
            merged.extend(launch_env);
            self.terminal_environment(&merged)
        };
        let extension_pty = config["isExtensionCustomPtyTerminal"] == true;
        ShellLaunchConfig {
            name: config["name"].as_str().map(str::to_owned),
            executable: if extension_pty { PathBuf::new() } else { executable },
            args: if extension_pty { Vec::new() } else { args },
            cwd,
            env,
            icon: config["icon"]["id"].as_str().map(str::to_owned),
            color: config["color"].as_str().map(str::to_owned),
            ext_host_terminal_id: Some(ext_host_terminal_id.to_owned()),
            hide_from_user: config["hideFromUser"] == true,
            extension_pty,
            is_feature_terminal: config["isFeatureTerminal"] == true,
            nonce: config["shellIntegrationNonce"].as_str().unwrap_or_default().to_owned(),
        }
    }

    /// `getCwd` with `ignoreConfigurationCwd`: the workspace folder, else the home directory.
    fn terminal_cwd_without_setting(&self) -> PathBuf {
        let root = self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        root.unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")))
    }

    /// `_deserializeParentTerminal`: `{ parentTerminal }` or `{ splitActiveTerminal: true }` split beside it.
    fn extension_terminal_parent(&self, location: &Value) -> Option<u32> {
        if location["splitActiveTerminal"] == true {
            return self.terminal_view.active_instance_id();
        }
        self.extension_terminal_id(&location["parentTerminal"])
    }

    /// `_getTerminalInstance`: an extension host id string or an instance id.
    fn extension_terminal_id(&self, id: &Value) -> Option<u32> {
        match id {
            Value::String(ext_host_id) => self.terminal_view.instances.iter().find(|instance| instance.launch.ext_host_terminal_id.as_deref() == Some(ext_host_id)).map(|instance| instance.id),
            Value::Number(id) => id.as_u64().map(|id| id as u32).filter(|id| self.terminal_view.instance(*id).is_some()),
            _ => None,
        }
    }

    /// `$show`: the terminal becomes active (a hidden one joins the view as its own group) and the view shows it.
    fn show_extension_terminal(&mut self, id: u32, preserve_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_view.show_background(id);
        self.terminal_view.set_active(id);
        let had_focus = window.focused(cx);
        self.show_terminal_tab(window, cx);
        if preserve_focus && let Some(focus) = had_focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// The parts of terminalProcessManager.ts's launch that need the workbench: a nonce, the extensions'
    /// environment variable collections and the shell integration injection.
    pub(super) fn prepare_terminal_launch(&self, launch: &mut ShellLaunchConfig) {
        if launch.nonce.is_empty() {
            launch.nonce = exthost::main_thread_storage::generate_uuid();
        }
        if launch.extension_pty {
            return;
        }
        let folder_index = self.workspace().and_then(|workspace| workspace.folders.iter().position(|folder| launch.cwd.starts_with(&folder.path))).map(|index| index as u64);
        self.terminal_view.host.environment_collections.apply(&mut launch.env, folder_index);
        let enabled = self.setting("terminal.integrated.shellIntegration.enabled") == true;
        if !enabled || launch.is_feature_terminal {
            return;
        }
        let scripts = self.app_root.join("src/vs/workbench/contrib/terminal/common/scripts");
        let home = std::env::var("HOME").unwrap_or_default();
        let user_zdotdir = launch.env.get("ZDOTDIR").cloned().unwrap_or(home);
        let user = std::env::var("USER").unwrap_or_default();
        // terminalProcess.ts: `${tmpdir}/${username}-${applicationName}-zsh`.
        let zdotdir = std::env::temp_dir().join(format!("{user}-vsrustium-zsh"));
        let path_prefix = launch.env.get("VSCODE_PATH_PREFIX").cloned().unwrap_or_default();
        let Some(injection) = injection::injection(&launch.executable, &launch.args, &launch.nonce, &scripts, &path_prefix, &zdotdir, &user_zdotdir) else { return };
        for (source, dest) in &injection.files_to_copy {
            let copied = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::copy(source, dest));
            if let Err(error) = copied {
                eprintln!("[terminal] shell integration: copying {} failed: {error}", source.display());
            }
        }
        launch.args = injection.args;
        launch.env.extend(injection.env);
    }

    /// The callbacks a new instance reports its input through: `$acceptTerminalInteraction`, and for an
    /// extension pty `$acceptProcessInput`.
    pub(super) fn connect_terminal_to_host(&self, instance: &mut TerminalInstance) {
        let id = instance.id;
        let send = self.host.notifier();
        instance.on_input = Some(Box::new(move || send("ExtHostTerminalService", "$acceptTerminalInteraction", vec![json!(id)])));
        if instance.launch.extension_pty {
            let send = self.host.notifier();
            instance.process_input = Some(Box::new(move |data: &str| send("ExtHostTerminalService", "$acceptProcessInput", vec![json!(id), json!(data)])));
        }
    }

    /// `onDidCreateInstance`: opened, its process id, its dimensions, then for an extension pty the start request.
    pub(super) fn terminal_opened(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let launch = &instance.launch;
        let dto = json!({
            "name": launch.name,
            "executable": (!launch.extension_pty).then(|| launch.executable.clone()),
            "args": launch.args,
            "cwd": launch.cwd,
            "hideFromUser": launch.hide_from_user,
        });
        let (title, pid, cols, rows) = (instance.title(), instance.pid, instance.term.columns(), instance.term.screen_lines());
        let extension_pty = launch.extension_pty;
        self.host.send("ExtHostTerminalService", "$acceptTerminalOpened", vec![json!(id), json!(launch.ext_host_terminal_id), json!(title), dto]);
        if let Some(pid) = pid {
            self.host.send("ExtHostTerminalService", "$acceptTerminalProcessId", vec![json!(id), json!(pid)]);
        }
        self.sync_terminals_to_host();
        if extension_pty {
            let started = self.host.call("ExtHostTerminalService", "$startExtensionTerminal", vec![json!(id), json!({ "columns": cols, "rows": rows })]);
            cx.spawn(async move |workbench, cx| {
                // `ITerminalLaunchError`: the message shows in the terminal and the process counts as exited.
                let Ok(Ok(error)) = started.await else { return };
                let Some(message) = error["message"].as_str().map(str::to_owned) else { return };
                let _ = workbench.update(cx, |workbench, cx| {
                    let colors = crate::terminal::colors::TerminalColors::new(&workbench.theme);
                    if let Some(instance) = workbench.terminal_view.instance_mut(id) {
                        instance.write_message(&message, &colors);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// `onDidDisposeInstance`: an extension pty hears its shutdown, then the host the close and the end of its
    /// shell integration.
    pub(super) fn terminal_closed(&mut self, instance: TerminalInstance, exit_reason: u8) {
        let id = instance.id;
        if instance.launch.extension_pty && exit_reason != EXIT_REASON_PROCESS {
            self.host.send("ExtHostTerminalService", "$acceptProcessShutdown", vec![json!(id), json!(true)]);
        }
        let exit_code = instance.exit_code.flatten().map_or_else(exthost::rpc::undefined, |code| json!(code));
        self.host.send("ExtHostTerminalService", "$acceptTerminalClosed", vec![json!(id), exit_code, json!(exit_reason)]);
        self.host.send("ExtHostTerminalShellIntegration", "$closeTerminal", vec![json!(id)]);
        self.terminal_view.host.reported.remove(&id);
        self.sync_terminals_to_host();
    }

    /// `onData` and the shell integration capability events of one chunk, in stream order.
    pub(super) fn terminal_output_to_host(&mut self, id: u32, processed: Processed) {
        if self.terminal_view.host.data_events && !processed.text.is_empty() {
            self.host.send("ExtHostTerminalService", "$acceptTerminalProcessData", vec![json!(id), json!(processed.text)]);
        }
        let Some(instance) = self.terminal_view.instance(id) else { return };
        let supports_execute_command = !instance.launch.is_feature_terminal;
        let shell_type = instance.shell_type;
        let cwd = instance.shell_integration.cwd().map(str::to_owned);
        for event in processed.shell_events {
            match event {
                ShellEvent::CommandDetectionAdded => {
                    // `_enableShellIntegration`.
                    drop(self.host.activate_by_event("onTerminalShellIntegration:*"));
                    if let Some(shell_type) = shell_type {
                        drop(self.host.activate_by_event(&format!("onTerminalShellIntegration:{shell_type}")));
                    }
                    self.host.send("ExtHostTerminalShellIntegration", "$shellIntegrationChange", vec![json!(id), json!(supports_execute_command)]);
                    if let Some(cwd) = &cwd {
                        self.host.send("ExtHostTerminalShellIntegration", "$cwdChange", vec![json!(id), json!(cwd)]);
                    }
                }
                ShellEvent::Cwd(cwd) => self.host.send("ExtHostTerminalShellIntegration", "$cwdChange", vec![json!(id), json!(cwd)]),
                ShellEvent::CommandExecuted(command) => {
                    let reported = self.terminal_view.host.reported.entry(id).or_default();
                    if reported.executing.as_ref() == Some(&command) {
                        continue;
                    }
                    reported.executing = Some(command.clone());
                    reported.output.clear();
                    let args = vec![json!(id), json!(supports_execute_command), json!(command.command_line), json!(command.confidence), json!(command.is_trusted), json!(command.cwd)];
                    self.host.send("ExtHostTerminalShellIntegration", "$shellExecutionStart", args);
                }
                ShellEvent::ExecutionData(data) => {
                    self.terminal_view.host.reported.entry(id).or_default().output.push_str(&data);
                    self.host.send("ExtHostTerminalShellIntegration", "$shellExecutionData", vec![json!(id), json!(data)]);
                }
                ShellEvent::CommandFinished(command) => {
                    let reported = self.terminal_view.host.reported.entry(id).or_default();
                    reported.executing = None;
                    let output = std::mem::take(&mut reported.output);
                    let args = vec![json!(id), json!(command.command_line), json!(command.confidence), json!(command.is_trusted), json!(command.exit_code)];
                    self.host.send("ExtHostTerminalShellIntegration", "$shellExecutionEnd", args);
                    if self.terminal_view.host.command_events {
                        let dto = json!({ "commandLine": command.command_line, "cwd": command.cwd, "exitCode": command.exit_code, "output": output });
                        self.host.send("ExtHostTerminalService", "$acceptDidExecuteCommand", vec![json!(id), dto]);
                    }
                }
            }
        }
    }

    /// The change events of mainThreadTerminalService.ts, sent for what changed since the host last heard:
    /// the active terminal, titles, dimensions, shell types and selections.
    pub(super) fn sync_terminals_to_host(&mut self) {
        let active = self.terminal_view.active_instance_id();
        if self.terminal_view.host.active != active {
            self.terminal_view.host.active = active;
            self.host.send("ExtHostTerminalService", "$acceptActiveTerminalChanged", vec![json!(active)]);
        }
        let mut messages = Vec::new();
        for instance in &self.terminal_view.instances {
            let id = instance.id;
            let reported = self.terminal_view.host.reported.entry(id).or_default();
            let title = instance.title();
            if reported.title != title {
                messages.push(("$acceptTerminalTitleChange", vec![json!(id), json!(title)]));
                reported.title = title;
            }
            let dimensions = (instance.term.columns(), instance.term.screen_lines());
            if reported.dimensions != dimensions {
                messages.push(("$acceptTerminalDimensions", vec![json!(id), json!(dimensions.0), json!(dimensions.1)]));
                messages.push(("$acceptTerminalMaximumDimensions", vec![json!(id), json!(dimensions.0), json!(dimensions.1)]));
                if instance.launch.extension_pty && reported.dimensions != (0, 0) {
                    messages.push(("$acceptProcessResize", vec![json!(id), json!(dimensions.0), json!(dimensions.1)]));
                }
                reported.dimensions = dimensions;
            }
            if reported.shell_type != instance.shell_type {
                reported.shell_type = instance.shell_type;
                messages.push(("$acceptTerminalShellType", vec![json!(id), json!(instance.shell_type)]));
            }
            let selection = instance.selection_text();
            if reported.selection != selection {
                messages.push(("$acceptTerminalSelection", vec![json!(id), selection.as_ref().map_or_else(exthost::rpc::undefined, |text| json!(text))]));
                reported.selection = selection;
            }
        }
        for (method, args) in messages {
            self.host.send("ExtHostTerminalService", method, args);
        }
    }

    pub(super) fn terminal_resized(&mut self, _id: u32) {
        self.sync_terminals_to_host();
    }

    /// `instance.rename(title)` with `TitleEventSource.Api`.
    pub(super) fn rename_terminal(&mut self, id: u32, title: String) {
        if let Some(instance) = self.terminal_view.instance_mut(id) {
            instance.user_title = Some(title);
        }
        self.sync_terminals_to_host();
    }

    /// terminalInstance.ts `runCommand`: waits for the prompt (`getShellIntegrationTimeout`), then types the line.
    fn run_terminal_command(&mut self, id: u32, command_line: String, window: &mut Window, cx: &mut Context<Self>) {
        let timeout = shell_integration_timeout(&self.setting("terminal.integrated.shellIntegration.timeout"), self.setting("terminal.integrated.shellIntegration.enabled") == true);
        let started = Instant::now();
        cx.spawn_in(window, async move |workbench, cx| {
            loop {
                let ready = workbench.read_with(cx, |workbench, _| workbench.terminal_view.instance(id).map(|instance| instance.shell_integration.prompt_ready()));
                match ready {
                    Ok(Some(false)) if started.elapsed() < timeout => cx.background_executor().timer(Duration::from_millis(20)).await,
                    Ok(Some(_)) => break,
                    _ => return,
                }
            }
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(instance) = workbench.terminal_view.instance_mut(id) else { return };
                // shortcut: the prompt's typed text is not modelled (promptInputModel), so a ^C is sent only
                // without command detection.
                if !instance.shell_integration.has_command_detection {
                    instance.send_text("\x03", false);
                }
                instance.send_text(&command_line, true);
                cx.notify();
            });
        })
        .detach();
    }
}

/// terminalEnvironment.ts `getShellIntegrationTimeout` without the process ready adjustment.
fn shell_integration_timeout(setting: &Value, injection_enabled: bool) -> Duration {
    let millis = match setting.as_f64() {
        // `ShellIntegrationTimeoutOverride.DisableForTests`
        Some(-2.) | Some(0.) => 0.,
        Some(value) if value > 0. => value.max(500.),
        _ if injection_enabled => 5000.,
        _ => 2000.,
    };
    Duration::from_millis(millis as u64)
}
