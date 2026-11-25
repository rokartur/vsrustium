//! Installing and uninstalling extensions from the workbench (extensions.contribution.ts): the commands, the
//! notification after a VSIX install, and the contributions the workbench reads again once the host runs the change.

use std::path::PathBuf;

use exthost::extension_management::Job;
use exthost::scan::ScannedExtension;
use futures::channel::oneshot;
use gpui::{Context, PathPromptOptions};
use serde_json::Value;

use super::Workbench;
use super::host_requests::uri_parts;
use crate::commands::Outcome;
use exthost::main_thread::serialized_error;
use crate::notifications::Severity;

/// `SELECT_INSTALL_VSIX_EXTENSION_COMMAND_ID`.
const SELECT_INSTALL_VSIX: &str = "workbench.extensions.action.installVSIX";
/// `INSTALL_EXTENSION_FROM_VSIX_COMMAND_ID`.
const INSTALL_FROM_VSIX: &str = "workbench.extensions.command.installFromVSIX";

impl Workbench {
    pub(super) fn register_extension_management_commands(&mut self) {
        self.commands.register(SELECT_INSTALL_VSIX, |_, _, window, cx| {
            // shortcut: gpui's open panel takes no file type filter, so the 'VSIX Extensions' filter is not applied.
            let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Install".into()) });
            let (sender, receiver) = oneshot::channel();
            cx.spawn_in(window, async move |workbench, cx| {
                let Ok(Ok(Some(paths))) = picked.await else { return drop(sender.send(Ok(Value::Null))) };
                let uris = paths.iter().map(|path| exthost::scan::file_uri(path)).collect();
                let outcome = workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(INSTALL_FROM_VSIX, vec![Value::Array(uris)], window, cx));
                let result = match outcome {
                    Ok(Outcome::Ready(result)) => result,
                    Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(Value::Null)),
                    Err(_) => Ok(Value::Null),
                };
                let _ = sender.send(result);
            })
            .detach();
            Outcome::Pending(receiver)
        });
        self.commands.register(INSTALL_FROM_VSIX, |workbench, args, _, cx| {
            let resources = match args.into_iter().next() {
                Some(Value::Array(resources)) => resources,
                Some(resource) => vec![resource],
                None => Vec::new(),
            };
            let mut vsixs = Vec::new();
            for resource in &resources {
                match uri_parts(resource) {
                    Ok((_, path)) => vsixs.push(path),
                    Err(error) => return Outcome::Ready(Err(error)),
                }
            }
            workbench.install_vsixs(vsixs, true, cx)
        });
        self.commands.register("workbench.extensions.installExtension", |workbench, args, _, cx| match args.first() {
            Some(Value::String(id)) => {
                // shortcut: installing by id needs the Open VSX gallery (milestone 18 item 2); until then it fails as an unknown id does.
                let message = format!("Extension '{id}' not found.");
                workbench.notify_error(message.clone());
                cx.notify();
                Outcome::Ready(Err(serialized_error(&message)))
            }
            Some(uri) => match uri_parts(uri) {
                Ok((_, path)) => workbench.install_vsixs(vec![path], false, cx),
                Err(error) => Outcome::Ready(Err(error)),
            },
            None => Outcome::error("Extension id required."),
        });
        self.commands.register("workbench.extensions.uninstallExtension", |workbench, args, _, cx| {
            let Some(id) = args.first().and_then(Value::as_str).filter(|id| !id.is_empty()) else { return Outcome::error("Extension id required.") };
            if let Some(extension) = workbench.extensions.iter().find(|extension| extension.id.eq_ignore_ascii_case(id))
                && extension.description["isBuiltin"] == true
            {
                return Outcome::error(format!("Extension '{id}' is a Built-in extension and cannot be uninstalled"));
            }
            let id = id.to_owned();
            workbench.run_extension_jobs(vec![Job::Uninstall(id.clone())], None, cx, move |error| {
                error.replace(&format!("Extension '{id}' is not installed."), &format!("Extension '{id}' is not installed. Make sure you use the full extension ID, including the publisher, e.g.: ms-dotnettools.csharp."))
            })
        });
    }

    /// `extensionsWorkbenchService.install(vsix)` for each file; `notify` shows the result as INSTALL_FROM_VSIX does.
    fn install_vsixs(&mut self, vsixs: Vec<PathBuf>, notify: bool, cx: &mut Context<Self>) -> Outcome {
        let done = match (notify, vsixs.len()) {
            (false, _) => None,
            (true, 1) => Some("Completed installing extension."),
            (true, _) => Some("Completed installing extensions."),
        };
        self.run_extension_jobs(vsixs.into_iter().map(Job::InstallVsix).collect(), done, cx, |error| error)
    }

    /// Runs the jobs one after another on the host thread, then reads the contributions of the running extensions.
    /// The first failure is shown and rejects the command, as `onUnexpectedError` and the rethrow upstream do.
    fn run_extension_jobs(&mut self, jobs: Vec<Job>, done: Option<&'static str>, cx: &mut Context<Self>, error_text: impl Fn(String) -> String + 'static) -> Outcome {
        let replies: Vec<_> = jobs.into_iter().map(|job| self.host.run_extensions(job)).collect();
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let mut result = Ok(Value::Null);
            for reply in replies {
                let reply = reply.await.unwrap_or_else(|_| Err("The extension host is not running.".into()));
                let updated = workbench.update(cx, |workbench, cx| {
                    match reply {
                        Ok(running) => workbench.set_running_extensions(running, cx),
                        Err(error) if result.is_ok() => {
                            let error = error_text(error);
                            workbench.notify_error(error.clone());
                            result = Err(serialized_error(&error));
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
            if let (Ok(_), Some(done)) = (&result, done) {
                let _ = workbench.update(cx, |workbench, cx| {
                    drop(workbench.prompt_notification(Severity::Info, done, &[]));
                    cx.notify();
                });
            }
            let _ = sender.send(result);
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// The extension points the workbench reads, after `_doHandleExtensionPoints` with the running extensions.
    fn set_running_extensions(&mut self, running: Vec<ScannedExtension>, cx: &mut Context<Self>) {
        self.languages = crate::languages::Languages::new(&running);
        self.menus = crate::menus::workbench_menus();
        self.menus.extend(crate::menus::extension_menus(&running));
        self.command_icons = crate::menus::command_icons(&running);
        self.set_default_keybindings(crate::keybindings::default_keybindings(&running));
        if let Some(file) = self.keybindings.user_file.clone() {
            self.set_user_keybindings(crate::keybindings::user_keybindings(&file));
        }
        self.set_command_palette_menu(crate::command_palette::menu(&running));
        match crate::theme::color_themes(&running) {
            Ok(themes) => self.color_themes = themes,
            Err(error) => eprintln!("[extensions] reading color themes: {error:#}"),
        }
        self.extensions = running;
        cx.notify();
    }
}
