//! textFileService.ts with the text file and untitled model behavior the workbench runs: New Untitled Text File,
//! save with its conflict check and error message (textFileSaveErrorHandler.ts), Save As (`saveAs`,
//! `suggestSavePath`), Save without Formatting, revert, and models following their file on disk
//! (textFileEditorModelManager.ts and textFileEditorModel.ts `onDidFilesChange`, editorService.ts watchers).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use config::configuration::Overrides;
use exthost::main_thread_files::resource;
use files::encoding;
use files::watcher::{Change, ChangeType, Watcher};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Workbench;
use super::actions::when_saved;
use super::auto_save::SaveReason;
use super::host_requests::uri_parts;
use super::search_editor_view::SEARCH_EDITOR_SCHEME;
use crate::commands::Outcome;
use crate::editor::model_service::compute_edits;
use crate::editor::view_state::{self, ViewStates};
use crate::editor::{EditReason, OpenEditor, TextDocument, uri_string};
use crate::host_sync::FileJob;
use crate::languages::PLAINTEXT;
use crate::notifications::{Action, Notification, Run, Severity};

#[cfg(test)]
mod tests;

/// `ITextFileSaveOptions`: `force` writes a clean model too; `unlock` makes a write locked file writable first;
/// `elevated` writes as the superuser.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SaveOptions {
    pub force: bool,
    pub skip_participants: bool,
    pub ignore_modified_since: bool,
    pub unlock: bool,
    pub elevated: bool,
}

/// Why a write failed: the `FileOperationResult` the save error message depends on, or another error's message.
#[derive(Debug)]
pub(super) enum SaveError {
    ModifiedSince,
    /// `FILE_WRITE_LOCKED`, after a write with `unlock` when it is true.
    WriteLocked { tried_unlock: bool },
    PermissionDenied,
    Other(String),
}

/// `IFileStat` of a model's file as last read or written: its modification time and size (the etag's parts).
type Stat = (SystemTime, u64);

#[derive(Default)]
pub(super) struct TextFiles {
    stats: HashMap<PathBuf, Stat>,
    /// textFileEditorModel.ts `preferredLanguageId`: models whose language was picked, which `files.associations`
    /// leaves alone.
    pub preferred_languages: HashSet<(String, PathBuf)>,
    /// `TextFileEditorModelState.ORPHAN`: models whose file is gone.
    pub orphaned: HashSet<PathBuf>,
    /// textFileSaveErrorHandler.ts `messages`: the save error notification of each model by URI.
    save_errors: HashMap<String, u64>,
    watcher: Option<Watcher>,
    /// editorService.ts `activeOutOfWorkspaceWatchers`, by watch id.
    watched_files: HashMap<PathBuf, u64>,
    next_watch: u64,
    /// The model the group's code editor showed at the last editors change (`onDidChangeModel`).
    shown_model: Option<u32>,
}

impl Workbench {
    pub(super) fn register_text_file_commands(&mut self) {
        // fileCommands.ts NEW_UNTITLED_FILE_COMMAND_ID.
        self.commands.register("workbench.action.files.newUntitledFile", |workbench, args, window, cx| {
            let language = args.first().and_then(|args| args["languageId"].as_str()).map(str::to_string);
            workbench.open_untitled(language, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.action.files.saveAs", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let (scheme, path) = (editor.document.scheme.clone(), editor.document.path.clone());
            let saved = workbench.save_as(scheme, path, SaveOptions { force: true, ..SaveOptions::default() }, cx);
            when_saved(vec![saved], |_| Value::Null, cx)
        });
        self.commands.register("workbench.action.files.saveWithoutFormatting", |workbench, _, _, cx| {
            let Some(editor) = workbench.editors.active() else { return Outcome::done() };
            let (scheme, path) = (editor.document.scheme.clone(), editor.document.path.clone());
            let saved = workbench.save_model_with(scheme, path, SaveReason::Explicit, SaveOptions { force: true, skip_participants: true, ..SaveOptions::default() }, cx);
            when_saved(vec![saved], |_| Value::Null, cx)
        });
        // The actions of the save error message, on the model of the URI they carry.
        self.commands.register("workbench.files.action.saveModel", |workbench, args, _, cx| {
            workbench.run_on_model(&args, cx, |workbench, scheme, path, cx| workbench.save_model_with(scheme, path, SaveReason::Explicit, SaveOptions { force: true, ..SaveOptions::default() }, cx))
        });
        self.commands.register("workbench.files.action.saveIgnoreModifiedSince", |workbench, args, _, cx| {
            let options = SaveOptions { force: true, ignore_modified_since: true, ..SaveOptions::default() };
            workbench.run_on_model(&args, cx, move |workbench, scheme, path, cx| workbench.save_model_with(scheme, path, SaveReason::Explicit, options, cx))
        });
        self.commands.register("workbench.files.action.unlock", |workbench, args, _, cx| {
            let options = SaveOptions { force: true, unlock: true, ..SaveOptions::default() };
            workbench.run_on_model(&args, cx, move |workbench, scheme, path, cx| workbench.save_model_with(scheme, path, SaveReason::Explicit, options, cx))
        });
        self.commands.register("workbench.files.action.saveModelElevated", |workbench, args, _, cx| {
            let options = SaveOptions { force: true, elevated: true, unlock: args.get(1) == Some(&json!(true)), ..SaveOptions::default() };
            workbench.run_on_model(&args, cx, move |workbench, scheme, path, cx| workbench.save_model_with(scheme, path, SaveReason::Explicit, options, cx))
        });
        self.commands.register("workbench.files.action.saveModelAs", |workbench, args, _, cx| {
            workbench.run_on_model(&args, cx, |workbench, scheme, path, cx| workbench.save_as(scheme, path, SaveOptions { force: true, ..SaveOptions::default() }, cx))
        });
        self.commands.register("workbench.files.action.revertModel", |workbench, args, _, cx| {
            let Some((scheme, path)) = args.first().and_then(|uri| uri_parts(uri).ok()).map(|(scheme, path)| (scheme.to_string(), path)) else { return Outcome::done() };
            if let Err(error) = workbench.revert_text_file(&scheme, &path, cx) {
                workbench.notify_error(error);
            }
            Outcome::done()
        });
    }

    fn run_on_model(&mut self, args: &[Value], cx: &mut Context<Self>, run: impl FnOnce(&mut Self, String, PathBuf, &mut Context<Self>) -> oneshot::Receiver<bool>) -> Outcome {
        let Some((scheme, path)) = args.first().and_then(|uri| uri_parts(uri).ok()).map(|(scheme, path)| (scheme.to_string(), path)) else { return Outcome::done() };
        if self.editors.model(&scheme, &path).is_none() {
            return Outcome::done();
        }
        let saved = run(self, scheme, path, cx);
        when_saved(vec![saved], |_| Value::Null, cx)
    }

    /// `editorService.openEditor({ resource: undefined, options: { pinned: true }, languageId })`: a new untitled model
    /// in `languageId`, else `files.defaultLanguage` (`${activeEditorLanguage}` the active editor's language).
    pub(super) fn open_untitled(&mut self, language: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let path = (1..).map(|n| PathBuf::from(format!("Untitled-{n}"))).find(|path| self.editors.model("untitled", path).is_none()).expect("a free name");
        let configured = || self.settings.lock().unwrap().get_value(Some("files.defaultLanguage"), &Overrides::default()).and_then(|value| value.as_str().map(str::to_string));
        let language = language.or_else(configured).filter(|language| !language.is_empty());
        let language = match language.as_deref() {
            Some("${activeEditorLanguage}") => self.editors.active().map(|editor| editor.document.language_id.clone()),
            _ => language,
        };
        let document = TextDocument::untitled(&path, "", language.as_deref().filter(|id| self.languages.is_registered(id)), &self.languages, &self.settings.lock().unwrap());
        self.editors.open(document, false);
        window.focus(&self.editor_focus, cx);
        self.editors_changed(cx);
    }

    /// A file's model from disk, its stat kept for the save conflict check.
    pub(super) fn read_document(&mut self, path: &Path) -> std::io::Result<TextDocument> {
        let bytes = std::fs::read(path)?;
        if let Some(stat) = stat(path) {
            self.text_files.stats.insert(path.to_path_buf(), stat);
        }
        Ok(TextDocument::from_bytes("file", path, &bytes, None, &self.languages, &self.settings.lock().unwrap()))
    }

    /// `textFileService.save` of a model: an untitled one asks for a path (Save As), a read-only one is not saved, a
    /// clean one is written only with `force`; save participants run first unless skipped. The receiver answers
    /// whether the model was saved.
    pub(super) fn save_model_with(&mut self, scheme: String, path: PathBuf, reason: SaveReason, options: SaveOptions, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        if scheme == SEARCH_EDITOR_SCHEME {
            return self.save_search_editor(path, cx);
        }
        if scheme == "untitled" {
            return self.save_untitled(path, options, cx);
        }
        if scheme == super::url_actions::TRUSTED_DOMAINS_SCHEME {
            return self.save_trusted_domains(&path, cx);
        }
        let (sender, receiver) = oneshot::channel();
        let Some(model) = self.editors.model_mut(&scheme, &path) else {
            let _ = sender.send(false);
            return receiver;
        };
        if !options.force && !model.is_dirty() {
            let _ = sender.send(true);
            return receiver;
        }
        // textFileEditorModel.ts `doSave`: the edits so far undo as one step back to the saved text.
        model.push_undo_stop();
        if self.readonly(&scheme, &path).is_some() {
            let _ = sender.send(false);
            return receiver;
        }
        if !options.skip_participants {
            self.run_save_participants(&scheme, &path, reason);
        }
        let editor = self.editors.model(&scheme, &path).expect("participants keep the model");
        let version = editor.alternative_version_id();
        let bytes = editor.saved_bytes();
        if scheme == "file" && options.elevated {
            let write = cx.background_executor().spawn({
                let path = path.clone();
                async move { write_elevated(&path, &bytes, options.unlock) }
            });
            cx.spawn(async move |workbench, cx| {
                let written = write.await;
                let saved = workbench.update(cx, |workbench, cx| {
                    if let Some(stat) = written.is_ok().then(|| stat(&path)).flatten() {
                        workbench.text_files.stats.insert(path.clone(), stat);
                    }
                    let saved = workbench.finish_save(&scheme, &path, version, written, cx);
                    workbench.save_signal(saved, reason);
                    saved
                });
                let _ = sender.send(saved.unwrap_or(false));
            })
            .detach();
            return receiver;
        }
        if scheme == "file" {
            let written = self.write_text_file(&path, &editor.document.language_id.clone(), &bytes, options);
            let saved = self.finish_save(&scheme, &path, version, written, cx);
            self.save_signal(saved, reason);
            let _ = sender.send(saved);
            return receiver;
        }
        let write = self.host.run_files(FileJob::Write(resource(&editor.document.uri()), bytes));
        cx.spawn(async move |workbench, cx| {
            let written = match write.await {
                Ok(result) => result.map(drop).map_err(|error| SaveError::Other(error.to_string())),
                Err(_) => Err(SaveError::Other("Canceled".to_string())),
            };
            let saved = workbench.update(cx, |workbench, cx| {
                let saved = workbench.finish_save(&scheme, &path, version, written, cx);
                workbench.save_signal(saved, reason);
                saved
            });
            let _ = sender.send(saved.unwrap_or(false));
        })
        .detach();
        receiver
    }

    /// fileService.ts `validateWriteFile` with the model's last stat as `mtime` and `etag`, then the write: a file
    /// changed on disk since (newer, and of another size) fails with `FILE_MODIFIED_SINCE` unless
    /// `files.saveConflictResolution` is `overwriteFileOnDisk`.
    fn write_text_file(&mut self, path: &Path, language_id: &str, bytes: &[u8], options: SaveOptions) -> Result<(), SaveError> {
        let overrides = Overrides { resource: Some(path), override_identifier: Some(language_id) };
        let resolution = self.settings.lock().unwrap().get_value(Some("files.saveConflictResolution"), &overrides);
        let prevent_conflicts = resolution != Some(json!("overwriteFileOnDisk"));
        if let (true, false, Some(&(mtime, size)), Some((disk_mtime, disk_size))) = (prevent_conflicts, options.ignore_modified_since, self.text_files.stats.get(path), stat(path))
            && mtime < disk_mtime
            && size != disk_size
        {
            return Err(SaveError::ModifiedSince);
        }
        if options.unlock {
            // diskFileSystemProvider.ts `open` with `unlock`: the owner's write bit, errors logged only.
            if let Err(error) = unlock(path) {
                eprintln!("unlocking {}: {error}", path.display());
            }
        }
        // FileUserDataProvider: the user's settings, keybindings, tasks and extensions files are written atomically.
        let user_data = ["settings.json", "keybindings.json", "tasks.json", "extensions.json"].iter().any(|name| path.ends_with(name))
            && path.starts_with(self.settings.lock().unwrap().user_home());
        let written = match user_data {
            true => config::service::write_user_data_file(path, bytes),
            false => std::fs::write(path, bytes),
        };
        written.map_err(|error| write_error(path, error, options.unlock))?;
        if let Some(stat) = stat(path) {
            self.text_files.stats.insert(path.to_path_buf(), stat);
        }
        Ok(())
    }

    /// The model is clean up to the version that was written, and the host hears it was saved; a failed save shows
    /// the save error message.
    pub(super) fn finish_save(&mut self, scheme: &str, path: &Path, version: u32, written: Result<(), SaveError>, cx: &mut Context<Self>) -> bool {
        let uri = uri_string(scheme, path);
        if let Err(error) = written {
            self.show_save_error(scheme, path, error);
            cx.notify();
            return false;
        }
        self.close_save_error(&uri, cx);
        self.text_files.orphaned.remove(path);
        // An editor closed while its model was written has nothing left to mark.
        let Some(model) = self.editors.model_mut(scheme, path) else { return true };
        let was_dirty = model.is_dirty();
        model.mark_saved_at(version);
        let became_clean = was_dirty && !model.is_dirty();
        let model = self.editors.model(scheme, path).expect("found above");
        self.host.send_saved(&self.editors, model, became_clean);
        // storedFileWorkingCopy.ts saves only a dirty model, and only those saves reach the history tracker.
        if scheme == "file" && was_dirty {
            self.add_local_history_entry(path, cx);
        }
        cx.notify();
        true
    }

    /// textFileSaveErrorHandler.ts `onSaveError`: one message per model, with the actions that resolve it.
    fn show_save_error(&mut self, scheme: &str, path: &Path, error: SaveError) {
        let uri = uri_string(scheme, path);
        let uri_value = match self.editors.model(scheme, path) {
            Some(model) => model.document.uri(),
            None => json!({ "$mid": 1, "scheme": scheme, "authority": "", "path": path, "query": "", "fragment": "" }),
        };
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let action = |id: &str, label: &str| Action { id: id.into(), label: label.into(), run: Run::Command(id.into(), vec![uri_value.clone()]) };
        let mut notification = match error {
            SaveError::ModifiedSince => {
                let message = format!("Failed to save '{name}': The content of the file is newer. Please compare your version with the file contents or overwrite the content of the file with your changes.");
                let mut notification = Notification::new(Severity::Error, message);
                // ResolveSaveConflictAction (Compare) opens the diff editor, which the port does not have yet.
                notification.primary = vec![action("workbench.files.action.saveIgnoreModifiedSince", "Overwrite")];
                let configure = Run::Command("workbench.action.openSettings".into(), vec![json!("files.saveConflictResolution")]);
                notification.secondary = vec![Action { id: "workbench.files.action.configureSaveConflict".into(), label: "Configure".into(), run: configure }];
                notification
            }
            error => {
                let elevated = |label: &str, unlock: bool| Action {
                    id: "workbench.files.action.saveModelElevated".into(),
                    label: label.into(),
                    run: Run::Command("workbench.files.action.saveModelElevated".into(), vec![uri_value.clone(), json!(unlock)]),
                };
                let (message, first) = match error {
                    SaveError::WriteLocked { tried_unlock: true } => {
                        (format!("Failed to save '{name}': File is read-only. Select 'Overwrite as Sudo' to retry as superuser."), elevated("Overwrite as Sudo...", true))
                    }
                    SaveError::WriteLocked { tried_unlock: false } => {
                        (format!("Failed to save '{name}': File is read-only. Select 'Overwrite' to attempt to make it writeable."), action("workbench.files.action.unlock", "Overwrite"))
                    }
                    SaveError::PermissionDenied => {
                        (format!("Failed to save '{name}': Insufficient permissions. Select 'Retry as Sudo' to retry as superuser."), elevated("Retry as Sudo...", false))
                    }
                    SaveError::Other(message) => (format!("Failed to save '{name}': {message}"), action("workbench.files.action.saveModel", "Retry")),
                    SaveError::ModifiedSince => unreachable!("matched above"),
                };
                let mut notification = Notification::new(Severity::Error, message);
                notification.primary = vec![first, action("workbench.files.action.saveModelAs", "Save As..."), action("workbench.files.action.revertModel", "Revert")];
                notification
            }
        };
        // `id: hash(model.resource)`: a new error replaces the model's last message (#121539).
        notification.id = Some(uri.clone());
        if let Some(key) = self.notify(notification) {
            self.text_files.save_errors.insert(uri, key);
        }
    }

    /// `onFileSavedOrReverted`: the model's save error message closes.
    fn close_save_error(&mut self, uri: &str, cx: &mut Context<Self>) {
        let Some(key) = self.text_files.save_errors.remove(uri) else { return };
        self.in_window(cx, move |workbench, window, cx| workbench.close_notification(key, window, cx));
    }

    /// Runs `run` with the workbench's window, for work a caller without one must do there.
    fn in_window(&self, cx: &mut Context<Self>, run: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
        cx.spawn(async move |_, cx| {
            let window = cx.update(|cx| cx.windows().into_iter().find_map(|window| window.downcast::<Workbench>()));
            if let Some(window) = window {
                let _ = window.update(cx, |workbench, window, cx| run(workbench, window, cx));
            }
        })
        .detach();
    }

    /// `textFileService.save` of an untitled model: to its associated file, else to the path picked in the save dialog.
    fn save_untitled(&mut self, path: PathBuf, options: SaveOptions, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let model = self.editors.model("untitled", &path).expect("saving an open model");
        match model.associated_file {
            true => self.save_as_to("untitled".into(), path.clone(), path, options, cx),
            false => self.save_as("untitled".into(), path, options, cx),
        }
    }

    /// `saveAs` without a target: the save dialog at `suggestSavePath`, then the save to the picked path.
    pub(super) fn save_as(&mut self, scheme: String, path: PathBuf, options: SaveOptions, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        let suggested = self.suggest_save_path(&scheme, &path);
        let directory = suggested.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = suggested.file_name().map(|name| name.to_string_lossy().into_owned());
        let picked = cx.prompt_for_new_path(&directory, name.as_deref());
        cx.spawn(async move |workbench, cx| {
            let Ok(Ok(Some(target))) = picked.await else {
                let _ = sender.send(false);
                return;
            };
            let saved = workbench.update(cx, |workbench, cx| workbench.save_as_to(scheme, path, target, options, cx));
            let saved = match saved {
                Ok(saved) => saved.await.unwrap_or(false),
                Err(_) => false,
            };
            let _ = sender.send(saved);
        })
        .detach();
        receiver
    }

    /// `saveAs(source, target)`: a read-only target asks to save anyway, an untitled model's associated file that
    /// exists asks to be replaced (#67946); the source's own file saves in place.
    fn save_as_to(&mut self, scheme: String, path: PathBuf, target: PathBuf, options: SaveOptions, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        if scheme == "file" && path == target {
            return self.save_model_with(scheme, path, SaveReason::Explicit, SaveOptions { force: true, ..options }, cx);
        }
        let readonly = self.readonly("file", &target).is_some();
        let associated = scheme == "untitled" && self.editors.model(&scheme, &path).is_some_and(|model| model.associated_file);
        let replace = associated && target == path && target.exists();
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let folder = target.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy().into_owned();
        let (sender, receiver) = oneshot::channel();
        let skip_dialogs = self.skip_dialogs;
        let this = cx.entity().downgrade();
        self.in_window(cx, move |_, window, cx| {
            let readonly_answer = (readonly && !skip_dialogs).then(|| {
                let detail = "Paths can be configured as read-only via settings.";
                window.prompt(PromptLevel::Warning, &format!("'{name}' is marked as read-only. Do you want to save anyway?"), Some(detail), &["Save Anyway", "Cancel"], cx)
            });
            let replace_answer = (replace && !skip_dialogs).then(|| {
                let detail = format!("A file or folder with the name '{name}' already exists in the folder '{folder}'. Replacing it will overwrite its current contents.");
                window.prompt(PromptLevel::Warning, &format!("'{name}' already exists. Do you want to replace it?"), Some(&detail), &["Replace", "Cancel"], cx)
            });
            window
                .spawn(cx, async move |cx| {
                    for answer in [readonly_answer, replace_answer].into_iter().flatten() {
                        if answer.await != Ok(0) {
                            let _ = sender.send(false);
                            return;
                        }
                    }
                    let saved = this.update(cx, |workbench, cx| {
                        if readonly {
                            workbench.readonly_overrides.insert(("file".into(), target.clone()), false);
                        }
                        workbench.do_save_as(&scheme, &path, target, options, cx)
                    });
                    let _ = sender.send(saved.unwrap_or(false));
                })
                .detach();
        });
        receiver
    }

    /// `doSaveAsTextFile` and `editorService.saveAs`: the target's model takes the source's text, encoding, more
    /// specific language, indentation and EOL and is saved; then the source reverts and the target's editor
    /// takes the source editor's place with its cursors and scroll.
    pub(super) fn do_save_as(&mut self, scheme: &str, path: &Path, target: PathBuf, options: SaveOptions, cx: &mut Context<Self>) -> bool {
        let Some(source) = self.editors.model(scheme, path) else { return false };
        let text = source.document.lines.join(source.document.eol);
        let (encoding, language, indentation, eol) = (source.document.encoding.clone(), source.document.language_id.clone(), source.document.indentation, source.document.eol);
        let (selections, scroll) = (source.selections.clone(), source.scroll);
        if self.editors.model("file", &target).is_none() {
            let document = match target.exists() {
                true => self.read_document(&target),
                false => Ok(TextDocument::from_text("file", &target, "", encoding.clone(), None, &self.languages, &self.settings.lock().unwrap())),
            };
            match document {
                Ok(document) => drop(self.editors.open_model(document)),
                Err(error) => {
                    self.show_save_error("file", &target, SaveError::Other(error.to_string()));
                    return false;
                }
            }
            self.editors_changed(cx);
        }
        self.update_model("file", &target, &text, false);
        let target_model = self.editors.model_mut("file", &target).expect("opened above");
        target_model.document.encoding = encoding;
        target_model.document.eol = eol;
        target_model.set_indentation(indentation);
        let target_language = target_model.document.language_id.clone();
        if language != PLAINTEXT && target_language == PLAINTEXT {
            self.set_model_language("file", &target, &language, cx);
        }
        let saved = self.save_model_with("file".into(), target.clone(), SaveReason::Explicit, SaveOptions { force: true, ..options }, cx);
        if saved_now(saved) != Some(true) {
            return false;
        }
        let source_index = self.editors.editors.iter().position(|editor| editor.document.is(scheme, path));
        match scheme {
            "untitled" => {
                self.revert_untitled(path);
                self.editors.models.retain(|model| !model.document.is("untitled", path));
            }
            _ => {
                if let Err(error) = self.revert_text_file(scheme, path, cx) {
                    eprintln!("reverting {}: {error}", path.display());
                }
            }
        }
        if let Some(source_index) = source_index {
            self.replace_editor_with_model(source_index, &target);
            let editor = self.editors.active_mut().expect("just shown");
            editor.set_selections(selections);
            editor.scroll = scroll;
        }
        self.editors_changed(cx);
        true
    }

    /// `replaceEditors`: the editor at `index` closes and `target`'s editor shows in its place, the one already open
    /// moved there.
    fn replace_editor_with_model(&mut self, index: usize, target: &Path) {
        let pinned_target = self.editors.editors.iter().position(|editor| editor.document.is("file", target));
        match pinned_target {
            Some(open) => {
                self.editors.activate(open);
                self.editors.pin(open);
                self.editors.close(index);
            }
            None => {
                self.editors.activate(index);
                let model = self.editors.models.iter().position(|model| model.document.is("file", target)).expect("opened by the save");
                self.editors.show_model(model, false);
                self.editors.close(index);
            }
        }
    }

    /// `suggestSavePath`: an untitled model's associated file; else its name, with the language's extension when
    /// it has none of its own (`suggestFilename`), in the dialog's default folder; a file's own path.
    fn suggest_save_path(&self, scheme: &str, path: &Path) -> PathBuf {
        let Some(model) = self.editors.model(scheme, path) else { return path.to_path_buf() };
        if scheme == "file" || model.associated_file {
            return path.to_path_buf();
        }
        let mut name = crate::untitled::name(model, self.untitled_names_from_content());
        if name.contains('/') || name.starts_with('.') && name.chars().all(|char| char == '.') {
            name = model.document.name();
        }
        let language = &model.document.language_id;
        let name = match language != PLAINTEXT {
            true => self.languages.suggest_file_name(language, &name),
            false => name,
        };
        self.default_file_path().join(name)
    }

    /// `fileDialogService.defaultFilePath`: `files.dialog.defaultPath`, else the folder of the last active file, else
    /// the first workspace folder, else the home folder.
    pub(super) fn default_file_path(&self) -> PathBuf {
        let configured = self.settings.lock().unwrap().get_value(Some("files.dialog.defaultPath"), &Overrides::default());
        if let Some(configured) = configured.as_ref().and_then(Value::as_str).map(PathBuf::from).filter(|path| path.is_absolute()) {
            return configured;
        }
        // fileDialogService.ts `defaultFilePath`: the last active file's folder, then the first workspace folder, then home.
        let last_active_folder = self.quick_access.editor_history.entries.first().and_then(|file| file.parent()).map(Path::to_path_buf);
        let workspace_folder = || self.workspace().and_then(|workspace| workspace.folders.first()).map(|folder| folder.path.clone());
        last_active_folder.or_else(workspace_folder).or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_default()
    }

    /// textFileEditorModel.ts `onDidChangeFilesAssociation`: a file model without a picked language takes the one
    /// its name and first line give now.
    pub(super) fn files_associations_changed(&mut self, cx: &mut Context<Self>) {
        let associations = self.settings.lock().unwrap().get_value(Some("files.associations"), &Overrides::default()).unwrap_or_default();
        let mut changed = Vec::new();
        for model in self.editors.editors.iter().chain(&self.editors.models).filter(|model| model.document.scheme == "file") {
            let document = &model.document;
            if self.text_files.preferred_languages.contains(&(document.scheme.clone(), document.path.clone())) {
                continue;
            }
            let configured = crate::languages::Languages::configured_language(&associations, &document.path);
            let language = configured.unwrap_or_else(|| self.languages.language_of(&document.path, &document.lines[0]).to_string());
            if language != document.language_id {
                changed.push((document.path.clone(), language));
            }
        }
        for (path, language) in changed {
            self.set_model_language("file", &path, &language, cx);
        }
    }

    /// labels.ts: an untitled model named by its content has its resource path as its label's description.
    pub(super) fn untitled_description(&self, editor: &OpenEditor) -> Option<String> {
        if editor.document.scheme != "untitled" || editor.associated_file {
            return None;
        }
        let path = editor.document.path.to_string_lossy().into_owned();
        (crate::untitled::name(editor, self.untitled_names_from_content()) != path).then_some(path)
    }

    /// `workbench.editor.untitled.labelFormat` is `content`.
    pub(super) fn untitled_names_from_content(&self) -> bool {
        self.settings.lock().unwrap().get_value(Some("workbench.editor.untitled.labelFormat"), &Overrides::default()) != Some(json!("name"))
    }

    /// `textFileService.revert` of a file's model: read again in its encoding (`forceResolveFromFile`) and clean; a
    /// file that is gone leaves the text and only cleans the model.
    pub(super) fn revert_text_file(&mut self, scheme: &str, path: &Path, cx: &mut Context<Self>) -> Result<(), String> {
        if scheme != "file" {
            if let Some(model) = self.editors.model_mut(scheme, path) {
                model.mark_saved();
            }
            return Ok(());
        }
        let Some(model) = self.editors.model(scheme, path) else { return Ok(()) };
        let (was_dirty, encoding) = (model.is_dirty(), model.document.encoding.clone());
        let sent = match std::fs::read(path) {
            Ok(bytes) => {
                let sent = self.update_model(scheme, path, &encoding::decode(&bytes, &encoding), true);
                if let Some(stat) = stat(path) {
                    self.text_files.stats.insert(path.to_path_buf(), stat);
                }
                sent
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(format!("Unable to read file '{}' ({error})", path.display())),
        };
        let model = self.editors.model_mut(scheme, path).expect("found above");
        model.mark_saved();
        if was_dirty && !sent {
            let model = self.editors.model(scheme, path).expect("found above");
            self.host.send_dirty_state(&self.editors, model, false);
        }
        self.close_save_error(&uri_string(scheme, path), cx);
        cx.notify();
        Ok(())
    }

    /// untitledTextEditorModel.ts `revert`: the text is emptied and the model disposed, also for the extension host;
    /// the caller closes its editor.
    pub(super) fn revert_untitled(&mut self, path: &Path) {
        self.update_model("untitled", path, "", false);
        let model = self.editors.model_mut("untitled", path).expect("still open");
        let dirty = model.is_dirty();
        model.associated_file = false;
        model.host_reference = None;
        if dirty {
            let model = self.editors.model("untitled", path).expect("found above");
            self.host.send_dirty_state(&self.editors, model, false);
        }
    }

    /// `modelService.updateModel`: between undo stops, the model takes the text's EOL and one edit over the lines
    /// that differ (`_computeEdits`); the same text changes nothing. With `clean` the model is clean after it, before
    /// the host hears the change (textFileEditorModel.ts `ignoreDirtyOnModelContentChange`). Answers whether the
    /// host heard a change.
    pub(super) fn update_model(&mut self, scheme: &str, path: &Path, text: &str, clean: bool) -> bool {
        let model = self.editors.model_mut(scheme, path).expect("the caller found the model");
        let (lines, eol) = crate::editor::split_lines(text);
        let eol_change = eol.and_then(|eol| model.push_eol(if eol == "\r\n" { 1 } else { 0 }));
        let change = match compute_edits(&model.document.lines, &lines, model.document.eol) {
            Some(edit) => {
                model.push_undo_stop();
                let change = model.execute_edits(vec![(edit.start, edit.end, edit.text, true)], None, EditReason::Unknown("updateModel"));
                model.push_undo_stop();
                change
            }
            None => None,
        };
        if clean {
            model.mark_saved();
        }
        let Some(change) = change.or(eol_change) else { return false };
        if self.editors.active().is_some_and(|editor| editor.document.is(scheme, path)) {
            self.find_editor_edited(Some(&change));
        }
        self.send_model_edit(scheme, path, &change);
        true
    }

    /// The text file part of `editors_changed`: the host's untitled tab names, the watched folders and visible files
    /// outside them, and the stats of models that are gone.
    pub(super) fn text_files_editors_changed(&mut self) {
        self.host.untitled_names_from_content = self.untitled_names_from_content();
        let open: HashSet<&Path> = self.editors.editors.iter().chain(&self.editors.models).filter(|model| model.document.scheme == "file").map(|model| model.document.path.as_path()).collect();
        self.text_files.stats.retain(|path, _| open.contains(path.as_path()));
        self.text_files.orphaned.retain(|path| open.contains(path.as_path()));
        let models: HashSet<(&str, &Path)> = self.editors.editors.iter().chain(&self.editors.models).map(|model| (model.document.scheme.as_str(), model.document.path.as_path())).collect();
        self.text_files.preferred_languages.retain(|(scheme, path)| models.contains(&(scheme.as_str(), path.as_path())));
        self.watch_visible_file();
        let shown = self.editors.active().map(|editor| editor.model_id);
        if shown != std::mem::replace(&mut self.text_files.shown_model, shown) {
            self.warn_large_file();
        }
    }

    /// `WorkspaceWatcher.watchWorkspace`: the folders recursively, minus `files.watcherExclude`; the changes reach
    /// `files_changed`.
    pub fn start_file_watcher(&mut self, cx: &mut Context<Self>) {
        let (sender, mut receiver) = mpsc::unbounded::<Vec<Change>>();
        let mut watcher = match Watcher::new(move |changes| drop(sender.unbounded_send(changes))) {
            Ok(watcher) => watcher,
            Err(error) => return eprintln!("[File Watcher] {error}"),
        };
        let folders: Vec<PathBuf> = self.workspace().into_iter().flat_map(|workspace| workspace.folders.iter().map(|folder| folder.path.clone())).collect();
        for folder in folders {
            let excludes = self.settings.lock().unwrap().get_value(Some("files.watcherExclude"), &Overrides { resource: Some(&folder), ..Overrides::default() });
            let excludes: Vec<String> = excludes.as_ref().and_then(Value::as_object).into_iter().flatten().filter(|(_, on)| **on == true).map(|(pattern, _)| pattern.clone()).collect();
            self.text_files.next_watch += 1;
            if let Err(error) = watcher.watch(self.text_files.next_watch, &folder, true, &excludes) {
                eprintln!("[File Watcher] {}: {error}", folder.display());
            }
        }
        self.text_files.watcher = Some(watcher);
        self.watch_visible_file();
        cx.spawn(async move |workbench, cx| {
            while let Some(changes) = receiver.next().await {
                if workbench.update(cx, |workbench, cx| workbench.files_changed(changes, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// editorService.ts `handleVisibleEditorsChange`: the visible file outside the folders is watched alone.
    fn watch_visible_file(&mut self) {
        if self.text_files.watcher.is_none() {
            return;
        }
        let inside = |path: &Path| self.workspace().is_some_and(|workspace| workspace.folders.iter().any(|folder| path.starts_with(&folder.path)));
        let visible = self.editors.active().filter(|editor| editor.document.scheme == "file" && !inside(&editor.document.path)).map(|editor| editor.document.path.clone());
        let text_files = &mut self.text_files;
        let watcher = text_files.watcher.as_mut().expect("started above");
        text_files.watched_files.retain(|path, id| {
            let keep = Some(path) == visible.as_ref();
            if !keep {
                watcher.unwatch(*id);
            }
            keep
        });
        if let Some(path) = visible.filter(|path| !text_files.watched_files.contains_key(path)) {
            text_files.next_watch += 1;
            match watcher.watch(text_files.next_watch, &path, false, &[]) {
                Ok(()) => drop(text_files.watched_files.insert(path, text_files.next_watch)),
                Err(error) => eprintln!("[File Watcher] {}: {error}", path.display()),
            }
        }
    }

    /// `onDidFilesChange`: a clean model whose file was updated or added reads it again; a deleted file orphans
    /// its model once it is still gone 100 ms later (#13665), or closes its clean editors with
    /// `workbench.editor.closeOnFileDelete`; a file added back ends the orphan state.
    pub(super) fn files_changed(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        for change in changes {
            let Some(model) = self.editors.model("file", &change.path) else { continue };
            let dirty = model.is_dirty();
            match change.kind {
                ChangeType::Updated | ChangeType::Added => {
                    if self.text_files.orphaned.remove(&change.path) {
                        cx.notify();
                    }
                    if !dirty {
                        self.reload_model(&change.path, cx);
                    }
                }
                ChangeType::Deleted if !self.text_files.orphaned.contains(&change.path) => {
                    let path = change.path;
                    cx.spawn(async move |workbench, cx| {
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                        if !path.exists() {
                            let _ = workbench.update(cx, |workbench, cx| workbench.file_deleted(path, cx));
                        }
                    })
                    .detach();
                }
                ChangeType::Deleted => {}
            }
        }
    }

    fn file_deleted(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(model) = self.editors.model("file", &path) else { return };
        let close = !model.is_dirty() && self.settings.lock().unwrap().get_value(Some("workbench.editor.closeOnFileDelete"), &Overrides::default()) == Some(json!(true));
        if close {
            let mut indices: Vec<usize> = (0..self.editors.editors.len()).filter(|index| self.editors.editors[*index].document.is("file", &path)).collect();
            indices.reverse();
            for index in indices {
                self.editors.close(index);
            }
            self.editors_changed(cx);
            return;
        }
        self.text_files.orphaned.insert(path);
        cx.notify();
    }

    /// `resolveFromFile` after a change on disk: a file with the stat the model last saw is not read again
    /// (`FILE_NOT_MODIFIED_SINCE`), else its text replaces the model's as `updateModel` does and the model is clean.
    fn reload_model(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(disk) = stat(path) else { return };
        if self.text_files.stats.get(path) == Some(&disk) {
            return;
        }
        let Ok(bytes) = std::fs::read(path) else { return };
        let encoding = self.editors.model("file", path).expect("the caller found the model").document.encoding.clone();
        self.update_model("file", path, &encoding::decode(&bytes, &encoding), true);
        self.text_files.stats.insert(path.to_path_buf(), disk);
        cx.notify();
    }
}

/// diskFileSystemProvider.ts `toFileSystemProviderWriteError`: no permission on a file without the owner's write
/// bit is `FILE_WRITE_LOCKED`.
fn write_error(path: &Path, error: std::io::Error, tried_unlock: bool) -> SaveError {
    use std::os::unix::fs::PermissionsExt;
    if error.kind() != std::io::ErrorKind::PermissionDenied {
        return SaveError::Other(error.to_string());
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.permissions().mode() & 0o200 == 0 => SaveError::WriteLocked { tried_unlock },
        _ => SaveError::PermissionDenied,
    }
}

fn unlock(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    if permissions.mode() & 0o200 == 0 {
        permissions.set_mode(permissions.mode() | 0o200);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// nativeHostMainService.ts `writeElevated`: the bytes go to a temporary file the superuser copies over the
/// target (with `unlock`, after giving the owner write permission), behind the system's administrator prompt;
/// sudo-prompt's macOS applet is the same prompt as AppleScript's `with administrator privileges`.
fn write_elevated(path: &Path, bytes: &[u8], unlock: bool) -> Result<(), SaveError> {
    let source = std::env::temp_dir().join(format!("code-elevated-{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_nanos())));
    std::fs::write(&source, bytes).map_err(|error| SaveError::Other(error.to_string()))?;
    let command = match unlock {
        true => r#"do shell script "chmod u+w " & quoted form of item 2 of argv & "; cat " & quoted form of item 1 of argv & " > " & quoted form of item 2 of argv with administrator privileges"#,
        false => r#"do shell script "cat " & quoted form of item 1 of argv & " > " & quoted form of item 2 of argv with administrator privileges"#,
    };
    let output = std::process::Command::new("/usr/bin/osascript").args(["-e", "on run argv", "-e", command, "-e", "end run"]).arg(&source).arg(path).output();
    let _ = std::fs::remove_file(&source);
    let output = output.map_err(|error| SaveError::Other(error.to_string()))?;
    match output.status.success() {
        true => Ok(()),
        false => Err(SaveError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string())),
    }
}

fn stat(path: &Path) -> Option<Stat> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The answer of a save that finished without waiting, as a file save does.
pub(super) fn saved_now(mut saved: oneshot::Receiver<bool>) -> Option<bool> {
    saved.try_recv().ok().flatten()
}

impl Workbench {
    /// The text file editor's `EditorMemento` from workspace storage.
    pub(super) fn restore_editor_view_states(&mut self) {
        let stored = self.workspace_storage.lock().unwrap().get(view_state::STORAGE_KEY).map(str::to_owned);
        self.editors.view_states = stored.map(|raw| ViewStates::from_memento(&raw)).unwrap_or_default();
    }

    /// `saveState` on shutdown: the open file editors' view states, then the memento.
    pub(super) fn store_editor_view_states(&mut self) {
        let editors = std::mem::take(&mut self.editors.editors);
        for editor in &editors {
            self.editors.save_view_state(editor);
        }
        self.editors.editors = editors;
        let memento = self.editors.view_states.to_memento();
        self.workspace_storage.lock().unwrap().set(view_state::STORAGE_KEY, memento);
    }
}
