//! The main-thread side of output channels and loggers: workbench/api/browser/mainThreadOutputService.ts and
//! mainThreadLogService.ts.

use std::path::PathBuf;

use exthost::session::ForwardedRequest;
use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::output::logger_from_json;
use crate::workbench::Workbench;

/// output.ts `OutputChannelUpdateMode`.
const APPEND: u64 = 1;

impl Workbench {
    pub fn handle_output_request(&mut self, request: ForwardedRequest, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let arg = |index: usize| request.args.get(index).cloned().unwrap_or(Value::Null);
        let text = |index: usize| arg(index).as_str().unwrap_or_default().to_owned();
        let path = |value: Value| PathBuf::from(value["path"].as_str().unwrap_or_default());
        match (request.proxy, request.method.as_str()) {
            ("MainThreadOutputService", "$register") => {
                let id = self.output.register_extension_channel(&text(0), path(arg(1)), arg(2).as_str().map(str::to_owned), &text(3));
                self.output_channels_changed(window, cx);
                Outcome::Ready(Ok(json!(id)))
            }
            ("MainThreadOutputService", "$update") => {
                let mode = arg(1).as_u64().unwrap_or(APPEND);
                // `$update` ignores a clear or replace without `till`.
                if mode == APPEND || arg(2).is_u64() {
                    self.update_output_channel(&text(0), mode, arg(2).as_u64(), cx);
                }
                Outcome::done()
            }
            ("MainThreadOutputService", "$reveal") => {
                self.reveal_output_channel(&text(0), arg(1) == true, window, cx);
                Outcome::done()
            }
            ("MainThreadOutputService", "$close") => {
                if self.output_view_visible() && self.output.active.as_deref() == Some(text(0).as_str()) {
                    self.close_output_view(cx);
                }
                Outcome::done()
            }
            ("MainThreadOutputService", "$dispose") => {
                self.output.remove_channel(&text(0));
                self.output_channels_changed(window, cx);
                Outcome::done()
            }
            ("MainThreadLogger", "$registerLogger") => {
                self.output.register_logger(logger_from_json(&arg(0)));
                self.output_channels_changed(window, cx);
                Outcome::done()
            }
            ("MainThreadLogger", "$deregisterLogger") => {
                self.output.deregister_logger(&path(arg(0)));
                self.output_channels_changed(window, cx);
                Outcome::done()
            }
            ("MainThreadLogger", "$setVisibility") => {
                self.output.set_logger_visibility(&path(arg(0)), arg(1) == true);
                self.output_channels_changed(window, cx);
                Outcome::done()
            }
            // The host writes `file` loggers itself (node extHostLoggerService.ts); the others it creates here.
            ("MainThreadLogger", "$createLogger") => {
                let uri = arg(0);
                let uri_text = crate::editor::uri_string(uri["scheme"].as_str().unwrap_or("file"), &path(uri.clone()));
                self.output.create_logger(path(uri), &uri_text, &arg(1));
                self.output_channels_changed(window, cx);
                Outcome::done()
            }
            ("MainThreadLogger", "$log") => {
                let messages: Vec<(u8, String)> = arg(1).as_array().into_iter().flatten().map(|entry| (entry[0].as_u64().unwrap_or(3) as u8, entry[1].as_str().unwrap_or_default().to_owned())).collect();
                match self.output.log(&path(arg(0)), &messages) {
                    Ok(()) => Outcome::done(),
                    Err(message) => Outcome::error(&message),
                }
            }
            ("MainThreadLogger", "$flush") => match self.output.flush_logger(&path(arg(0))) {
                Ok(()) => Outcome::done(),
                Err(message) => Outcome::error(&message),
            },
            _ => Outcome::done(),
        }
    }
}
