//! largeFileOptimizations.ts: a model too large for tokenization (`TextModel.isTooLargeForTokenization`) gets
//! a prompt when an editor shows it, with Forcefully Enable Features and Don't Show Again.

use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::notifications::{Action, Notification, Run, Severity};

/// `LargeFileOptimizationsWarner.ID`, the `neverShowAgain` id.
const WARNER_ID: &str = "editor.contrib.largeFileOptimizationsWarner";
const FORCE_ENABLE_COMMAND: &str = "_largeFileOptimizations.forceEnable";
const NEVER_SHOW_AGAIN_COMMAND: &str = "workbench.notification.neverShowAgain";

impl Workbench {
    pub(super) fn register_large_file_commands(&mut self) {
        self.commands.register(FORCE_ENABLE_COMMAND, |workbench, _, _, cx| {
            workbench.update_setting("editor.largeFileOptimizations", json!(false));
            workbench.notify(Notification::new(Severity::Info, "Please reopen file in order for this setting to take effect."));
            cx.notify();
            Outcome::done()
        });
        // notificationService.ts `neverShowAgain` with its default APPLICATION scope, the port's profile storage.
        self.commands.register(NEVER_SHOW_AGAIN_COMMAND, |workbench, args, _, _| {
            if let Some(id) = args.first().and_then(Value::as_str) {
                workbench.profile_storage.lock().unwrap().set(id, "true".into());
            }
            Outcome::done()
        });
    }

    /// `_update` on `onDidChangeModel`: the editor now shows `model_id`.
    pub(super) fn warn_large_file(&mut self) {
        let Some(editor) = self.editors.active() else { return };
        if !editor.document.too_large_for_tokenization || self.profile_storage.lock().unwrap().get(WARNER_ID) == Some("true") {
            return;
        }
        let name = editor.document.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let message = format!("{name}: tokenization, wrapping, folding, codelens, word highlighting and sticky scroll have been turned off for this large file in order to reduce memory usage and avoid freezing or crashing.");
        let mut notification = Notification::new(Severity::Info, message);
        notification.primary = vec![
            Action { id: NEVER_SHOW_AGAIN_COMMAND.into(), label: "Don't Show Again".into(), run: Run::Command(NEVER_SHOW_AGAIN_COMMAND.into(), vec![json!(WARNER_ID)]) },
            Action { id: FORCE_ENABLE_COMMAND.into(), label: "Forcefully Enable Features".into(), run: Run::Command(FORCE_ENABLE_COMMAND.into(), Vec::new()) },
        ];
        self.notify(notification);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use exthost::scan::scan_extension;
    use gpui::TestAppContext;

    use crate::host_sync::HostSync;
    use crate::languages::Languages;
    use crate::theme::load_theme;
    use crate::workbench::Workbench;

    #[gpui::test]
    fn a_large_file_turns_off_folding_and_highlights_and_prompts_once(cx: &mut TestAppContext) {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-large-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let large = dir.join("large.txt");
        std::fs::write(&large, "a\n  b\n".repeat(150_001)).unwrap();
        let small = dir.join("small.txt");
        std::fs::write(&small, "a\n  b\n").unwrap();
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, Languages::new(&[]), crate::default_settings(), HostSync::new(std::sync::mpsc::channel().0), cx));
        let open = |path: &Path, cx: &mut gpui::VisualTestContext| workbench.update_in(cx, |workbench, window, cx| workbench.open_file(path.to_path_buf(), false, window, cx));
        let prompts = |cx: &mut gpui::VisualTestContext| workbench.read_with(cx, |workbench, _| workbench.notifications.model.items.iter().filter(|item| item.message.raw.starts_with("large.txt: tokenization")).count());

        open(&small, cx);
        assert_eq!(prompts(cx), 0);
        open(&large, cx);
        workbench.read_with(cx, |workbench, _| {
            let editor = workbench.editors.active().unwrap();
            assert!(editor.document.too_large_for_tokenization);
            assert_eq!(editor.folding.regions.len(), 0);
            assert!(editor.highlights.is_empty());
        });
        assert_eq!(prompts(cx), 1);
        workbench.update_in(cx, |workbench, window, cx| workbench.execute_command(super::NEVER_SHOW_AGAIN_COMMAND, vec![serde_json::json!(super::WARNER_ID)], window, cx));
        open(&small, cx);
        open(&large, cx);
        assert_eq!(prompts(cx), 1, "Don't Show Again");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
