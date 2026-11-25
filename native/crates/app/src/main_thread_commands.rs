//! The main-thread side of `vscode.commands`: workbench/api/browser/mainThreadCommands.ts.

use exthost::session::ForwardedRequest;
use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::commands::{Outcome, error_value};
use crate::workbench::Workbench;

impl Workbench {
    /// Answers one forwarded host request; the reply goes back to the host once the outcome is ready.
    pub fn handle_host_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        match (request.proxy, request.method.as_str()) {
            ("MainThreadQuickOpen", _) | ("", "$cancel") => self.handle_quick_open_request(request, window, cx),
            ("MainThreadMessageService", "$showMessage") => self.show_extension_message(&request.args, window, cx),
            ("MainThreadProgress", "$startProgress") => self.start_progress(&request.args, window, cx),
            ("MainThreadProgress", "$progressReport") => self.report_progress(&request.args, window, cx),
            ("MainThreadProgress", "$progressEnd") => self.end_progress(&request.args, window, cx),
            ("MainThreadWebviews" | "MainThreadWebviewPanels", method) => self.handle_webview_request(method, &request.args, window, cx),
            ("MainThreadSCM", method) => self.handle_scm_request(method, &request.args, window, cx),
            ("MainThreadStatusBar", "$setEntry") => self.set_extension_status_entry(&request.args, cx),
            ("MainThreadStatusBar", "$disposeEntry") => self.dispose_extension_status_entry(&request.args, cx),
            ("MainThreadOutputService" | "MainThreadLogger", _) => self.handle_output_request(request, window, cx),
            ("MainThreadCommands", "$registerCommand") => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                self.commands.register(id.clone(), move |workbench, args, _, _| {
                    let mut call_args = vec![json!(id)];
                    call_args.extend(args);
                    Outcome::Pending(workbench.host.call("ExtHostCommands", "$executeContributedCommand", call_args))
                });
                Outcome::done()
            }
            ("MainThreadCommands", "$unregisterCommand") => {
                self.commands.unregister(arg(0).as_str().unwrap_or_default());
                Outcome::done()
            }
            ("MainThreadCommands", "$fireCommandActivationEvent") => {
                // Not awaited: drive-by activation for commands the host already knows.
                drop(self.host.activate_by_event(&format!("onCommand:{}", arg(0).as_str().unwrap_or_default())));
                Outcome::done()
            }
            ("MainThreadCommands", "$executeCommand") => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                let args = arg(1).as_array().cloned().unwrap_or_default();
                let retry = arg(2).as_bool().unwrap_or(false);
                if retry && !args.is_empty() && self.commands.get(&id).is_none() {
                    return retry_after_activation(self.host.activate_by_event(&format!("onCommand:{id}")), cx);
                }
                self.execute_command(&id, args, window, cx)
            }
            ("MainThreadFileSystemEventService", "$confirmParticipantEdits") => {
                let names = arg(1).as_array().cloned().unwrap_or_default();
                self.confirm_participant_edits(arg(0).as_u64().unwrap_or_default(), &names, arg(2) == true, window, cx)
            }
            ("MainThreadWorkspace", "$save") => self.save_resource(&arg(0), cx),
            ("MainThreadWorkspace", "$saveAll") => self.save_all(arg(0) == true, cx),
            ("MainThreadEditorTabs", "$moveTab") => self.move_tab(arg(0).as_str().unwrap_or_default(), arg(1).as_i64().unwrap_or(-1), arg(2).as_i64().unwrap_or(-1), cx),
            ("MainThreadEditorTabs", "$closeTab") => self.close_tabs(arg(0).as_array().map_or(&[], Vec::as_slice), window, cx),
            ("MainThreadEditorTabs", "$closeGroup") => self.close_groups(arg(0).as_array().map_or(&[], Vec::as_slice), window, cx),
            ("workbench", "applyTextEdits") => Outcome::Ready(self.apply_workspace_text_edits(&arg(0), &arg(1), cx)),
            ("workbench", "didRunFileOperation") => {
                self.models_after_file_operation(arg(0).as_u64().unwrap_or_default(), &arg(1), cx);
                Outcome::done()
            }
            ("MainThreadDiagnostics", "$changeMany") => {
                self.change_many_markers(arg(0).as_str().unwrap_or_default(), &arg(1), cx);
                Outcome::done()
            }
            ("MainThreadDiagnostics", "$clear") => {
                self.clear_markers(arg(0).as_str().unwrap_or_default(), cx);
                Outcome::done()
            }
            ("MainThreadCommands", "$getCommands") => Outcome::Ready(Ok(json!(self.commands.ids().collect::<Vec<_>>()))),
            _ => Outcome::done(),
        }
    }
}

/// The host runs the command again once `onCommand:<id>` activated the extension that registers it.
fn retry_after_activation(activation: oneshot::Receiver<Result<Value, Value>>, cx: &mut Context<Workbench>) -> Outcome {
    let (sender, receiver) = oneshot::channel();
    cx.background_executor()
        .spawn(async move {
            let _ = activation.await;
            let _ = sender.send(Err(error_value("$executeCommand:retry")));
        })
        .detach();
    Outcome::Pending(receiver)
}

/// `_generateCommandsDocumentation`: the markdown of every command with metadata, printed to the log.
pub fn register_commands_documentation(workbench: &mut Workbench) {
    workbench.commands.register("_generateCommandsDocumentation", |workbench, _, _, cx| {
        let metadata = workbench.host.call("ExtHostCommands", "$getContributedCommandMetadata", vec![]);
        let (sender, receiver) = oneshot::channel();
        cx.background_executor()
            .spawn(async move {
            let result = metadata.await;
            if let Ok(Ok(Value::Object(commands))) = &result {
                let lines: Vec<String> = commands.iter().map(|(id, description)| format!("`{id}` - {}", command_markdown(description))).collect();
                println!("{}", lines.join("\n"));
            }
            let _ = sender.send(Ok(Value::Null));
        })
        .detach();
        Outcome::Pending(receiver)
    });
}

/// `_generateMarkdown` of mainThreadCommands.ts.
fn command_markdown(description: &Value) -> String {
    if let Some(text) = description.as_str() {
        return text.to_owned();
    }
    let text = description["description"].as_str().or(description["description"]["original"].as_str()).unwrap_or_default();
    let mut parts = vec![text.to_owned(), "\n\n".into()];
    for arg in description["args"].as_array().into_iter().flatten() {
        parts.push(format!("* _{}_ - {}\n", arg["name"].as_str().unwrap_or_default(), arg["description"].as_str().unwrap_or_default()));
    }
    if let Some(returns) = description["returns"].as_str() {
        parts.push(format!("* _(returns)_ - {returns}"));
    }
    parts.push("\n\n".into());
    parts.concat()
}
