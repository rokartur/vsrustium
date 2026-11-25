//! The main-thread side of `window.show*Message`: workbench/api/browser/mainThreadMessageService.ts.

use futures::channel::oneshot;
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use crate::commands::Outcome;
use crate::dialog;
use crate::notifications::{Action, Notification, Run, Severity};
use crate::workbench::Workbench;

/// src/vs/platform/extensions/common/extensions.ts `MANAGE_EXTENSION_COMMAND_ID`.
const MANAGE_EXTENSION_COMMAND_ID: &str = "_extensions.manage";

impl Workbench {
    /// `$showMessage(severity, message, options, commands)`: answers the handle of the picked command, or null.
    pub(crate) fn show_extension_message(&mut self, args: &[Value], window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let severity = Severity::from_number(&args[0]);
        let message = args[1].as_str().unwrap_or_default();
        let options = &args[2];
        let commands = args[3].as_array().cloned().unwrap_or_default();
        if options["modal"] == true {
            return show_modal_message(severity, message, options["detail"].as_str(), &commands, window, cx);
        }
        let mut notification = Notification::new(severity, message);
        notification.legacy_links = true;
        notification.primary = commands
            .iter()
            .map(|command| Action { id: format!("_extension_message_handle_{}", command["handle"]), label: command["title"].as_str().unwrap_or_default().to_owned(), run: Run::Reply(command["handle"].as_u64().unwrap_or_default()) })
            .collect();
        match options["source"]["identifier"]["value"].as_str() {
            Some(id) => {
                notification.source = Some(options["source"]["label"].as_str().unwrap_or(id).to_owned());
                notification.source_id = Some(id.to_owned());
                notification.secondary = vec![Action { id: id.to_owned(), label: "Manage Extension".into(), run: Run::Command(MANAGE_EXTENSION_COMMAND_ID.into(), vec![json!(id)]) }];
            }
            None => notification.source = Some("Extension".into()),
        }
        let Some(key) = self.notify(notification) else { return Outcome::done() };
        let (sender, receiver) = oneshot::channel();
        self.notifications.replies.insert(key, sender);
        cx.notify();
        Outcome::Pending(receiver)
    }
}

/// `_showModalMessage`: the close affordance, else Cancel or OK, is the dialog's cancel button, placed last.
fn show_modal_message(severity: Severity, message: &str, detail: Option<&str>, commands: &[Value], window: &mut Window, cx: &mut Context<Workbench>) -> Outcome {
    let mut buttons: Vec<(&str, Value)> = Vec::new();
    let mut cancel = None;
    for command in commands {
        let button = (command["title"].as_str().unwrap_or_default(), command["handle"].clone());
        match command["isCloseAffordance"] == true {
            true => cancel = Some(button),
            false => buttons.push(button),
        }
    }
    let cancel_label = if buttons.is_empty() { "OK" } else { "Cancel" };
    buttons.push(cancel.unwrap_or((cancel_label, Value::Null)));
    let level = match severity {
        Severity::Info => PromptLevel::Info,
        Severity::Warning => PromptLevel::Warning,
        Severity::Error => PromptLevel::Critical,
    };
    let labels: Vec<&str> = buttons.iter().map(|(label, _)| *label).collect();
    let picked = dialog::show_message_box(window, level, message, detail, &labels, None, cx);
    let handles: Vec<Value> = buttons.into_iter().map(|(_, handle)| handle).collect();
    let (sender, receiver) = oneshot::channel();
    cx.background_executor()
        .spawn(async move {
            let handle = picked.await.map(|(index, _)| handles[index].clone()).unwrap_or(Value::Null);
            let _ = sender.send(Ok(handle));
        })
        .detach();
    Outcome::Pending(receiver)
}
