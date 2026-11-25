//! Installing and uninstalling extensions from the workbench (extensions.contribution.ts): the commands, the
//! notification after a VSIX install, and the contributions the workbench reads again once the host runs the change.

use std::path::PathBuf;

use config::configuration::Overrides;
use exthost::extension_management::Job;
use exthost::gallery::{Gallery, Operation};
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
            Some(Value::String(arg)) => {
                let (id, version) = id_and_version(arg);
                let pre_release = args.get(1).is_some_and(|options| options["installPreReleaseVersion"] == true);
                workbench.install_from_gallery(id, version, pre_release, cx)
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

    /// `extensionsWorkbenchService.install(id)`: the gallery lookup and the download run off the UI thread, then the
    /// host installs the VSIX and starts the extension. A built-in cannot be installed from the gallery.
    fn install_from_gallery(&mut self, id: String, version: Option<String>, pre_release: bool, cx: &mut Context<Self>) -> Outcome {
        let installed = self.extensions.iter().find(|extension| extension.id.eq_ignore_ascii_case(&id));
        let builtin = installed.is_some_and(|extension| extension.description["isBuiltin"] == true);
        let operation = if installed.is_some() { Operation::Update } else { Operation::Install };
        let settings = self.settings.lock().unwrap();
        let downloads = settings.user_home().parent().expect("the user home is <user data>/User").join("CachedExtensionVSIXs");
        // shortcut: `extensions.verifySignature` is not registered yet, so only a user value reaches here; registered with the extensions settings.
        let verify_signature = settings.get_value(Some("extensions.verifySignature"), &Overrides::default()) != Some(Value::Bool(false));
        drop(settings);
        let download = cx.background_executor().spawn(async move {
            if builtin {
                anyhow::bail!("The extension '{id}' cannot be installed because it was not found.");
            }
            let gallery = Gallery::open_vsx(crate::VERSION);
            let extension = gallery.installable_extension(&id, version.as_deref(), pre_release)?;
            let vsix = gallery.download(&extension, operation, &downloads, verify_signature)?;
            Ok((vsix, extension))
        });
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |workbench, cx| {
            let installed = match download.await {
                Ok((vsix, extension)) => workbench.update(cx, |workbench, cx| workbench.run_extension_jobs(vec![Job::InstallGallery(vsix, Box::new(extension))], None, cx, |error| error)),
                Err(error) => workbench.update(cx, |workbench, cx| {
                    let error = format!("{error:#}");
                    workbench.notify_error(error.clone());
                    cx.notify();
                    Outcome::Ready(Err(serialized_error(&error)))
                }),
            };
            let result = match installed {
                Ok(Outcome::Ready(result)) => result,
                Ok(Outcome::Pending(pending)) => pending.await.unwrap_or(Ok(Value::Null)),
                Err(_) => Ok(Value::Null),
            };
            let _ = sender.send(result);
        })
        .detach();
        Outcome::Pending(receiver)
    }

    /// `MainThreadDownloadService.$download`, downloadService.ts: a file is copied, anything else is fetched with a GET.
    pub(crate) fn download_resource(&mut self, resource: &Value, target: &Value, cx: &mut Context<Self>) -> Outcome {
        let target = match uri_parts(target) {
            Ok((_, path)) => path,
            Err(error) => return Outcome::Ready(Err(error)),
        };
        let part = |key: &str| resource[key].as_str().unwrap_or_default().to_owned();
        let scheme = part("scheme");
        let mut url = format!("{scheme}://{}{}", part("authority"), part("path"));
        if !part("query").is_empty() {
            url += &format!("?{}", part("query"));
        }
        if !part("fragment").is_empty() {
            url += &format!("#{}", part("fragment"));
        }
        let source = PathBuf::from(part("path"));
        let download = cx.background_executor().spawn(async move {
            if scheme != "file" {
                return exthost::request::download(&url, &target).map_err(|error| format!("{error:#}"));
            }
            if target.exists() {
                return Err(format!("Unable to move/copy '{}' because target '{}' already exists at destination.", source.display(), target.display()));
            }
            std::fs::copy(&source, &target).map(drop).map_err(|error| error.to_string())
        });
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |_, _| {
            let _ = sender.send(download.await.map(|()| Value::Null).map_err(|error| serialized_error(&error)));
        })
        .detach();
        Outcome::Pending(receiver)
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

/// `getIdAndVersion`: `publisher.name@1.2.3` or `publisher.name@prerelease`, the id lowercased.
fn id_and_version(arg: &str) -> (String, Option<String>) {
    let split = arg.rsplit_once('@').filter(|(id, version)| {
        let semver = || {
            let core = version.split_once('-').map_or(*version, |(core, _)| core);
            let parts: Vec<&str> = core.split('.').collect();
            parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        };
        id.split_once('.').is_some_and(|(publisher, name)| !publisher.is_empty() && !publisher.contains('@') && !name.is_empty()) && (*version == "prerelease" || semver())
    });
    match split {
        Some((id, version)) => (id.to_lowercase(), Some(version.to_owned())),
        None => (arg.to_lowercase(), None),
    }
}

#[cfg(test)]
#[test]
fn id_and_version_splits_a_version_suffix() {
    assert_eq!(id_and_version("RedHat.vscode-yaml"), ("redhat.vscode-yaml".into(), None));
    assert_eq!(id_and_version("redhat.vscode-yaml@2.0.0"), ("redhat.vscode-yaml".into(), Some("2.0.0".into())));
    assert_eq!(id_and_version("redhat.vscode-yaml@2.0.0-next.1"), ("redhat.vscode-yaml".into(), Some("2.0.0-next.1".into())));
    assert_eq!(id_and_version("redhat.vscode-yaml@prerelease"), ("redhat.vscode-yaml".into(), Some("prerelease".into())));
    assert_eq!(id_and_version("redhat.vscode-yaml@latest"), ("redhat.vscode-yaml@latest".into(), None));
}
