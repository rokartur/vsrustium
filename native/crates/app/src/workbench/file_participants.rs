//! The dialog mainThreadFileSystemEventService.ts shows before the edits of extensions' file operation
//! participants apply, its "Do not ask me again" memento and the `files.participants.resetChoice` action.

use futures::channel::oneshot;
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::dialog;

/// `MainThreadFileSystemEventService.MementoKeyAdditionalEdits`, in the profile storage.
const MEMENTO_KEY_ADDITIONAL_EDITS: &str = "file.particpants.additionalEdits";

impl Workbench {
    pub(super) fn register_file_participant_commands(&mut self) {
        self.commands.register("files.participants.resetChoice", |workbench, _, _, _| {
            workbench.profile_storage.lock().unwrap().delete(MEMENTO_KEY_ADDITIONAL_EDITS);
            Outcome::done()
        });
    }

    /// The host's `$confirmParticipantEdits(operation, extensionNames, needsConfirmation)`: answers `showPreview`,
    /// or null when the changes are skipped.
    pub fn confirm_participant_edits(&mut self, operation: u64, extension_names: &[Value], needs_confirmation: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let stored = self.profile_storage.lock().unwrap().get(MEMENTO_KEY_ADDITIONAL_EDITS).map(|value| value == "true");
        if let Some(show_preview) = stored {
            return Outcome::Ready(Ok(json!(show_preview)));
        }
        if self.skip_dialogs {
            return Outcome::Ready(Ok(json!(false)));
        }
        let message = participants_message(operation, extension_names);
        let (sender, receiver) = oneshot::channel();
        // An edit that needs confirmation always asks, with no checkbox, and always previews.
        if needs_confirmation {
            let answer = dialog::show_message_box(window, PromptLevel::Info, &message, None, &["Show Preview", "Skip Changes"], None, cx);
            cx.background_executor()
                .spawn(async move {
                    let show_preview = matches!(answer.await, Ok((0, _))).then_some(true);
                    let _ = sender.send(Ok(json!(show_preview)));
                })
                .detach();
            return Outcome::Pending(receiver);
        }
        let answer = dialog::show_message_box(window, PromptLevel::Info, &message, None, &["OK", "Show Preview", "Skip Changes"], Some("Do not ask me again"), cx);
        let storage = self.profile_storage.clone();
        cx.background_executor()
            .spawn(async move {
                let show_preview = match answer.await {
                    Ok((0, checked)) => Some((false, checked)),
                    Ok((1, checked)) => Some((true, checked)),
                    // Skip Changes is not remembered.
                    _ => None,
                };
                if let Some((show_preview, true)) = show_preview {
                    storage.lock().unwrap().set(MEMENTO_KEY_ADDITIONAL_EDITS, show_preview.to_string());
                }
                let _ = sender.send(Ok(json!(show_preview.map(|(show_preview, _)| show_preview))));
            })
            .detach();
        Outcome::Pending(receiver)
    }
}

/// The `ask.1.*` and `ask.N.*` messages; `operation` is a `FileOperation`.
fn participants_message(operation: u64, extension_names: &[Value]) -> String {
    let action = match operation {
        0 => "file creation",
        2 => "file move",
        3 => "file copy",
        _ => "file deletion",
    };
    match extension_names {
        [name] => format!("Extension '{}' wants to make refactoring changes with this {action}", name.as_str().unwrap_or_default()),
        names => format!("{} extensions want to make refactoring changes with this {action}", names.len()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use exthost::scan::scan_extension;
    use gpui::TestAppContext;

    use super::*;
    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;

    #[gpui::test]
    fn a_remembered_choice_answers_without_a_dialog_until_reset(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let host = HostSync::new(std::sync::mpsc::channel().0);
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), host, cx));
        let answer = |cx: &mut gpui::VisualTestContext| {
            workbench.update_in(cx, |workbench, window, cx| match workbench.confirm_participant_edits(2, &[json!("a")], false, window, cx) {
                Outcome::Ready(result) => Some(result.unwrap()),
                Outcome::Pending(_) => None,
            })
        };
        workbench.update(cx, |workbench, _| workbench.profile_storage.lock().unwrap().set(MEMENTO_KEY_ADDITIONAL_EDITS, "true".into()));
        assert_eq!(answer(cx), Some(json!(true)));
        workbench.update(cx, |workbench, _| workbench.profile_storage.lock().unwrap().set(MEMENTO_KEY_ADDITIONAL_EDITS, "false".into()));
        assert_eq!(answer(cx), Some(json!(false)));

        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command("files.participants.resetChoice", Vec::new(), window, cx));
        assert_eq!(workbench.update(cx, |workbench, _| workbench.profile_storage.lock().unwrap().get(MEMENTO_KEY_ADDITIONAL_EDITS).map(str::to_owned)), None);
        // With dialogs skipped (extension tests), the edits apply without a preview.
        workbench.update(cx, |workbench, _| workbench.skip_dialogs = true);
        assert_eq!(answer(cx), Some(json!(false)));
    }

    #[test]
    fn participants_message_names_one_extension_or_counts_them() {
        assert_eq!(participants_message(2, &[json!("Test Ext")]), "Extension 'Test Ext' wants to make refactoring changes with this file move");
        assert_eq!(participants_message(0, &[json!("a"), json!("b")]), "2 extensions want to make refactoring changes with this file creation");
        assert_eq!(participants_message(1, &[json!("a")]), "Extension 'a' wants to make refactoring changes with this file deletion");
        assert_eq!(participants_message(3, &[json!("a"), json!("b"), json!("c")]), "3 extensions want to make refactoring changes with this file copy");
    }
}
