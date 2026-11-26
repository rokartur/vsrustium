//! The save conflict editor of textFileSaveErrorHandler.ts: Compare in the "content of the file is newer" message
//! opens the file on disk (`conflictResolution` scheme, TextFileContentProvider) against the model, with the
//! help message and the editor title actions that keep or discard the model's changes.

use std::path::{Path, PathBuf};

use files::encoding;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::auto_save::SaveReason;
use super::host_requests::uri_parts;
use super::text_files::SaveOptions;
use crate::commands::Outcome;
use crate::editor::{TextDocument, uri_string, uri_value};
use crate::notifications::{Action, Notification, Run, Severity};

pub(super) const CONFLICT_RESOLUTION_SCHEME: &str = "conflictResolution";
/// `LEARN_MORE_DIRTY_WRITE_IGNORE_KEY`, the help message's `neverShowAgain` id.
const LEARN_MORE_IGNORE_KEY: &str = "learnMoreDirtyWriteError";
pub(super) const CONFLICT_EDITOR_HELP: &str = "Use the actions in the editor tool bar to either undo your changes or overwrite the content of the file with your changes.";
const LEARN_MORE_URL: &str = "https://go.microsoft.com/fwlink/?linkid=868264";

impl Workbench {
    pub(super) fn register_save_conflict_commands(&mut self) {
        self.commands.register("workbench.files.action.resolveConflict", |workbench, args, window, cx| {
            let Some(Ok((scheme, path))) = args.first().map(uri_parts) else { return Outcome::done() };
            let (scheme, path) = (scheme.to_string(), path);
            workbench.open_save_conflict_editor(&scheme, &path, window, cx)
        });
        self.commands.register("workbench.files.action.resolveConflictLearnMore", |_, _, _, cx| {
            cx.open_url(LEARN_MORE_URL);
            Outcome::done()
        });
        self.commands.register("workbench.files.action.acceptLocalChanges", |workbench, args, window, cx| workbench.accept_or_revert_local_changes(args, true, window, cx));
        self.commands.register("workbench.files.action.revertLocalChanges", |workbench, args, window, cx| workbench.accept_or_revert_local_changes(args, false, window, cx));
    }

    /// `ResolveSaveConflictAction.run`: the diff of the file on disk and the model, pinned, then the help message.
    fn open_save_conflict_editor(&mut self, scheme: &str, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        if self.editors.model(scheme, path).is_none() {
            return Outcome::done();
        }
        if let Err(error) = self.read_conflict_resolution_model(path) {
            return Outcome::Ready(Err(exthost::main_thread::serialized_error(&error)));
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{name} (in file) \u{2194} {name} (in {}) - Resolve save conflict", crate::product::name_long());
        let original = uri_value(CONFLICT_RESOLUTION_SCHEME, path);
        let opened = self.execute_command("_workbench.diff", vec![original, uri_value(scheme, path), json!(label), json!([null, { "pinned": true }])], window, cx);
        self.show_conflict_editor_help(&uri_string(scheme, path), false);
        opened
    }

    /// TextFileContentProvider `resolveEditorModel`: the conflict model holds the file's text in the model's
    /// encoding and language, and follows the file while it is open.
    pub(super) fn read_conflict_resolution_model(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|error| format!("Unable to read file '{}' ({error})", path.display()))?;
        let model = self.editors.model("file", path);
        let encoding = model.map_or_else(|| "utf8".to_string(), |model| model.document().encoding.clone());
        let language_id = model.map(|model| model.document().language_id.clone());
        let text = encoding::decode(&bytes, &encoding);
        if self.editors.model(CONFLICT_RESOLUTION_SCHEME, path).is_some() {
            self.update_model(CONFLICT_RESOLUTION_SCHEME, path, &text, true);
            return Ok(());
        }
        let document = TextDocument::from_text(CONFLICT_RESOLUTION_SCHEME, path, &text, encoding, language_id.as_deref(), &self.languages, &self.settings.lock().unwrap());
        self.editors.open_model(document);
        Ok(())
    }

    /// The conflict editor's help (`conflictEditorHelp`): Learn More, and Don't Show Again as `neverShowAgain`;
    /// `in_editor` is the message of a save from the conflict editor that failed again.
    pub(super) fn show_conflict_editor_help(&mut self, uri: &str, in_editor: bool) {
        if self.profile_storage.lock().unwrap().get(LEARN_MORE_IGNORE_KEY) == Some("true") {
            return;
        }
        let mut notification = Notification::new(if in_editor { Severity::Error } else { Severity::Info }, CONFLICT_EDITOR_HELP);
        notification.id = Some(uri.to_string());
        let learn_more = "workbench.files.action.resolveConflictLearnMore";
        notification.primary = vec![Action { id: learn_more.into(), label: "Learn More".into(), run: Run::Command(learn_more.into(), Vec::new()) }];
        let never = "workbench.notification.neverShowAgain";
        notification.secondary = vec![Action { id: never.into(), label: "Don't Show Again".into(), run: Run::Command(never.into(), vec![json!(LEARN_MORE_IGNORE_KEY)]) }];
        if let Some(key) = self.notify(notification) {
            self.text_files.conflict_messages.push(key);
        }
    }

    /// `acceptOrRevertLocalChangesCommand`: the help messages close, the model is saved over the file or reverted,
    /// then the file's own editor opens and the conflict editor closes.
    fn accept_or_revert_local_changes(&mut self, args: Vec<Value>, accept: bool, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some(Ok((scheme, path))) = args.first().map(uri_parts) else { return Outcome::done() };
        let (scheme, path) = (scheme.to_string(), path);
        let Some(conflict_editor) = self.editors.active_tab().map(|editor| editor.model_id) else { return Outcome::done() };
        for key in std::mem::take(&mut self.text_files.conflict_messages) {
            self.close_notification(key, window, cx);
        }
        if !accept {
            if let Err(error) = self.revert_text_file(&scheme, &path, cx) {
                self.notify_error(error);
            }
            self.reopen_after_conflict(path, conflict_editor, window, cx);
            return Outcome::done();
        }
        let saved = self.save_model_with(scheme, path.clone(), SaveReason::Explicit, SaveOptions { force: true, ignore_modified_since: true, ..SaveOptions::default() }, cx);
        cx.spawn_in(window, async move |workbench, cx| {
            let _ = saved.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| workbench.reopen_after_conflict(path, conflict_editor, window, cx));
        })
        .detach();
        Outcome::done()
    }

    /// `activeConflictResolutionResource`: the file of the active conflict editor, which is also the
    /// `saveConflictResolutionContext` context key.
    pub(super) fn active_conflict_resolution_path(&self) -> Option<&Path> {
        self.editors.active_diff().filter(|diff| diff.original.scheme == CONFLICT_RESOLUTION_SCHEME).map(|diff| diff.modified.path.as_path())
    }

    fn reopen_after_conflict(&mut self, path: PathBuf, conflict_editor: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, true, window, cx);
        if let Some(index) = self.editors.editors.iter().position(|editor| editor.model_id == conflict_editor) {
            drop(self.close_editors(vec![index], window, cx));
        }
    }
}
