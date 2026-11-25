//! The workbench's side of mainThreadConsole.ts, mainThreadErrors.ts and the activation errors of
//! mainThreadExtensionService.ts. The session prints each call to the port log (stderr, the window's console) and
//! answers it; the workbench gets a copy (`OBSERVED_REQ`) for the Window log and notifications.

use std::time::Instant;

use exthost::session::ForwardedRequest;
use serde_json::Value;

use crate::commands::Outcome;
use crate::notifications::{Notification, Severity};
use crate::output::logger::{console_log_line, to_error_message};
use crate::workbench::Workbench;

impl Workbench {
    pub fn handle_observed_request(&mut self, request: &ForwardedRequest) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        match (request.proxy, request.method.as_str()) {
            // `MainThreadConsole.$logExtensionHostMessage`: an extension test run from the command line logs every
            // entry (`isExtensionDevTestFromCli`), any other run its errors only.
            ("MainThreadConsole", "$logExtensionHostMessage") => {
                if let Some((level, text)) = console_log_line(&arg(0), self.skip_dialogs) {
                    self.output.log_window(level, &text);
                }
            }
            ("MainThreadErrors", "$onUnexpectedError") => {
                if let Some(text) = self.unexpected_errors.handle(&arg(0), Instant::now()) {
                    self.output.log_window(5, &text);
                }
            }
            ("MainThreadExtensionService", "$onExtensionActivationError") => self.extension_activation_failed(&arg(0), &arg(1), &arg(2)),
            _ => {}
        }
        Outcome::done()
    }

    /// `$onExtensionActivationError`: a missing dependency is a notification; any other error one while an extension
    /// is developed (`isDev`, the port being a built product), else only the console line the session printed.
    fn extension_activation_failed(&mut self, extension_id: &Value, error: &Value, missing_dependency: &Value) {
        let id = extension_id["value"].as_str().unwrap_or_default();
        let message = match missing_dependency["dependency"].as_str() {
            // `_handleMissingNotInstalledDependency` without a gallery: `getExtensions` finds nothing.
            // shortcut: no Install and Reload offer until the Open VSX gallery (milestone 18) can find the dependency.
            Some(dependency) => {
                let Some(extension) = self.extensions.iter().find(|extension| extension.id.eq_ignore_ascii_case(id)) else { return };
                let description = &extension.description;
                let name = description["displayName"].as_str().or(description["name"].as_str()).unwrap_or_default();
                format!("Cannot activate the '{name}' extension because it depends on an unknown '{dependency}' extension.")
            }
            None if self.extension_development => to_error_message(error, false),
            None => return,
        };
        self.notify(Notification::new(Severity::Error, message));
    }
}

#[cfg(test)]
mod tests;
