//! The main-thread side of `vscode.commands`: workbench/api/browser/mainThreadCommands.ts.

use exthost::session::ForwardedRequest;
use futures::channel::oneshot;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::commands::{Outcome, error_value};
use crate::host_sync::Activation;
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
            ("MainThreadDecorations", method) => self.handle_decorations_request(method, &request.args, cx),
            ("MainThreadTreeViews", method) => self.handle_tree_view_request(method, &request.args, window, cx),
            ("MainThreadSCM", method) => self.handle_scm_request(method, &request.args, window, cx),
            ("MainThreadQuickDiff", method) => self.handle_quick_diff_request(method, &request.args, cx),
            ("MainThreadTimeline", method) => self.handle_timeline_request(method, &request.args, cx),
            ("MainThreadComments", method) => self.handle_comments_request(method, &request.args, window, cx),
            ("MainThreadGitHub", method) => self.handle_github_request(method, &request.args, cx),
            ("MainThreadWebviewViews", method) => self.handle_webview_views_request(method, &request.args, window, cx),
            ("MainThreadCustomEditors", method) => self.handle_custom_editors_request(method, &request.args, window, cx),
            ("MainThreadStatusBar", "$setEntry") => self.set_extension_status_entry(&request.args, cx),
            ("MainThreadLanguages", "$setLanguageStatus") => {
                self.set_language_status(request.args[0].as_u64().unwrap_or_default(), request.args[1].clone(), cx);
                Outcome::done()
            }
            ("MainThreadLanguages", "$removeLanguageStatus") => {
                self.remove_language_status(request.args[0].as_u64().unwrap_or_default(), cx);
                Outcome::done()
            }
            ("MainThreadStatusBar", "$disposeEntry") => self.dispose_extension_status_entry(&request.args, cx),
            ("MainThreadOutputService" | "MainThreadLogger", _) => self.handle_output_request(request, window, cx),
            ("MainThreadConsole" | "MainThreadErrors" | "MainThreadExtensionService", _) => self.handle_observed_request(&request),
            ("MainThreadDebugService", _) => self.handle_debug_request(request, window, cx),
            ("MainThreadTesting", _) => self.handle_testing_request(request, cx),
            ("MainThreadTerminalService" | "MainThreadTerminalShellIntegration", _) => self.handle_terminal_request(request, window, cx),
            ("MainThreadTask", _) => self.handle_task_request(request, window, cx),
            ("MainThreadCommands", "$registerCommand") => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                let handler_id = id.clone();
                let registration = self.commands.register(id.clone(), move |workbench, args, _, _| {
                    let mut call_args = vec![json!(handler_id)];
                    call_args.extend(args);
                    Outcome::Pending(workbench.host.call("ExtHostCommands", "$executeContributedCommand", call_args))
                });
                // DisposableMap.set disposes the registration it replaces.
                if let Some(replaced) = self.extension_commands.insert(id.clone(), registration) {
                    self.commands.dispose(&id, replaced);
                }
                Outcome::done()
            }
            ("MainThreadCommands", "$unregisterCommand") => {
                let id = arg(0).as_str().unwrap_or_default().to_owned();
                if let Some(registration) = self.extension_commands.remove(&id) {
                    self.commands.dispose(&id, registration);
                }
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
            ("MainThreadDownloadService", "$download") => self.download_resource(&arg(0), &arg(1), cx),
            ("MainThreadWorkspace", "$save") => self.save_resource(&arg(0), cx),
            ("MainThreadWorkspace", "$saveAll") => self.save_all(arg(0) == true, cx),
            ("MainThreadWorkspace", "$updateWorkspaceFolders") => self.api_update_workspace_folders(&request.args, window, cx),
            ("MainThreadEditorTabs", "$moveTab") => self.move_tab(arg(0).as_str().unwrap_or_default(), arg(1).as_i64().unwrap_or(-1), arg(2).as_i64().unwrap_or(-1), cx),
            ("MainThreadEditorTabs", "$closeTab") => self.close_tabs(arg(0).as_array().map_or(&[], Vec::as_slice), window, cx),
            ("MainThreadEditorTabs", "$closeGroup") => self.close_groups(arg(0).as_array().map_or(&[], Vec::as_slice), window, cx),
            ("MainThreadDialogs", "$showOpenDialog") => self.api_show_open_dialog(&arg(0), cx),
            ("MainThreadDialogs", "$showSaveDialog") => self.api_show_save_dialog(&arg(0), cx),
            ("MainThreadWindow", "$openUri") => self.api_open_uri(&request.args, window, cx),
            ("MainThreadUrls", "$registerUriHandler") => {
                // ExtensionIdentifier serializes as `{ value, _lower }`.
                let extension = arg(1)["value"].as_str().unwrap_or_default().to_owned();
                self.register_uri_handler(arg(0).as_u64().unwrap_or_default(), &extension, arg(2).as_str().unwrap_or_default());
                Outcome::done()
            }
            ("MainThreadUrls", "$unregisterUriHandler") => {
                self.unregister_uri_handler(arg(0).as_u64().unwrap_or_default());
                Outcome::done()
            }
            // shortcut: milestones 12 (url_handler.rs) and 17 (url_handlers.rs) both ported MainThreadUrls; the rest goes to 17's until one module remains.
            ("MainThreadUrls", method) => self.handle_urls_request(method, &request.args),
            ("MainThreadUriOpeners", "$registerUriOpener") => self.register_uri_opener(&request.args),
            ("MainThreadUriOpeners", "$unregisterUriOpener") => {
                self.urls.openers.retain(|opener| Some(opener.id.as_str()) != arg(0).as_str());
                Outcome::done()
            }
            ("MainThreadWorkspace", "$requestWorkspaceTrust") => self.request_workspace_trust(&arg(0), window, cx),
            ("MainThreadWorkspace", "$isResourceTrusted") => self.is_resource_trusted(&arg(0)),
            ("workbench", "previewWorkspaceEdit") => self.preview_workspace_edit(&arg(0), window, cx),
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
            // codelensController.ts: `codeLensProvider.onDidChange` asks for the lenses again.
            ("MainThreadLanguageFeatures", "$unregister") => {
                if let Some(method) = self.language_features.unregister(arg(0).as_u64().unwrap_or_default()) {
                    self.language_providers_changed(&method, cx);
                }
                Outcome::done()
            }
            ("MainThreadLanguageFeatures", "$emitFoldingRangeEvent") => {
                self.folding_range_event(&arg(0), cx);
                Outcome::done()
            }
            ("MainThreadLanguageFeatures", "$emitDocumentSemanticTokensEvent" | "$emitDocumentRangeSemanticTokensEvent") => {
                self.semantic_tokens_event(arg(0).as_u64().unwrap_or_default(), cx);
                Outcome::done()
            }
            ("MainThreadLanguageFeatures", "$emitInlayHintsEvent") => {
                self.inlay_hints_provider_changed(arg(0).as_u64().unwrap_or_default(), cx);
                Outcome::done()
            }
            ("MainThreadLanguageFeatures", "$emitCodeLensEvent") => {
                self.code_lens_provider_changed(arg(0).as_u64().unwrap_or_default(), cx);
                Outcome::done()
            }
            ("MainThreadLanguageFeatures", method) if method.starts_with("$register") => {
                self.language_features.register(method, request.args.clone());
                self.language_providers_changed(method, cx);
                Outcome::done()
            }
            ("MainThreadCommands", "$getCommands") => Outcome::Ready(Ok(json!(self.commands.ids().collect::<Vec<_>>()))),
            _ => Outcome::done(),
        }
    }
}

/// The host runs the command again once `onCommand:<id>` activated the extension that registers it.
fn retry_after_activation(activation: Activation, cx: &mut Context<Workbench>) -> Outcome {
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
